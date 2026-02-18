// Library interface for ralph-wiggum
// Exposes shared module for integration tests (tests/ directory)

pub mod shared;
pub mod tui;

// Required by lib-scoped tests in shared/ that reference commands::task::orchestrate::verify.
// commands depends on cli, templates, and updater — all must be present.
#[cfg(test)]
pub mod cli;
#[cfg(test)]
pub mod commands;
#[cfg(test)]
pub mod templates;
#[cfg(test)]
pub mod test_helpers;
#[cfg(test)]
pub mod updater;
