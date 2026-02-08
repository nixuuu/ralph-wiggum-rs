use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use super::github_release::GitHubRelease;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_OWNER: &str = "nixuuu";
const REPO_NAME: &str = "ralph-wiggum-rs";
const CHECK_INTERVAL_SECS: u64 = 300;

/// Update information shared with the status bar
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub update_available: bool,
}

/// Compare two semver strings. Returns true if latest > current.
pub fn is_newer(current: &str, latest: &str) -> Result<bool> {
    let current_clean = current.trim_start_matches('v');
    let latest_clean = latest.trim_start_matches('v');

    let parse = |v: &str| -> Result<Vec<u32>> {
        v.split('.')
            .map(|p| {
                p.parse::<u32>()
                    .map_err(|e| anyhow::anyhow!("Invalid version component '{p}': {e}"))
            })
            .collect()
    };

    let curr = parse(current_clean)?;
    let lat = parse(latest_clean)?;

    for (c, l) in curr.iter().zip(lat.iter()) {
        if l > c {
            return Ok(true);
        }
        if c > l {
            return Ok(false);
        }
    }
    Ok(lat.len() > curr.len())
}

/// Manages background version checking
pub struct VersionChecker {
    update_info: Arc<Mutex<Option<UpdateInfo>>>,
}

impl VersionChecker {
    pub fn new() -> Self {
        Self {
            update_info: Arc::new(Mutex::new(None)),
        }
    }

    /// Get the shared state handle for the status bar to read
    pub fn update_info(&self) -> Arc<Mutex<Option<UpdateInfo>>> {
        self.update_info.clone()
    }

    /// Spawn a background task that checks for updates at startup
    /// and then periodically every CHECK_INTERVAL_SECS
    pub fn spawn_checker(&self, shutdown: Arc<AtomicBool>) {
        let info = self.update_info.clone();
        tokio::spawn(async move {
            // Initial check at startup
            if let Ok(result) = check_latest_version().await {
                *info.lock().unwrap() = Some(result);
            }

            // Periodic checks
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS)).await;
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(result) = check_latest_version().await {
                    *info.lock().unwrap() = Some(result);
                }
            }
        });
    }
}

/// Async version check against GitHub releases API
async fn check_latest_version() -> Result<UpdateInfo> {
    let client = reqwest::Client::new();
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");

    let response = client
        .get(&url)
        .header("User-Agent", format!("ralph-wiggum/{CURRENT_VERSION}"))
        .send()
        .await?;

    let release: GitHubRelease = response.json().await?;
    let update_available = is_newer(CURRENT_VERSION, &release.tag_name)?;

    Ok(UpdateInfo {
        latest_version: release.tag_name,
        update_available,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_basic() {
        assert!(is_newer("0.1.0", "0.2.0").unwrap());
        assert!(is_newer("0.1.0", "1.0.0").unwrap());
        assert!(is_newer("1.0.0", "1.0.1").unwrap());
    }

    #[test]
    fn test_is_newer_equal() {
        assert!(!is_newer("1.0.0", "1.0.0").unwrap());
        assert!(!is_newer("0.1.0", "0.1.0").unwrap());
    }

    #[test]
    fn test_is_newer_older() {
        assert!(!is_newer("1.0.0", "0.9.0").unwrap());
        assert!(!is_newer("2.0.0", "1.9.9").unwrap());
    }

    #[test]
    fn test_is_newer_with_v_prefix() {
        assert!(is_newer("v0.1.0", "v0.2.0").unwrap());
        assert!(is_newer("0.1.0", "v0.2.0").unwrap());
        assert!(is_newer("v0.1.0", "0.2.0").unwrap());
    }

    #[test]
    fn test_is_newer_different_lengths() {
        assert!(is_newer("1.0", "1.0.1").unwrap());
        assert!(!is_newer("1.0.1", "1.0").unwrap());
    }
}
