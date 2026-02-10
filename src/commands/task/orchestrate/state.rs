#![allow(dead_code)]
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::shared::error::{RalphError, Result};

/// Persistent state of an orchestration session.
#[derive(Debug, Serialize, Deserialize)]
pub struct OrchestrateState {
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub workers_count: u32,
    pub tasks: HashMap<String, TaskState>,
    pub dag: HashMap<String, Vec<String>>,
}

/// Per-task state within an orchestration session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub status: String, // "pending", "in_progress", "done", "blocked"
    pub worker: Option<u32>,
    pub retries: u32,
    pub cost: f64,
}

impl OrchestrateState {
    /// Create a new state for a fresh session.
    pub fn new(workers_count: u32, dag: HashMap<String, Vec<String>>) -> Self {
        Self {
            session_id: uuid_v4(),
            started_at: Utc::now(),
            workers_count,
            tasks: HashMap::new(),
            dag,
        }
    }

    /// Save state atomically: write to .tmp file, then rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp_path = path.with_extension("yaml.tmp");
        let content = serde_yaml::to_string(self)
            .map_err(|e| RalphError::SessionResume(format!("Failed to serialize state: {e}")))?;
        std::fs::write(&tmp_path, content)?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// Load state from a YAML file.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| RalphError::SessionResume(format!("Failed to read state: {e}")))?;
        serde_yaml::from_str(&content)
            .map_err(|e| RalphError::SessionResume(format!("Failed to parse state: {e}")))
    }
}

/// Lockfile for exclusive access to orchestration.
///
/// Contains PID and heartbeat timestamp. A lock is considered stale
/// if the heartbeat is older than 10 seconds or the PID is not alive.
#[derive(Debug, Serialize, Deserialize)]
pub struct Lockfile {
    pub pid: u32,
    pub heartbeat: DateTime<Utc>,
    path: PathBuf,
}

impl Lockfile {
    /// Acquire a lockfile. Fails if another active session holds it.
    pub fn acquire(path: &Path) -> Result<Self> {
        // Check for existing lock
        if path.exists() {
            if !Self::is_stale(path) {
                let content = std::fs::read_to_string(path).unwrap_or_default();
                return Err(RalphError::LockfileHeld(format!(
                    "Lock held by: {content}"
                )));
            }
            // Stale lock — remove it
            std::fs::remove_file(path).ok();
        }

        let lock = Self {
            pid: std::process::id(),
            heartbeat: Utc::now(),
            path: path.to_path_buf(),
        };
        lock.write()?;
        Ok(lock)
    }

    /// Update the heartbeat timestamp.
    pub fn heartbeat(&mut self) -> Result<()> {
        self.heartbeat = Utc::now();
        self.write()
    }

    /// Release the lockfile by deleting it.
    pub fn release(self) -> Result<()> {
        if self.path.exists() {
            std::fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    /// Check if a lockfile is stale (heartbeat >10s old or PID not alive).
    pub fn is_stale(path: &Path) -> bool {
        let Ok(content) = std::fs::read_to_string(path) else {
            return true;
        };
        let Ok(lock) = serde_yaml::from_str::<LockfileData>(&content) else {
            return true;
        };

        // Check heartbeat age (>10 seconds = stale)
        let age = Utc::now() - lock.heartbeat;
        if age.num_seconds() > 10 {
            return true;
        }

        // Check if PID is alive
        !is_pid_alive(lock.pid)
    }

    fn write(&self) -> Result<()> {
        let data = LockfileData {
            pid: self.pid,
            heartbeat: self.heartbeat,
        };
        let content = serde_yaml::to_string(&data)
            .map_err(|e| RalphError::Orchestrate(format!("Failed to serialize lock: {e}")))?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }
}

/// Internal lockfile data (without path, for serialization).
#[derive(Debug, Serialize, Deserialize)]
struct LockfileData {
    pid: u32,
    heartbeat: DateTime<Utc>,
}

/// Check if a process with the given PID is alive.
fn is_pid_alive(pid: u32) -> bool {
    // On Unix, kill(pid, 0) checks if process exists without sending a signal
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On non-Unix, assume alive (conservative)
        true
    }
}

/// Generate a simple UUID v4 without external dependency.
fn uuid_v4() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        now.as_secs() as u32,
        (now.subsec_nanos() >> 16) & 0xFFFF,
        now.subsec_nanos() & 0xFFF,
        (pid & 0xFFFF) | 0x8000,
        now.as_nanos() as u64 & 0xFFFFFFFFFFFF,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_new() {
        let state = OrchestrateState::new(3, HashMap::new());
        assert_eq!(state.workers_count, 3);
        assert!(state.tasks.is_empty());
        assert!(!state.session_id.is_empty());
    }

    #[test]
    fn test_state_yaml_roundtrip() {
        let mut state = OrchestrateState::new(2, HashMap::new());
        state.tasks.insert(
            "T01".to_string(),
            TaskState {
                status: "done".to_string(),
                worker: Some(1),
                retries: 0,
                cost: 0.042,
            },
        );
        state.dag.insert(
            "T02".to_string(),
            vec!["T01".to_string()],
        );

        let dir = std::env::temp_dir().join("ralph-test-state");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test-state.yaml");

        state.save(&path).unwrap();
        let loaded = OrchestrateState::load(&path).unwrap();

        assert_eq!(loaded.workers_count, 2);
        assert_eq!(loaded.tasks["T01"].status, "done");
        assert_eq!(loaded.tasks["T01"].cost, 0.042);
        assert_eq!(loaded.dag["T02"], vec!["T01"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_lockfile_acquire_release() {
        let dir = std::env::temp_dir().join("ralph-test-lock");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.lock");

        // Clean up any leftover lock
        std::fs::remove_file(&path).ok();

        let lock = Lockfile::acquire(&path).unwrap();
        assert!(path.exists());
        assert_eq!(lock.pid, std::process::id());

        lock.release().unwrap();
        assert!(!path.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_lockfile_stale_detection() {
        let dir = std::env::temp_dir().join("ralph-test-stale-lock");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stale.lock");

        // Write a lock with old heartbeat and non-existent PID
        let data = LockfileData {
            pid: 99999999, // Very unlikely to be alive
            heartbeat: Utc::now() - chrono::Duration::seconds(30),
        };
        let content = serde_yaml::to_string(&data).unwrap();
        std::fs::write(&path, content).unwrap();

        assert!(Lockfile::is_stale(&path));

        // Should be able to acquire over stale lock
        let lock = Lockfile::acquire(&path).unwrap();
        lock.release().unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_uuid_v4_format() {
        let id = uuid_v4();
        assert!(id.contains('-'));
        assert!(id.contains('4')); // version 4
        // Basic length check
        assert!(id.len() > 30);
    }

    #[test]
    fn test_task_state_serialization() {
        let state = TaskState {
            status: "in_progress".to_string(),
            worker: Some(2),
            retries: 1,
            cost: 0.018,
        };
        let yaml = serde_yaml::to_string(&state).unwrap();
        assert!(yaml.contains("in_progress"));
        assert!(yaml.contains("0.018"));

        let parsed: TaskState = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.status, "in_progress");
        assert_eq!(parsed.worker, Some(2));
    }
}
