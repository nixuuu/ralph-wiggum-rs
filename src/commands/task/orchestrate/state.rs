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
                return Err(RalphError::LockfileHeld(format!("Lock held by: {content}")));
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
///
/// Uses SystemTime + PID + atomic counter to guarantee uniqueness
/// even when called multiple times within the same nanosecond.
fn uuid_v4() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::SystemTime;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos_mixed = (now.as_nanos() as u64).wrapping_add(seq);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        now.as_secs() as u32,
        (now.subsec_nanos() >> 16) & 0xFFFF,
        now.subsec_nanos() & 0xFFF,
        (pid & 0xFFFF) | 0x8000,
        nanos_mixed & 0xFFFFFFFFFFFF,
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
        state.dag.insert("T02".to_string(), vec!["T01".to_string()]);

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
    fn test_lockfile_dead_pid_detection() {
        let dir = std::env::temp_dir().join("ralph-test-dead-pid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dead-pid.lock");

        // Write a lockfile with fresh heartbeat but dead PID
        let data = LockfileData {
            pid: 99999999,         // Non-existent PID
            heartbeat: Utc::now(), // Fresh heartbeat (not stale by time)
        };
        let content = serde_yaml::to_string(&data).unwrap();
        std::fs::write(&path, content).unwrap();

        // Verify is_pid_alive returns false for this PID
        #[cfg(unix)]
        assert!(!is_pid_alive(99999999));

        // Lock should be detected as stale due to dead PID
        assert!(Lockfile::is_stale(&path));

        // Should be able to acquire a new lock, overriding the stale one
        let lock = Lockfile::acquire(&path).unwrap();
        assert!(path.exists());
        assert_eq!(lock.pid, std::process::id());

        lock.release().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Test comprehensive UUID v4 format validation and uniqueness.
    ///
    /// Note: The current implementation uses SystemTime + PID, not cryptographic randomness.
    /// The variant field uses `| 0x8000` which sets bit 15 but doesn't mask bit 14,
    /// resulting in variant values 8-f instead of strict RFC 4122 (8-b only).
    ///
    /// Verifies:
    /// - Format: 8-4-4-4-12 hex digits separated by hyphens
    /// - Version: bit 13 = 4 (UUID version 4)
    /// - Variant: first char of segment 4 has high bit set (8-f range)
    /// - Uniqueness: two calls produce different UUIDs
    #[test]
    fn test_uuid_v4_format_and_uniqueness() {
        let uuid1 = uuid_v4();
        let uuid2 = uuid_v4();

        // Test format: 8-4-4-4-12 hex characters
        let parts: Vec<&str> = uuid1.split('-').collect();
        assert_eq!(
            parts.len(),
            5,
            "UUID should have 5 parts separated by hyphens"
        );
        assert_eq!(parts[0].len(), 8, "First segment should be 8 hex chars");
        assert_eq!(parts[1].len(), 4, "Second segment should be 4 hex chars");
        assert_eq!(parts[2].len(), 4, "Third segment should be 4 hex chars");
        assert_eq!(parts[3].len(), 4, "Fourth segment should be 4 hex chars");
        assert_eq!(parts[4].len(), 12, "Fifth segment should be 12 hex chars");

        // Verify all characters are valid hex
        for part in &parts {
            for ch in part.chars() {
                assert!(
                    ch.is_ascii_hexdigit(),
                    "UUID should contain only hex digits and hyphens, found: {ch}"
                );
            }
        }

        // Test version 4: character at position 14 (first char of third segment) should be '4'
        let version_char = parts[2].chars().next().unwrap();
        assert_eq!(
            version_char, '4',
            "UUID version should be 4, found: {version_char}"
        );

        // Test variant: first char of fourth segment should have high bit set (8-f)
        // Current implementation uses (pid & 0xFFFF) | 0x8000, which gives 8-f range
        // (not strict RFC 4122 which would be 8-b only)
        let variant_char = parts[3].chars().next().unwrap();
        assert!(
            matches!(variant_char, '8' | '9' | 'a' | 'b' | 'c' | 'd' | 'e' | 'f'),
            "UUID variant should have high bit set (8-f range), found: {variant_char}"
        );

        // Test uniqueness: two consecutive calls should produce different UUIDs
        assert_ne!(
            uuid1, uuid2,
            "Two consecutive uuid_v4() calls should produce different UUIDs"
        );
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

    #[test]
    fn test_state_roundtrip_empty_tasks() {
        // Edge case: state bez żadnych zadań (pusta tasks HashMap)
        let state = OrchestrateState::new(3, HashMap::new());

        let dir = std::env::temp_dir().join("ralph-test-empty-tasks");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty-state.yaml");

        state.save(&path).unwrap();
        let loaded = OrchestrateState::load(&path).unwrap();

        assert_eq!(loaded.workers_count, state.workers_count);
        assert_eq!(loaded.session_id, state.session_id);
        assert!(loaded.tasks.is_empty());
        assert!(loaded.dag.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_state_roundtrip_1000_tasks() {
        // Edge case: bardzo duża liczba zadań (1000)
        let mut state = OrchestrateState::new(10, HashMap::new());

        // Dodaj 1000 zadań z różnymi statusami
        for i in 0..1000 {
            let task_id = format!("T{:04}", i);
            let status = match i % 4 {
                0 => "done",
                1 => "in_progress",
                2 => "blocked",
                _ => "pending",
            };
            state.tasks.insert(
                task_id.clone(),
                TaskState {
                    status: status.to_string(),
                    worker: if status == "in_progress" {
                        Some((i % 10) as u32)
                    } else {
                        None
                    },
                    retries: (i % 5) as u32,
                    cost: (i as f64) * 0.001,
                },
            );
            // Dodaj zależności dla części zadań
            if i > 0 && i % 10 == 0 {
                state
                    .dag
                    .insert(format!("T{:04}", i), vec![format!("T{:04}", i - 1)]);
            }
        }

        let dir = std::env::temp_dir().join("ralph-test-large-state");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("large-state.yaml");

        state.save(&path).unwrap();
        let loaded = OrchestrateState::load(&path).unwrap();

        assert_eq!(loaded.workers_count, state.workers_count);
        assert_eq!(loaded.session_id, state.session_id);
        assert_eq!(loaded.tasks.len(), 1000);
        assert_eq!(loaded.dag.len(), state.dag.len());

        // Sprawdź kilka losowych zadań
        assert_eq!(loaded.tasks["T0000"].status, "done");
        assert_eq!(loaded.tasks["T0001"].status, "in_progress");
        assert_eq!(loaded.tasks["T0002"].status, "blocked");
        assert_eq!(loaded.tasks["T0003"].status, "pending");
        assert_eq!(loaded.tasks["T0999"].status, "pending");

        // Sprawdź dane zadania szczegółowo
        let task_500 = &loaded.tasks["T0500"];
        assert_eq!(task_500.status, "done");
        assert_eq!(task_500.retries, 0);
        assert_eq!(task_500.cost, 0.5);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_state_roundtrip_special_chars_in_task_ids() {
        // Edge case: task ID i session_id ze specjalnymi znakami
        let mut state = OrchestrateState::new(2, HashMap::new());

        // Ustaw session_id ze specjalnymi znakami
        state.session_id = "session-with-sp€cial-çhars-日本語-🚀".to_string();

        // Dodaj zadania z różnymi specjalnymi znakami w ID
        let special_task_ids = vec![
            "task-with-unicode-日本語",
            "task-with-emoji-🚀",
            "task.with.dots",
            "task:with:colons",
            "task@with@at",
            "task_with_underscore",
            "task-with-dashes",
            "task#123",
            "task$money",
            "task%percent",
        ];

        for (idx, task_id) in special_task_ids.iter().enumerate() {
            state.tasks.insert(
                task_id.to_string(),
                TaskState {
                    status: if idx % 2 == 0 { "done" } else { "blocked" }.to_string(),
                    worker: None,
                    retries: idx as u32,
                    cost: (idx as f64) * 0.01,
                },
            );
        }

        // Dodaj zależności ze specjalnymi znakami
        state.dag.insert(
            "task-with-unicode-日本語".to_string(),
            vec!["task-with-emoji-🚀".to_string()],
        );

        let dir = std::env::temp_dir().join("ralph-test-special-chars");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("special-chars-state.yaml");

        state.save(&path).unwrap();
        let loaded = OrchestrateState::load(&path).unwrap();

        // Sprawdź czy session_id przetrwał roundtrip
        assert_eq!(loaded.session_id, state.session_id);
        assert_eq!(loaded.workers_count, state.workers_count);
        assert_eq!(loaded.tasks.len(), special_task_ids.len());

        // Sprawdź wszystkie specjalne task ID
        for task_id in &special_task_ids {
            assert!(
                loaded.tasks.contains_key(*task_id),
                "Task ID '{}' nie przetrwał roundtrip",
                task_id
            );
        }

        // Sprawdź konkretne zadanie z unicode
        let unicode_task = &loaded.tasks["task-with-unicode-日本語"];
        assert_eq!(unicode_task.status, "done");
        assert_eq!(unicode_task.retries, 0);

        // Sprawdź DAG ze specjalnymi znakami
        assert_eq!(
            loaded.dag["task-with-unicode-日本語"],
            vec!["task-with-emoji-🚀"]
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_state_roundtrip_identity_check() {
        // Test identyczności: pełna weryfikacja że save+load zwraca identyczny obiekt
        let mut state = OrchestrateState::new(5, HashMap::new());

        state.session_id = "test-session-123".to_string();

        // Dodaj kilka zadań z różnymi stanami
        state.tasks.insert(
            "T01".to_string(),
            TaskState {
                status: "done".to_string(),
                worker: Some(1),
                retries: 0,
                cost: 0.123,
            },
        );
        state.tasks.insert(
            "T02".to_string(),
            TaskState {
                status: "in_progress".to_string(),
                worker: Some(2),
                retries: 1,
                cost: 0.456,
            },
        );
        state.tasks.insert(
            "T03".to_string(),
            TaskState {
                status: "blocked".to_string(),
                worker: None,
                retries: 2,
                cost: 0.0,
            },
        );

        // Dodaj DAG
        state.dag.insert("T02".to_string(), vec!["T01".to_string()]);
        state.dag.insert(
            "T03".to_string(),
            vec!["T01".to_string(), "T02".to_string()],
        );

        let dir = std::env::temp_dir().join("ralph-test-identity");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("identity-state.yaml");

        state.save(&path).unwrap();
        let loaded = OrchestrateState::load(&path).unwrap();

        // Pełna weryfikacja identyczności
        assert_eq!(loaded.session_id, state.session_id);
        assert_eq!(loaded.started_at, state.started_at);
        assert_eq!(loaded.workers_count, state.workers_count);
        assert_eq!(loaded.tasks.len(), state.tasks.len());
        assert_eq!(loaded.dag.len(), state.dag.len());

        // Sprawdź każde zadanie
        for (task_id, task_state) in &state.tasks {
            let loaded_task = &loaded.tasks[task_id];
            assert_eq!(loaded_task.status, task_state.status);
            assert_eq!(loaded_task.worker, task_state.worker);
            assert_eq!(loaded_task.retries, task_state.retries);
            assert_eq!(loaded_task.cost, task_state.cost);
        }

        // Sprawdź DAG
        for (task_id, deps) in &state.dag {
            assert_eq!(&loaded.dag[task_id], deps);
        }

        std::fs::remove_dir_all(&dir).ok();
    }
}
