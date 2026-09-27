//! Wine PE patches target a working copy because `WriteProcessMemory` cannot cross the Wine boundary; never pass the original binary.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use object::read::pe::PeFile32;
use object::{Object, ObjectSection};

use super::LaunchError;
use crate::launcher::pe_patch::PePatch;

/// Apply patches to a working copy after resolving every RVA; an unmapped RVA writes no bytes.
pub fn apply_patches_on_disk(exe_path: &Path, patches: &[PePatch]) -> Result<(), LaunchError> {
    let data = std::fs::read(exe_path).map_err(|source| LaunchError::Io {
        context: "reading the client working copy for patching",
        source,
    })?;
    let pe = PeFile32::parse(&*data)
        .map_err(|e| LaunchError::PeParse(format!("{}: {e}", exe_path.display())))?;

    let mut plan: Vec<(u64, &[u8])> = Vec::with_capacity(patches.len());
    for patch in patches {
        let file_offset =
            rva_to_file_offset(&pe, patch.rva).ok_or(LaunchError::RvaUnmapped(patch.rva))?;
        tracing::debug!(
            rva = format_args!("0x{:08X}", patch.rva),
            file_offset = format_args!("0x{:08X}", file_offset),
            bytes = patch.bytes.len(),
            "planned on-disk PE patch"
        );
        plan.push((file_offset, patch.bytes.as_slice()));
    }

    let mut file = OpenOptions::new()
        .write(true)
        .open(exe_path)
        .map_err(|source| LaunchError::Io {
            context: "opening the client working copy for patching",
            source,
        })?;
    for (offset, bytes) in plan {
        file.seek(SeekFrom::Start(offset))
            .map_err(|source| LaunchError::Io {
                context: "seeking in the client working copy",
                source,
            })?;
        file.write_all(bytes).map_err(|source| LaunchError::Io {
            context: "writing a patch into the client working copy",
            source,
        })?;
    }
    file.flush().map_err(|source| LaunchError::Io {
        context: "flushing the patched client working copy",
        source,
    })?;
    Ok(())
}

/// Translate an image RVA to a file offset; normalize `ObjectSection::address()` by the PE image base.
fn rva_to_file_offset(pe: &PeFile32<'_>, rva: u32) -> Option<u64> {
    let image_base = pe.relative_address_base() as u32;
    for section in pe.sections() {
        let section_rva = (section.address() as u32).checked_sub(image_base)?;
        let vsize = section.size() as u32;
        if rva >= section_rva && rva < section_rva.saturating_add(vsize) {
            let (file_offset, _) = section.file_range()?;
            return Some(file_offset + (rva - section_rva) as u64);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::pe_patch::{
        ASSERT_LOG_PATCH_RVA, ENCRYPTION_TIME_PATCH_RVA, NULL_MEMBER8_WRITE_NOP_RVA,
        NULL_THIS_GUARD_PATCH_RVA, plan_wine_patches,
    };

    /// Real-binary validation. Point `BAHAMUT_TEST_GAME_EXE` at an original,
    /// unpatched `ffxivgame.exe` to prove that (1) our RVA -> file-offset
    /// translation lands on the exact instructions the patches expect (the
    /// documented "before" bytes), and (2) the on-disk write lays down the
    /// patch bytes at those offsets. Skips when the env var is unset so the
    /// suite stays hermetic.
    #[test]
    fn patches_land_on_the_expected_instructions_in_a_real_exe() {
        let Some(src) = std::env::var_os("BAHAMUT_TEST_GAME_EXE") else {
            eprintln!("skipping: set BAHAMUT_TEST_GAME_EXE to an original ffxivgame.exe to run");
            return;
        };
        let src = std::path::PathBuf::from(src);

        // These bytes validate the expected instructions at each patch offset.
        let expected_before: &[(u32, &[u8])] = &[
            (ENCRYPTION_TIME_PATCH_RVA, &[0xE8]), // call rel32 (helper)
            (ASSERT_LOG_PATCH_RVA, &[0xFF, 0x15, 0xB4, 0x51, 0x26, 0x01]), // call [0x012651B4]
            (NULL_THIS_GUARD_PATCH_RVA, &[0x8B, 0x49, 0x04]), // mov ecx,[ecx+4]
            (NULL_MEMBER8_WRITE_NOP_RVA, &[0xC6, 0x46, 0x20, 0x00]), // mov byte [esi+0x20],0
        ];

        let original = std::fs::read(&src).unwrap();
        let pe = PeFile32::parse(&*original).unwrap();
        for (rva, before) in expected_before {
            let off = rva_to_file_offset(&pe, *rva)
                .unwrap_or_else(|| panic!("RVA 0x{rva:X} unmapped")) as usize;
            assert_eq!(
                &original[off..off + before.len()],
                *before,
                "unexpected original bytes at RVA 0x{rva:X} (offset 0x{off:X}) - offset math or binary version differs",
            );
        }

        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("ffxivgame.patched.exe");
        std::fs::copy(&src, &work).unwrap();
        let patches = plan_wine_patches("127.0.0.1").unwrap();
        apply_patches_on_disk(&work, &patches).unwrap();

        let patched = std::fs::read(&work).unwrap();
        let pe2 = PeFile32::parse(&*patched).unwrap();
        for patch in &patches {
            let off = rva_to_file_offset(&pe2, patch.rva).unwrap() as usize;
            assert_eq!(
                &patched[off..off + patch.bytes.len()],
                patch.bytes.as_slice(),
                "patched bytes mismatch at RVA 0x{:X}",
                patch.rva,
            );
        }
    }
}
