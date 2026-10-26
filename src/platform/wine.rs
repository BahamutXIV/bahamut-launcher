//! Wine backends. The plain launch patches a working copy because the native launcher's
//! `WriteProcessMemory` cannot cross the Wine boundary (see `crate::launcher::pe_patch`); the
//! extension launch runs the x86 loader inside Wine, which patches the original client in memory.

#[cfg(target_os = "linux")]
pub use system::*;

#[cfg(target_os = "macos")]
pub use bundled::*;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod log_tail;

/// Linux launch backend: run the client under `BAHAMUT_WINE`, else the managed Wine engine on
/// x86_64 hosts, else the system Wine on PATH.
#[cfg(target_os = "linux")]
mod system;

/// macOS launch backend: run the client under a bundled Sikarugir Wine engine.
#[cfg(target_os = "macos")]
mod bundled;

/// Extension launch shared by the Wine backends: DOS path mapping and the wait-mode helper session.
pub mod extension_launch;
