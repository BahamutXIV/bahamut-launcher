//! Extension discovery, configuration, and launch planning.

pub mod addons;
pub mod helper;
pub mod identity;
pub mod launch_plan;
pub mod layout;
pub mod overlay_packages;

pub use addons::{
    ADDON_MANIFEST_FILE_NAME, AddonCommand, AddonDiscoveryError, AddonPackage, discover_addons,
    select_addon_manifests,
};
pub use helper::{
    CLIENT_MODULE_NAME, HelperErrorCode, HelperFailure, HelperPlanError, HelperSuccess,
    LOADER_BINARY_NAME, parse_helper_error, parse_helper_output, parse_wait_mode_line,
};
pub use identity::{
    CLIENT_EXECUTABLE_NAME, ClientIdentity, GAME_VERSION_FILE_NAME, IdentityGateError,
    IdentityMismatch, IdentityReadError, SUPPORTED_CLIENT_BYTE_LENGTH, SUPPORTED_CLIENT_SHA256,
    SUPPORTED_GAME_VERSION, gate_install, verify_supported_client,
};
pub use launch_plan::{
    ADDON_CATALOG_ENVIRONMENT, ADDON_MANIFESTS_ENVIRONMENT, ADDON_SETTINGS_ENVIRONMENT, ARG_CLIENT,
    ARG_LAUNCH_ARGUMENT, ARG_LOBBY_HOST, ARG_MODULE, ARG_PATCH, ARG_SERVER_UTC, ARG_TIMEOUT_MS,
    ARG_WAIT_FOR_CLIENT, CHAT_LOGS_ENVIRONMENT, DAT_PACKAGE_ROOTS_ENVIRONMENT,
    ExtensionLaunchRequest, GuestPathMapper, HelperInvocation, LaunchPlanError,
    SCREENSHOT_ENABLED_ENVIRONMENT, SCREENSHOT_FORMAT_ENVIRONMENT,
    SCREENSHOT_HIDE_OVERLAYS_ENVIRONMENT, SCREENSHOT_HOTKEY_ENVIRONMENT, SCREENSHOTS_ENVIRONMENT,
    STARTUP_SCRIPT_ENVIRONMENT, encode_patch_argument, plan_extension_launch,
    plan_wine_extension_launch,
};
pub use layout::ExtensionLayout;
pub use overlay_packages::{
    OVERLAY_MANIFEST_FILE_NAME, OverlayConflict, OverlayDiscoveryError, OverlayPackage,
    OverlaySelection, discover_overlay_packages, select_overlay_packages,
};
