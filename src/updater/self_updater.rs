use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::Result;

use super::executable_manager::get_current_executable;
use super::github_release::GitHubRelease;
use super::platform_detector::get_platform_target;
use super::version_checker::{UpdateState, is_newer};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_OWNER: &str = "nixuuu";
const REPO_NAME: &str = "ralph-wiggum-rs";
const BINARY_NAME: &str = "ralph-wiggum";

pub fn update_self() -> Result<()> {
    println!("Checking for updates...");
    println!("Current version: v{CURRENT_VERSION}");

    let client = reqwest::blocking::Client::new();
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");

    let response = client
        .get(&url)
        .header("User-Agent", format!("{BINARY_NAME}/{CURRENT_VERSION}"))
        .send()?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "GitHub API returned status: {}",
            response.status()
        ));
    }

    let release: GitHubRelease = response.json()?;
    println!("Latest version: {}", release.tag_name);

    if !is_newer(CURRENT_VERSION, &release.tag_name)? {
        println!("You're already running the latest version!");
        return Ok(());
    }

    println!("New version available: {}", release.tag_name);

    let target = get_platform_target()?;
    let asset_name = if cfg!(windows) {
        format!("{BINARY_NAME}-{target}.exe")
    } else {
        format!("{BINARY_NAME}-{target}")
    };

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow::anyhow!("No binary found for platform: {target}"))?;

    println!("Downloading {}...", asset.name);
    let binary_data = client.get(&asset.browser_download_url).send()?.bytes()?;

    let current_exe = get_current_executable()?;
    let backup_path = current_exe.with_extension("bak");

    println!("Creating backup at {}...", backup_path.display());
    std::fs::copy(&current_exe, &backup_path)?;

    println!("Installing update...");
    let temp_exe = current_exe.with_extension("tmp");
    std::fs::write(&temp_exe, &binary_data)?;
    std::fs::rename(&temp_exe, &current_exe)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&current_exe)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&current_exe, perms)?;
    }

    println!("Successfully updated to {}!", release.tag_name);
    println!("Backup saved to: {}", backup_path.display());
    Ok(())
}

/// Perform update in background (no stdout output, TUI-safe).
/// Sets update_state to Completed or Failed when done.
/// Designed for tokio::task::spawn_blocking.
pub fn update_in_background(update_state: Arc<AtomicU8>) {
    match do_background_update() {
        Ok(()) => {
            update_state.store(UpdateState::Completed as u8, Ordering::SeqCst);
        }
        Err(_) => {
            update_state.store(UpdateState::Failed as u8, Ordering::SeqCst);
        }
    }
}

fn do_background_update() -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");

    let response = client
        .get(&url)
        .header("User-Agent", format!("{BINARY_NAME}/{CURRENT_VERSION}"))
        .send()?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "GitHub API returned status: {}",
            response.status()
        ));
    }

    let release: GitHubRelease = response.json()?;

    if !is_newer(CURRENT_VERSION, &release.tag_name)? {
        return Ok(());
    }

    let target = get_platform_target()?;
    let asset_name = if cfg!(windows) {
        format!("{BINARY_NAME}-{target}.exe")
    } else {
        format!("{BINARY_NAME}-{target}")
    };

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow::anyhow!("No binary found for platform: {target}"))?;

    let binary_data = client.get(&asset.browser_download_url).send()?.bytes()?;

    let current_exe = get_current_executable()?;
    let backup_path = current_exe.with_extension("bak");
    std::fs::copy(&current_exe, &backup_path)?;

    let temp_exe = current_exe.with_extension("tmp");
    std::fs::write(&temp_exe, &binary_data)?;
    std::fs::rename(&temp_exe, &current_exe)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&current_exe)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&current_exe, perms)?;
    }

    Ok(())
}
