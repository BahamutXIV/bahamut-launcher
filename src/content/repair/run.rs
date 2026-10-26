//! The repair sequence: recover, scan, download, stage, publish, verify, commit.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::archive::{
    RepairSource, extract_selected_file, repair_sources, required_archive_indices,
    write_staged_bytes,
};
use super::control::{RepairPhase, RepairShared};
use super::journal::{
    JournalPhase, RepairJournal, cleanup_unjournaled_transaction, create_transaction, make_journal,
    write_journal,
};
use super::paths::{
    ResolvedCachePath, find_file, resolve_cache_path, resolve_existing_directory, transaction_path,
};
use super::publish::{
    detach_published_stage_links, ensure_snapshots_unchanged, missing_parent_directories,
    publish_selected, validate_staged_publication,
};
use super::quote::{preflight_space, quote_for_statuses, quote_for_statuses_with_cache, summary};
use super::recover::{
    cleanup_committed_transaction, inspect_recovery_state, recover_transaction,
    rollback_after_final_verification_error,
};
use super::scan::{scan_files_controlled, scan_selected_controlled, select_repair_files};
use super::types::{FileState, ManagedFileStatus, RepairQuote, RepairResult, VerificationSummary};
use super::{STAGED_DIR, acquire_repair_lock};
use crate::content::http::download_object;
use crate::content::installer::package_identity;
use crate::content::manifest::{BasePackage, InstallFile};
use crate::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};

/// Inputs fixed for the whole repair.
struct Run<'a> {
    package: &'a BasePackage,
    shared: &'a RepairShared,
    content_root: &'a str,
    sources: HashMap<String, RepairSource>,
    game: PathBuf,
    cache_path: ResolvedCachePath,
    package_sha256: String,
}

/// The files a repair replaces and what it costs.
struct Plan<'a> {
    selected: Vec<&'a InstallFile>,
    selected_statuses: Vec<ManagedFileStatus>,
    quote: RepairQuote,
    cached_archive_indices: HashSet<usize>,
}

enum Scan<'a> {
    UpToDate(RepairResult),
    NeedsRepair(Plan<'a>),
}

pub(super) fn repair_all_inner(
    destination: &Path,
    content_root: &str,
    cache: &Path,
    package: &BasePackage,
    shared: &RepairShared,
) -> Result<RepairResult, String> {
    package.validate()?;
    let sources = repair_sources(package)?;
    let game = resolve_existing_directory(destination)?;
    let _repair_lock = acquire_repair_lock(&game)?;
    let cache_path = resolve_cache_path(cache)?;
    let package_sha256 = package_identity(package);
    let transaction = transaction_path(&game)?;
    let run = Run {
        package,
        shared,
        content_root,
        sources,
        game,
        cache_path,
        package_sha256,
    };

    run.recover_interrupted(&transaction)?;
    let plan = match run.scan_and_plan(&transaction)? {
        Scan::UpToDate(result) => return Ok(result),
        Scan::NeedsRepair(plan) => plan,
    };
    let archives = run.download_archives(&plan)?;
    let transaction = run.stage(&transaction, &plan, &archives)?;
    let mut journal = run.write_staged_journal(&transaction, &plan)?;
    run.publish(&transaction, &plan, &mut journal)?;
    run.verify_published(&transaction, &plan)?;
    let verification = run.verify_inventory(&transaction, &plan)?;
    run.commit(&transaction, &mut journal)?;

    Ok(RepairResult {
        repaired_files: plan.selected.iter().map(|file| file.path.clone()).collect(),
        complete: true,
        verification,
    })
}

impl<'a> Run<'a> {
    fn recover_interrupted(&self, transaction: &Path) -> Result<(), String> {
        self.shared.begin_phase(RepairPhase::Recovering, None, None);
        self.shared.checkpoint()?;
        recover_transaction(transaction, &self.game, self.package, &self.package_sha256)?;
        self.shared.checkpoint()
    }

    /// Hash every managed file and choose the ones to replace.
    fn scan_and_plan(&self, transaction: &Path) -> Result<Scan<'a>, String> {
        let package = self.package;
        self.shared.begin_phase(
            RepairPhase::Verifying,
            None,
            Some(package.final_files.len()),
        );
        let before_files = scan_files_controlled(&self.game, &package.final_files, self.shared)?;
        let (recovery_required, recovery_detail) = inspect_recovery_state(transaction, package)?;
        if recovery_required {
            return Err(recovery_detail
                .unwrap_or_else(|| "A previous repair transaction needs recovery.".into()));
        }
        if let Some(unresolved) = before_files
            .iter()
            .find(|status| status.state == FileState::Unresolved)
        {
            return Err(format!(
                "Cannot safely repair {}: {}",
                unresolved.path,
                unresolved.detail.as_deref().unwrap_or("file access failed")
            ));
        }
        let (selected, selected_statuses) =
            select_repair_files(&package.final_files, &before_files)?;
        let (quote, cached_archive_indices) = quote_for_statuses_with_cache(
            &self.game,
            &self.cache_path.path,
            &self.cache_path.nearest_existing,
            package,
            &self.sources,
            &selected_statuses,
        )?;
        if selected.is_empty() {
            return Ok(Scan::UpToDate(RepairResult {
                repaired_files: Vec::new(),
                complete: true,
                verification: summary(&before_files, recovery_required, recovery_detail, quote),
            }));
        }
        if let Some(problem) = &quote.cache_problem {
            return Err(format!("The repair cache is unavailable: {problem}"));
        }
        preflight_space(&quote, &self.cache_path.path, &self.game)?;
        Ok(Scan::NeedsRepair(Plan {
            selected,
            selected_statuses,
            quote,
            cached_archive_indices,
        }))
    }

    /// Fetch every archive the plan needs, keyed by archive index.
    fn download_archives(&self, plan: &Plan<'_>) -> Result<HashMap<usize, PathBuf>, String> {
        let shared = self.shared;
        let mut required_archives = required_archive_indices(&self.sources, &plan.selected)
            .into_iter()
            .collect::<Vec<_>>();
        required_archives.sort_unstable();
        shared.begin_phase(
            RepairPhase::Downloading,
            Some(plan.quote.download_bytes),
            Some(required_archives.len()),
        );
        let mut cached_archives = HashMap::new();
        let mut downloaded_before = 0_u64;
        for index in required_archives {
            shared.checkpoint()?;
            let archive = &self.package.archives[index];
            shared.set_current_file(Some(&archive.object.object_key));
            let archive_was_cached = plan.cached_archive_indices.contains(&index);
            let path = download_object(
                self.content_root,
                &archive.object,
                &self.cache_path.path,
                |checkpoint| shared.download_checkpoint(checkpoint.durable),
                |bytes, _total| {
                    if !archive_was_cached {
                        shared.set_bytes(downloaded_before.saturating_add(bytes));
                    }
                },
            )
            .map_err(|error| {
                format!(
                    "Could not obtain the verified game archive {}: {error}",
                    archive.object.object_key
                )
            })?;
            if !archive_was_cached {
                downloaded_before = downloaded_before.saturating_add(archive.object.length);
                shared.set_bytes(downloaded_before);
            }
            cached_archives.insert(index, path);
            shared.complete_file();
        }
        Ok(cached_archives)
    }

    /// Create the transaction directory and write every replacement into it.
    /// Returns the transaction directory.
    fn stage(
        &self,
        transaction: &Path,
        plan: &Plan<'_>,
        archives: &HashMap<usize, PathBuf>,
    ) -> Result<PathBuf, String> {
        let replacement_bytes = plan
            .selected
            .iter()
            .try_fold(0_u64, |sum, file| sum.checked_add(file.length))
            .ok_or_else(|| "Repair staging size overflow.".to_string())?;
        self.shared.begin_phase(
            RepairPhase::Staging,
            Some(replacement_bytes),
            Some(plan.selected.len()),
        );
        let transaction = create_transaction(transaction, self.package, &self.package_sha256)?;
        let staged_root = transaction.join(STAGED_DIR);
        let extraction = (|| {
            for status in &plan.selected_statuses {
                self.shared.checkpoint()?;
                self.shared.set_current_file(Some(&status.path));
                self.stage_file(&staged_root, status, archives)?;
                self.shared.complete_file();
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = extraction {
            cleanup_unjournaled_transaction(&transaction, self.package, &self.package_sha256)?;
            return Err(error);
        }
        Ok(transaction)
    }

    fn stage_file(
        &self,
        staged_root: &Path,
        status: &ManagedFileStatus,
        archives: &HashMap<usize, PathBuf>,
    ) -> Result<(), String> {
        let file = find_file(self.package, &status.path)?;
        match status.path.to_ascii_lowercase().as_str() {
            "boot.ver" => {
                write_staged_bytes(staged_root, file, FFXIV_BOOT_VERSION.as_bytes())?;
                self.shared.add_bytes(file.length);
            }
            "game.ver" => {
                write_staged_bytes(staged_root, file, FFXIV_GAME_VERSION.as_bytes())?;
                self.shared.add_bytes(file.length);
            }
            _ => {
                let source = self
                    .sources
                    .get(&status.path.to_ascii_lowercase())
                    .ok_or_else(|| format!("No repair archive maps {}.", status.path))?;
                let archive_path = archives.get(&source.archive_index).ok_or_else(|| {
                    format!("The repair archive for {} was not prepared.", status.path)
                })?;
                extract_selected_file(
                    archive_path,
                    &self.package.archives[source.archive_index],
                    source,
                    file,
                    staged_root,
                    self.shared,
                )?;
            }
        }
        Ok(())
    }

    /// Confirm the live files still match the scan and persist the journal.
    /// Any failure removes the unjournaled transaction.
    fn write_staged_journal(
        &self,
        transaction: &Path,
        plan: &Plan<'_>,
    ) -> Result<RepairJournal, String> {
        self.shared
            .begin_phase(RepairPhase::Verifying, None, Some(plan.selected.len()));
        self.prepare_journal(transaction, plan).or_else(|error| {
            cleanup_unjournaled_transaction(transaction, self.package, &self.package_sha256)?;
            Err(error)
        })
    }

    fn prepare_journal(
        &self,
        transaction: &Path,
        plan: &Plan<'_>,
    ) -> Result<RepairJournal, String> {
        let fresh = scan_selected_controlled(&self.game, &plan.selected, self.shared)?;
        ensure_snapshots_unchanged(&plan.selected_statuses, &fresh)?;
        let created_directories = missing_parent_directories(&self.game, &plan.selected)?;
        let journal = make_journal(
            &self.package_sha256,
            &plan.selected_statuses,
            created_directories,
        )?;
        write_journal(transaction, &journal)?;
        Ok(journal)
    }

    /// Replace the live files; roll back if publishing fails.
    fn publish(
        &self,
        transaction: &Path,
        plan: &Plan<'_>,
        journal: &mut RepairJournal,
    ) -> Result<(), String> {
        self.shared
            .begin_phase(RepairPhase::Publishing, None, Some(plan.selected.len()));
        publish_selected(
            &self.game,
            transaction,
            &plan.selected,
            &plan.selected_statuses,
            journal,
            self.shared,
        )
        .map_err(|error| {
            self.roll_back(transaction, error, |error, recovery_error| {
                format!(
                    "{error}. Rollback is incomplete and will be retried on the next Repair: {recovery_error}"
                )
            })
        })
    }

    /// Hash the published files; roll back on a mismatch.
    fn verify_published(&self, transaction: &Path, plan: &Plan<'_>) -> Result<(), String> {
        self.shared
            .begin_phase(RepairPhase::Verifying, None, Some(plan.selected.len()));
        validate_staged_publication(transaction, &self.game, &plan.selected, self.shared)
            .map_err(|error| {
                let error = format!("Repaired file did not pass its final hash check: {error}");
                self.roll_back(transaction, error, |error, recovery_error| {
                    format!(
                        "{error} Rollback is incomplete and will be retried on the next Repair: {recovery_error}"
                    )
                })
            })
    }

    /// Hash the whole inventory and require it to be complete; every failure
    /// rolls back.
    fn verify_inventory(
        &self,
        transaction: &Path,
        plan: &Plan<'_>,
    ) -> Result<VerificationSummary, String> {
        self.shared.begin_phase(
            RepairPhase::FinalVerification,
            None,
            Some(self.package.final_files.len()),
        );
        self.inventory_summary(transaction, plan).map_err(|error| {
            rollback_after_final_verification_error(
                transaction,
                &self.game,
                self.package,
                &self.package_sha256,
                error,
            )
        })
    }

    fn inventory_summary(
        &self,
        transaction: &Path,
        plan: &Plan<'_>,
    ) -> Result<VerificationSummary, String> {
        detach_published_stage_links(transaction, &self.game, &plan.selected, self.shared)?;
        let verification_files =
            scan_files_controlled(&self.game, &self.package.final_files, self.shared)?;
        let quote = quote_for_statuses(
            &self.game,
            &self.cache_path.path,
            &self.cache_path.nearest_existing,
            self.package,
            &self.sources,
            &verification_files,
        )?;
        let verification = summary(&verification_files, false, None, quote);
        if verification.missing_count != 0
            || verification.corrupt_count != 0
            || verification.unresolved_count != 0
        {
            return Err(format!(
                "Final verification found {} missing, {} corrupt, and {} inaccessible managed files.",
                verification.missing_count,
                verification.corrupt_count,
                verification.unresolved_count
            ));
        }
        Ok(verification)
    }

    /// Mark the journal committed, then discard the transaction.
    fn commit(&self, transaction: &Path, journal: &mut RepairJournal) -> Result<(), String> {
        journal.phase = JournalPhase::Committed;
        self.shared
            .commit(|| write_journal(transaction, journal))
            .map_err(|error| {
                self.roll_back(transaction, error, |error, recovery_error| {
                    format!(
                        "Could not record completed repair: {error}. Rollback is incomplete and will be retried on the next Repair: {recovery_error}"
                    )
                })
            })?;
        // The full inventory was just hashed before commit; normal cleanup need not hash repaired files again.
        cleanup_committed_transaction(transaction, self.package, &self.package_sha256)
    }

    /// Roll the transaction back. Returns `error` when the rollback succeeds,
    /// otherwise `incomplete(error, rollback_error)`.
    fn roll_back(
        &self,
        transaction: &Path,
        error: String,
        incomplete: impl FnOnce(&str, &str) -> String,
    ) -> String {
        match recover_transaction(transaction, &self.game, self.package, &self.package_sha256) {
            Ok(()) => error,
            Err(recovery_error) => incomplete(&error, &recovery_error),
        }
    }
}
