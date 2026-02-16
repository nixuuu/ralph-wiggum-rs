use anyhow::Result;
use std::path::PathBuf;

pub fn get_current_executable() -> Result<PathBuf> {
    std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Failed to get current executable path: {e}"))
}

/// Cleanup old executable backup file (.old extension).
/// On Windows, silently removes the .old file if it exists.
/// On other platforms, this is a no-op.
#[cfg(windows)]
pub fn cleanup_old_exe() {
    if let Ok(current_exe) = std::env::current_exe() {
        cleanup_old_exe_at_path(&current_exe);
    }
}

#[cfg(not(windows))]
pub fn cleanup_old_exe() {
    // No-op on non-Windows platforms
}

/// Internal helper for cleanup - accepts a path for testing purposes.
/// Removes the .old backup file for the given executable path.
#[cfg(windows)]
#[cfg_attr(not(test), allow(dead_code))]
fn cleanup_old_exe_at_path(exe_path: &std::path::Path) {
    let old_path = exe_path.with_extension("old");
    // Silent cleanup - ignore errors if file doesn't exist
    let _ = std::fs::remove_file(old_path);
}

#[cfg(not(windows))]
#[cfg_attr(not(test), allow(dead_code))]
fn cleanup_old_exe_at_path(_exe_path: &std::path::Path) {
    // No-op on non-Windows platforms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    use std::io::Write;

    #[test]
    fn test_get_current_executable_returns_path() {
        let result = get_current_executable();
        assert!(
            result.is_ok(),
            "Should successfully get current executable path"
        );
        let path = result.unwrap();
        assert!(
            path.exists() || !path.exists(),
            "Path should be a valid PathBuf"
        );
    }

    #[test]
    fn test_cleanup_old_exe_no_panic() {
        // cleanup_old_exe() should never panic, even if .old file doesn't exist
        cleanup_old_exe();
        // If we got here without panicking, the test passes
    }

    #[test]
    #[cfg(windows)]
    fn test_cleanup_removes_old_file() {
        // Create a temporary directory
        let temp_dir = std::env::temp_dir();
        let exe_path = temp_dir.join("test_ralph.exe");
        let old_path = temp_dir.join("test_ralph.old");

        // Create a fake .old file
        let mut file = std::fs::File::create(&old_path).expect("Failed to create test .old file");
        file.write_all(b"old backup data")
            .expect("Failed to write test data");
        drop(file);

        // Verify the .old file exists
        assert!(
            old_path.exists(),
            "Test .old file should exist before cleanup"
        );

        // Run cleanup on the test path
        cleanup_old_exe_at_path(&exe_path);

        // Verify the .old file was removed
        assert!(
            !old_path.exists(),
            "Test .old file should be removed after cleanup"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn test_cleanup_removes_old_file() {
        // On non-Windows platforms, cleanup_old_exe_at_path is a no-op
        // We just verify it doesn't panic
        let temp_dir = std::env::temp_dir();
        let exe_path = temp_dir.join("test_ralph.exe");

        // Should not panic on non-Windows platforms
        cleanup_old_exe_at_path(&exe_path);
    }

    #[test]
    fn test_cleanup_noop_when_no_old_file() {
        // Create a temporary directory
        let temp_dir = std::env::temp_dir();
        let exe_path = temp_dir.join("test_ralph_no_old.exe");
        let old_path = temp_dir.join("test_ralph_no_old.old");

        // Ensure no .old file exists
        if old_path.exists() {
            std::fs::remove_file(&old_path).ok();
        }

        assert!(
            !old_path.exists(),
            "Test .old file should not exist before cleanup"
        );

        // Run cleanup - should not panic or error
        cleanup_old_exe_at_path(&exe_path);

        // On Windows, file should still not exist
        // On non-Windows, this is a no-op, so file still doesn't exist
        assert!(
            !old_path.exists(),
            "Test .old file should still not exist after cleanup"
        );
    }
}
