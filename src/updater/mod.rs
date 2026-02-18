pub mod executable_manager;
pub mod github_release;
pub mod platform_detector;
pub mod self_updater;
pub mod version_checker;

#[allow(unused_imports)]
// Legacy: używane przez StatusTerminal (inline mode), nie fullscreen TUI
pub use self_updater::{update_in_background, update_self};
#[allow(unused_imports)]
// Legacy: używane przez StatusTerminal (inline mode), nie fullscreen TUI
pub use version_checker::{UpdateState, VersionChecker};
