//! Debug-build crash hooks used by the helper process tests.

#[cfg(debug_assertions)]
pub(super) fn maybe_test_boundary(boundary: &str) {
    if std::env::var("BAHAMUT_UPDATE_HELPER_TEST_CRASH_AT").as_deref() == Ok(boundary) {
        std::process::exit(86);
    }
}

#[cfg(not(debug_assertions))]
pub(super) fn maybe_test_boundary(_boundary: &str) {}

#[cfg(debug_assertions)]
pub(super) fn maybe_test_interruption(applied_steps: usize) {
    if std::env::var("BAHAMUT_UPDATE_HELPER_TEST_CRASH_AFTER")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(applied_steps)
    {
        std::process::exit(86);
    }
}

#[cfg(not(debug_assertions))]
pub(super) fn maybe_test_interruption(_applied_steps: usize) {}
