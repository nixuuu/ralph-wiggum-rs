use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::shared::error::Result;

/// Phase of worker execution lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerPhase {
    Setup,
    Implement,
    ReviewFix,
    Verify,
}

impl std::fmt::Display for WorkerPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerPhase::Setup => write!(f, "setup"),
            WorkerPhase::Implement => write!(f, "implement"),
            WorkerPhase::ReviewFix => write!(f, "review+fix"),
            WorkerPhase::Verify => write!(f, "verify"),
        }
    }
}

/// Event sent from a worker to the orchestrator via mpsc channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerEvent {
    pub timestamp: DateTime<Utc>,
    #[serde(flatten)]
    pub kind: WorkerEventKind,
}

/// Profile verification status — name + optional success status.
/// Used during Verify phase to track individual profile execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileStatus {
    pub name: String,
    /// None = in progress, Some(true) = success, Some(false) = failed
    pub success: Option<bool>,
}

/// Specific event types for worker→orchestrator communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum WorkerEventKind {
    TaskStarted {
        worker_id: u32,
        task_id: String,
    },
    PhaseStarted {
        worker_id: u32,
        task_id: String,
        phase: WorkerPhase,
        /// Optional profile information for Verify phase
        #[serde(skip_serializing_if = "Option::is_none")]
        profiles: Option<Vec<String>>,
    },
    PhaseCompleted {
        worker_id: u32,
        task_id: String,
        phase: WorkerPhase,
        success: bool,
        /// Optional profile results for Verify phase
        #[serde(skip_serializing_if = "Option::is_none")]
        profile_results: Option<Vec<ProfileStatus>>,
    },
    TaskCompleted {
        worker_id: u32,
        task_id: String,
        success: bool,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
        files_changed: Vec<String>,
        commit_hash: Option<String>,
    },
    TaskFailed {
        worker_id: u32,
        task_id: String,
        error: String,
        retries_left: u32,
    },
    CostUpdate {
        worker_id: u32,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
    },
    MergeStarted {
        worker_id: u32,
        task_id: String,
    },
    MergeCompleted {
        worker_id: u32,
        task_id: String,
        success: bool,
        commit_hash: Option<String>,
    },
    MergeConflict {
        worker_id: u32,
        task_id: String,
        conflicting_files: Vec<String>,
    },
    /// Batch of formatted output lines from a worker's Claude session.
    OutputLines {
        worker_id: u32,
        lines: Vec<String>,
    },
    /// Output lines from a merge step (git commands, AI conflict resolution).
    MergeStepOutput {
        worker_id: u32,
        lines: Vec<String>,
    },
    /// Periodic heartbeat signal from a worker to indicate it's alive and working.
    Heartbeat {
        worker_id: u32,
        phase: WorkerPhase,
    },
    /// User message sent to the worker's Claude session via stdin.
    UserMessageSent {
        worker_id: u32,
        message: String,
    },
    /// User message received by the worker and forwarded to Claude stdin.
    /// Used for logging incoming messages in the worker's output panel.
    UserMessageReceived {
        worker_id: u32,
        message: String,
    },
}

// Removed: OrchestratorCommand enum (task 2.4)
// Was only used in tests, not in production code. Reserved for future bidirectional
// orchestrator-worker communication but never implemented.

impl WorkerEvent {
    /// Create a new WorkerEvent with the current UTC timestamp.
    pub fn new(kind: WorkerEventKind) -> Self {
        Self {
            timestamp: Utc::now(),
            kind,
        }
    }

    /// Serialize to a single JSON line (JSONL format).
    pub fn to_jsonl(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Logger that writes worker events to JSONL files.
///
/// Maintains per-worker log files and an optional combined log file.
/// Each event is written as a single JSON line.
pub struct EventLogger {
    log_dir: PathBuf,
    combined_log: Option<std::fs::File>,
}

impl EventLogger {
    /// Create a new EventLogger writing to the given directory.
    ///
    /// Creates the directory if it doesn't exist.
    /// If `combined_path` is Some, also writes all events to that file.
    pub fn new(log_dir: PathBuf, combined_path: Option<&Path>) -> Result<Self> {
        std::fs::create_dir_all(&log_dir)?;

        let combined_log = combined_path
            .map(|p| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
            })
            .transpose()?;

        Ok(Self {
            log_dir,
            combined_log,
        })
    }

    /// Log a worker event to the per-worker JSONL file and combined log.
    pub fn log_event(&mut self, event: &WorkerEvent, worker_id: u32) -> Result<()> {
        let line = event.to_jsonl()?;

        // Write to per-worker log
        let worker_log_path = self.log_dir.join(format!("w{worker_id}.jsonl"));
        let mut worker_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(worker_log_path)?;
        writeln!(worker_file, "{line}")?;

        // Write to combined log if configured
        if let Some(combined) = &mut self.combined_log {
            writeln!(combined, "{line}")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_event_serialize_task_started() {
        let event = WorkerEvent::new(WorkerEventKind::TaskStarted {
            worker_id: 1,
            task_id: "T01".to_string(),
        });
        let json = event.to_jsonl().unwrap();
        assert!(json.contains("\"event\":\"TaskStarted\""));
        assert!(json.contains("\"worker_id\":1"));
        assert!(json.contains("\"task_id\":\"T01\""));
        assert!(json.contains("\"timestamp\""));
        // Must be single line
        assert!(!json.contains('\n'));
    }

    #[test]
    fn test_worker_event_serialize_phase_started() {
        let event = WorkerEvent::new(WorkerEventKind::PhaseStarted {
            worker_id: 2,
            task_id: "T03".to_string(),
            phase: WorkerPhase::Implement,
            profiles: None,
        });
        let json = event.to_jsonl().unwrap();
        assert!(json.contains("\"phase\":\"Implement\""));
    }

    #[test]
    fn test_worker_event_serialize_phase_started_with_profiles() {
        let event = WorkerEvent::new(WorkerEventKind::PhaseStarted {
            worker_id: 2,
            task_id: "T03".to_string(),
            phase: WorkerPhase::Verify,
            profiles: Some(vec!["frontend".to_string(), "backend".to_string()]),
        });
        let json = event.to_jsonl().unwrap();
        assert!(json.contains("\"phase\":\"Verify\""));
        assert!(json.contains("\"profiles\""));
        assert!(json.contains("frontend"));
        assert!(json.contains("backend"));
    }

    #[test]
    fn test_worker_event_serialize_phase_completed_with_profile_results() {
        let event = WorkerEvent::new(WorkerEventKind::PhaseCompleted {
            worker_id: 2,
            task_id: "T03".to_string(),
            phase: WorkerPhase::Verify,
            success: true,
            profile_results: Some(vec![
                ProfileStatus {
                    name: "frontend".to_string(),
                    success: Some(true),
                },
                ProfileStatus {
                    name: "backend".to_string(),
                    success: Some(false),
                },
            ]),
        });
        let json = event.to_jsonl().unwrap();
        assert!(json.contains("\"phase\":\"Verify\""));
        assert!(json.contains("\"profile_results\""));
        assert!(json.contains("frontend"));
        assert!(json.contains("backend"));
    }

    #[test]
    fn test_worker_event_serialize_task_completed() {
        let event = WorkerEvent::new(WorkerEventKind::TaskCompleted {
            worker_id: 1,
            task_id: "T01".to_string(),
            success: true,
            cost_usd: 0.042,
            input_tokens: 1200,
            output_tokens: 500,
            files_changed: vec!["src/main.rs".to_string()],
            commit_hash: Some("abc1234".to_string()),
        });
        let json = event.to_jsonl().unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"cost_usd\":0.042"));
    }

    #[test]
    fn test_worker_event_serialize_merge_conflict() {
        let event = WorkerEvent::new(WorkerEventKind::MergeConflict {
            worker_id: 1,
            task_id: "T02".to_string(),
            conflicting_files: vec!["src/lib.rs".to_string(), "Cargo.toml".to_string()],
        });
        let json = event.to_jsonl().unwrap();
        assert!(json.contains("\"event\":\"MergeConflict\""));
        assert!(json.contains("src/lib.rs"));
    }

    #[test]
    fn test_worker_event_roundtrip() {
        let original = WorkerEvent::new(WorkerEventKind::CostUpdate {
            worker_id: 3,
            cost_usd: 0.1,
            input_tokens: 5000,
            output_tokens: 2000,
        });
        let json = original.to_jsonl().unwrap();
        let deserialized: WorkerEvent = serde_json::from_str(&json).unwrap();
        // Verify key fields survived roundtrip
        if let WorkerEventKind::CostUpdate {
            worker_id,
            cost_usd,
            ..
        } = &deserialized.kind
        {
            assert_eq!(*worker_id, 3);
            assert!((cost_usd - 0.1).abs() < f64::EPSILON);
        } else {
            panic!("Wrong event type after roundtrip");
        }
    }

    // Removed: test_orchestrator_command_serialize and test_orchestrator_command_roundtrip (task 2.4)
    // Tests removed along with OrchestratorCommand enum

    #[test]
    fn test_worker_phase_display() {
        assert_eq!(WorkerPhase::Setup.to_string(), "setup");
        assert_eq!(WorkerPhase::Implement.to_string(), "implement");
        assert_eq!(WorkerPhase::ReviewFix.to_string(), "review+fix");
        assert_eq!(WorkerPhase::Verify.to_string(), "verify");
    }

    #[test]
    fn test_all_event_types_single_line() {
        let events = vec![
            WorkerEventKind::TaskStarted {
                worker_id: 1,
                task_id: "T01".to_string(),
            },
            WorkerEventKind::PhaseStarted {
                worker_id: 1,
                task_id: "T01".to_string(),
                phase: WorkerPhase::Implement,
                profiles: None,
            },
            WorkerEventKind::PhaseCompleted {
                worker_id: 1,
                task_id: "T01".to_string(),
                phase: WorkerPhase::ReviewFix,
                success: true,
                profile_results: None,
            },
            WorkerEventKind::TaskFailed {
                worker_id: 1,
                task_id: "T01".to_string(),
                error: "verify failed".to_string(),
                retries_left: 2,
            },
            WorkerEventKind::CostUpdate {
                worker_id: 1,
                cost_usd: 0.01,
                input_tokens: 100,
                output_tokens: 50,
            },
            WorkerEventKind::MergeStarted {
                worker_id: 1,
                task_id: "T01".to_string(),
            },
            WorkerEventKind::MergeCompleted {
                worker_id: 1,
                task_id: "T01".to_string(),
                success: true,
                commit_hash: Some("abc".to_string()),
            },
            WorkerEventKind::MergeConflict {
                worker_id: 1,
                task_id: "T01".to_string(),
                conflicting_files: vec![],
            },
            WorkerEventKind::OutputLines {
                worker_id: 1,
                lines: vec!["Hello world".to_string()],
            },
            WorkerEventKind::MergeStepOutput {
                worker_id: 1,
                lines: vec!["$ git merge --squash".to_string()],
            },
            WorkerEventKind::Heartbeat {
                worker_id: 1,
                phase: WorkerPhase::Implement,
            },
            WorkerEventKind::PhaseStarted {
                worker_id: 1,
                task_id: "T01".to_string(),
                phase: WorkerPhase::Verify,
                profiles: Some(vec!["frontend".to_string()]),
            },
            WorkerEventKind::PhaseCompleted {
                worker_id: 1,
                task_id: "T01".to_string(),
                phase: WorkerPhase::Verify,
                success: true,
                profile_results: Some(vec![ProfileStatus {
                    name: "frontend".to_string(),
                    success: Some(true),
                }]),
            },
            WorkerEventKind::UserMessageSent {
                worker_id: 1,
                message: "Hello from user".to_string(),
            },
            WorkerEventKind::UserMessageReceived {
                worker_id: 1,
                message: "Message received".to_string(),
            },
        ];

        for kind in events {
            let event = WorkerEvent::new(kind);
            let json = event.to_jsonl().unwrap();
            assert!(
                !json.contains('\n'),
                "Event JSON must be single line: {json}"
            );
        }
    }

    #[test]
    fn test_event_logger_writes_files() {
        let dir = std::env::temp_dir().join("ralph-test-events");
        let _ = std::fs::remove_dir_all(&dir);

        let combined_path = dir.join("combined.jsonl");
        let mut logger = EventLogger::new(dir.clone(), Some(&combined_path)).unwrap();

        let event = WorkerEvent::new(WorkerEventKind::TaskStarted {
            worker_id: 1,
            task_id: "T01".to_string(),
        });
        logger.log_event(&event, 1).unwrap();

        // Per-worker file
        let worker_log = std::fs::read_to_string(dir.join("w1.jsonl")).unwrap();
        assert!(worker_log.contains("TaskStarted"));
        assert!(worker_log.ends_with('\n'));

        // Combined file
        let combined = std::fs::read_to_string(&combined_path).unwrap();
        assert!(combined.contains("TaskStarted"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_worker_event_serialize_heartbeat() {
        let event = WorkerEvent::new(WorkerEventKind::Heartbeat {
            worker_id: 3,
            phase: WorkerPhase::ReviewFix,
        });
        let json = event.to_jsonl().unwrap();
        assert!(json.contains("\"event\":\"Heartbeat\""));
        assert!(json.contains("\"worker_id\":3"));
        assert!(json.contains("\"phase\":\"ReviewFix\""));
        assert!(json.contains("\"timestamp\""));
        // Must be single line
        assert!(!json.contains('\n'));
    }

    #[test]
    fn test_heartbeat_event_roundtrip() {
        let original = WorkerEvent::new(WorkerEventKind::Heartbeat {
            worker_id: 5,
            phase: WorkerPhase::Verify,
        });
        let json = original.to_jsonl().unwrap();
        let deserialized: WorkerEvent = serde_json::from_str(&json).unwrap();
        // Verify key fields survived roundtrip
        if let WorkerEventKind::Heartbeat { worker_id, phase } = &deserialized.kind {
            assert_eq!(*worker_id, 5);
            assert_eq!(*phase, WorkerPhase::Verify);
        } else {
            panic!("Wrong event type after roundtrip");
        }
    }

    #[test]
    fn test_worker_event_serialize_user_message_sent() {
        let event = WorkerEvent::new(WorkerEventKind::UserMessageSent {
            worker_id: 2,
            message: "Test message from user".to_string(),
        });
        let json = event.to_jsonl().unwrap();
        assert!(json.contains("\"event\":\"UserMessageSent\""));
        assert!(json.contains("\"worker_id\":2"));
        assert!(json.contains("\"message\":\"Test message from user\""));
        assert!(json.contains("\"timestamp\""));
        // Must be single line
        assert!(!json.contains('\n'));
    }

    #[test]
    fn test_user_message_sent_event_roundtrip() {
        let original = WorkerEvent::new(WorkerEventKind::UserMessageSent {
            worker_id: 7,
            message: "Roundtrip test message".to_string(),
        });
        let json = original.to_jsonl().unwrap();
        let deserialized: WorkerEvent = serde_json::from_str(&json).unwrap();
        // Verify key fields survived roundtrip
        if let WorkerEventKind::UserMessageSent { worker_id, message } = &deserialized.kind {
            assert_eq!(*worker_id, 7);
            assert_eq!(message, "Roundtrip test message");
        } else {
            panic!("Wrong event type after roundtrip");
        }
    }

    #[test]
    fn test_worker_event_serialize_user_message_received() {
        let event = WorkerEvent::new(WorkerEventKind::UserMessageReceived {
            worker_id: 3,
            message: "Received from orchestrator".to_string(),
        });
        let json = event.to_jsonl().unwrap();
        assert!(json.contains("\"event\":\"UserMessageReceived\""));
        assert!(json.contains("\"worker_id\":3"));
        assert!(json.contains("\"message\":\"Received from orchestrator\""));
        assert!(json.contains("\"timestamp\""));
        // Must be single line
        assert!(!json.contains('\n'));
    }

    #[test]
    fn test_user_message_received_event_roundtrip() {
        let original = WorkerEvent::new(WorkerEventKind::UserMessageReceived {
            worker_id: 8,
            message: "Test roundtrip received".to_string(),
        });
        let json = original.to_jsonl().unwrap();
        let deserialized: WorkerEvent = serde_json::from_str(&json).unwrap();
        // Verify key fields survived roundtrip
        if let WorkerEventKind::UserMessageReceived { worker_id, message } = &deserialized.kind {
            assert_eq!(*worker_id, 8);
            assert_eq!(message, "Test roundtrip received");
        } else {
            panic!("Wrong event type after roundtrip");
        }
    }
}
