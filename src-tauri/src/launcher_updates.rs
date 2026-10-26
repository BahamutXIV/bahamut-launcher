//! Signed portable launcher updates coordinated by the packaged helper.

use std::sync::atomic::AtomicU64;

mod apply;
mod config;
mod fetch;
mod fs_io;
mod helper;
mod locks;
mod pending;
mod receipts;
mod recovery;
mod status;
#[cfg(test)]
mod tests;
mod verify;

pub(crate) use apply::apply_launcher_update;
pub(crate) use pending::{
    confirm_pending_launcher_update, finalize_recovered_launcher_update,
    has_recovered_update_argument, has_update_transaction_argument,
};
pub(crate) use recovery::recover_failed_launcher_update;
pub(crate) use status::{
    LauncherUpdateResult, LauncherUpdateState, LauncherUpdateStatus, check_launcher_update,
    get_launcher_update_status,
};

const STATE_FILE: &str = "config/launcher-release-state.json";

const INSTALLED_FILE: &str = "config/launcher-installed.json";

const OFFER_FILE: &str = "config/launcher-update-offer.json";

const ARTIFACT_CACHE: &str = "cache/launcher-updates";

const UPDATE_HELPER_FILE: &str = "bahamut-update-helper.exe";

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);
