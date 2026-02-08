pub mod executable_manager;
pub mod github_release;
pub mod platform_detector;
pub mod self_updater;
pub mod version_checker;

pub use self_updater::update_self;
pub use version_checker::VersionChecker;
