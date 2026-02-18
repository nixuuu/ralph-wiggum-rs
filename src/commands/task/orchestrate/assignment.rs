use std::sync::Arc;
use std::time::{Instant, SystemTime};

use tokio::sync::mpsc;

use crate::commands::task::orchestrate::events::WorkerPhase;
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::worker::{Worker, WorkerConfig};
use crate::commands::task::orchestrate::worker_status::{WorkerState, WorkerStatus};
use crate::commands::task::orchestrate::worktree::WorktreeInfo;
use crate::shared::error::Result;
use crate::shared::progress::TaskStatus;
use crate::shared::tasks::TasksFile;

use super::orchestrator::Orchestrator;
use super::run_loop::RunLoopContext;

// ── Worker slot tracking ────────────────────────────────────────────

/// Per-worker tracking state within the orchestrator.
///
/// Each worker slot tracks whether the worker is idle or busy executing a task.
/// For busy workers, we maintain state needed for orchestration and communication.
#[derive(Debug)]
pub(super) enum WorkerSlot {
    Idle,
    Busy {
        /// Task ID tracked in worker slot for debugging and state consistency.
        task_id: String,
        worktree: WorktreeInfo,
        /// When this worker started its current task (for watchdog stuck detection).
        started_at: Instant,
        /// MCP session ID for this worker (used for cleanup on task completion).
        mcp_session_id: String,
        /// Sender for orchestrator → worker messages.
        /// Allows the orchestrator to send messages directly to a specific worker
        /// (e.g., user input, pause/resume signals, etc.).
        message_tx: mpsc::Sender<String>,
    },
}

// ── Task assignment logic ───────────────────────────────────────────

impl Orchestrator {
    /// Assign ready tasks to idle workers.
    pub(super) async fn assign_tasks(&self, ctx: &mut RunLoopContext<'_>) -> Result<()> {
        // Always refresh ready_queue before assigning — guards against
        // stale state from hot reload or missed refresh paths.
        ctx.scheduler.refresh_ready_queue();

        // Find idle workers
        let idle_workers: Vec<u32> = ctx
            .worker_slots
            .iter()
            .filter(|(_, slot)| matches!(slot, WorkerSlot::Idle))
            .map(|(id, _)| *id)
            .collect();

        let mut started_ids: Vec<String> = Vec::new();

        for worker_id in idle_workers {
            let Some(task_id) = ctx.scheduler.next_ready_task() else {
                break;
            };

            // Find task info from cached_tasks_file
            let cached_tf = ctx.cached_tasks_file.as_ref();
            let leaf = cached_tf.find_leaf(&task_id);
            let task_desc = leaf
                .as_ref()
                .map(crate::shared::tasks::format_task_prompt)
                .unwrap_or_else(|| task_id.clone());

            // Extract component from leaf node
            let component = leaf.as_ref().map(|n| n.component.clone());

            // Extract profile names from task
            let task_profiles: Vec<String> = leaf
                .as_ref()
                .map(|n| n.profiles.clone())
                .unwrap_or_default();

            // Resolve model for this task using cached_tasks_file
            let model = self.resolve_model(&task_id, cached_tf);

            // Create worktree
            let worktree = ctx.worktree_manager.create_worktree(&task_id).await?;

            // Print assignment via TUI
            let msg = ctx.tui.mux_output.format_worker_line(
                worker_id,
                &format!("Assigned: {task_id} → {}", worktree.branch),
            );
            ctx.tui.app.push_log_line(&msg);

            // Mark task as started in scheduler
            ctx.scheduler.mark_started(&task_id);
            started_ids.push(task_id.clone());

            // Update TUI worker status
            ctx.tui.mux_output.assign_worker(worker_id, &task_id);
            let ws = WorkerStatus {
                state: WorkerState::Implementing,
                task_id: Some(task_id.clone()),
                component,
                phase: Some(WorkerPhase::Implement),
                model: model
                    .as_ref()
                    .map(|m| crate::shared::tasks::shorten_model_name(m)),
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
                verify_profiles: Vec::new(),
            };
            ctx.tui.app.update_worker_status(worker_id, ws);
            ctx.tui
                .task_start_times
                .insert(task_id.clone(), Instant::now());

            // Expand setup commands before worktree is moved into WorkerSlot
            // Start with global setup commands
            let mut expanded_setup: Vec<(String, String)> = self
                .setup_commands
                .iter()
                .map(|cmd| {
                    let expanded = expand_setup_command(
                        cmd.command(),
                        &self.project_root,
                        &worktree.path,
                        &task_id,
                        worker_id,
                        None, // No profile name for global commands
                        None, // No profile dir for global commands
                    );
                    (expanded, cmd.label().to_string())
                })
                .collect();

            // Collect valid (known) profiles and warn about unknown ones
            let (valid_profiles, unknown_profiles) =
                filter_valid_profiles(&task_profiles, &self.profiles);

            for profile_name in &unknown_profiles {
                let msg = ctx.tui.mux_output.format_worker_line(
                    worker_id,
                    &format!("⚠ Task {task_id}: nieznany profil \"{profile_name}\" — pomijam"),
                );
                ctx.tui.app.push_log_line(&msg);
            }

            // Append profile-specific setup commands (only for valid profiles, in TOML order)
            for profile_name in &valid_profiles {
                if let Some(profile) = self.profiles.iter().find(|p| &p.name == profile_name) {
                    let profile_dir = profile
                        .working_dir
                        .as_ref()
                        .map(|wd| worktree.path.join(wd));

                    for cmd in &profile.setup_commands {
                        let expanded = expand_setup_command(
                            cmd.command(),
                            &self.project_root,
                            &worktree.path,
                            &task_id,
                            worker_id,
                            Some(profile_name),
                            profile_dir.as_deref(),
                        );
                        expanded_setup.push((expanded, cmd.label().to_string()));
                    }
                }
            }

            // Extract path before moving worktree into WorkerSlot
            let worktree_path = worktree.path.clone();

            // Register MCP session for this worker — scoped to worktree's tasks_path
            let worktree_tasks_path = worktree_path.join(
                self.tasks_path
                    .strip_prefix(&self.project_root)
                    .unwrap_or(&self.tasks_path),
            );
            let mcp_session_id = ctx
                .mcp_session_registry
                .create_session(worktree_tasks_path, true);

            // Create message channel for orchestrator → worker communication.
            // The sender (message_tx) is stored in WorkerSlot::Busy for later use,
            // and the receiver (message_rx) is passed to the Worker instance.
            // This allows the orchestrator to send messages to specific workers.
            let (message_tx, message_rx) = mpsc::channel(16);

            // Update worker slot (moves worktree, avoids full WorktreeInfo clone).
            // Store the message_tx here so the orchestrator can send messages to this worker.
            ctx.worker_slots.insert(
                worker_id,
                WorkerSlot::Busy {
                    task_id: task_id.clone(),
                    worktree,
                    started_at: Instant::now(),
                    mcp_session_id: mcp_session_id.clone(),
                    message_tx,
                },
            );

            // Spawn worker as tokio task
            let worker_config = WorkerConfig {
                system_prompt: self.system_prompt.clone(),
                max_retries: self.config.max_retries,
                use_nerd_font: self.use_nerd_font,
                prompt_prefix: self.prompt_prefix.clone(),
                prompt_suffix: self.prompt_suffix.clone(),
                phase_timeout: self.config.phase_timeout,
                git_timeout: self.config.git_timeout,
                setup_timeout: self.config.setup_timeout,
                mcp_port: ctx.mcp_port,
                mcp_session_id,
                review_model: self.config.review_model.clone(),
                base_commit: None,
                all_profiles: self.profiles.clone(),
            };
            let mut worker = Worker::new(
                worker_id,
                ctx.event_tx.clone(),
                Arc::clone(&ctx.flags.shutdown),
                worker_config,
                message_rx,
            );

            let verify_cmds = self.verify_commands.clone();

            // Pass only valid (known) profiles to worker
            // Invalid profiles were filtered out earlier with warnings
            let valid_profiles_owned = valid_profiles;

            // task_id moved (not cloned) — last use of the owned String
            let task_id_owned = task_id;
            let task_desc_owned = task_desc;
            let model_owned = model;

            let handle = tokio::spawn(async move {
                worker
                    .execute_task(
                        &task_id_owned,
                        &task_desc_owned,
                        model_owned.as_deref(),
                        &worktree_path,
                        &expanded_setup,
                        &verify_cmds,
                        &valid_profiles_owned,
                    )
                    .await
            });

            ctx.join_handles.insert(worker_id, handle);
        }

        // Diagnostic: warn when idle workers exist but scheduler has pending/ready tasks
        let remaining_idle = ctx
            .worker_slots
            .values()
            .filter(|s| matches!(s, WorkerSlot::Idle))
            .count();
        if remaining_idle > 0 {
            let status = ctx.scheduler.status();
            if status.pending > 0 || status.ready > 0 {
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "⚠ {} idle workers, scheduler: {} ready, {} pending, {} done, {} in_progress, {} blocked",
                    remaining_idle,
                    status.ready,
                    status.pending,
                    status.done,
                    status.in_progress,
                    status.blocked,
                ));
                ctx.tui.app.push_log_line(&msg);
            }
        }

        // Batch-update tasks.yml for all newly started tasks
        if !started_ids.is_empty() {
            let updates: Vec<(String, TaskStatus)> = started_ids
                .into_iter()
                .map(|id| (id, TaskStatus::InProgress))
                .collect();
            if let Err(e) = TasksFile::batch_update_statuses(&self.tasks_path, &updates) {
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "Warning: tasks.yml batch update failed: {e}"
                ));
                ctx.tui.app.push_log_line(&msg);
            } else if let Some(mt) = get_mtime(&self.tasks_path) {
                ctx.progress_mtime = Some(mt);
                // Sync cached TasksFile for all started tasks
                let mut updated = (*ctx.cached_tasks_file).clone();
                for (task_id, status) in &updates {
                    let _ = updated.update_status(task_id, status.clone());
                }
                ctx.cached_tasks_file = Arc::new(updated);
            }
        }

        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Expand template variables in a setup command string.
///
/// Supported variables:
/// - `{ROOT_DIR}` — Project root directory (main repository)
/// - `{WORKTREE_DIR}` — Task-specific worktree path (e.g., `/path/to/project-ralph-task-T03`)
/// - `{TASK_ID}` — Task identifier (e.g., "T03", "1.2.3")
/// - `{WORKER_ID}` — Worker slot number (1-based, contextual to worker, not tied to worktree path)
/// - `{PROFILE_NAME}` — Name of the verify profile (only for profile setup commands)
/// - `{PROFILE_DIR}` — Working directory for the profile (only for profile setup commands with working_dir)
///
/// Note: `{WORKER_ID}` is preserved for contextual use (logs, debugging) but worktree paths
/// are now task-based (format: `{prefix}task-{task_id}`) rather than worker-based.
pub(super) fn expand_setup_command(
    cmd: &str,
    root_dir: &std::path::Path,
    worktree_dir: &std::path::Path,
    task_id: &str,
    worker_id: u32,
    profile_name: Option<&str>,
    profile_dir: Option<&std::path::Path>,
) -> String {
    let mut result = cmd
        .replace("{ROOT_DIR}", &root_dir.display().to_string())
        .replace("{WORKTREE_DIR}", &worktree_dir.display().to_string())
        .replace("{TASK_ID}", task_id)
        .replace("{WORKER_ID}", &worker_id.to_string());

    // Expand profile-specific variables if available
    if let Some(name) = profile_name {
        result = result.replace("{PROFILE_NAME}", name);
    }

    if let Some(dir) = profile_dir {
        result = result.replace("{PROFILE_DIR}", &dir.display().to_string());
    }

    result
}

/// Filter task profiles into valid (known) and unknown ones.
///
/// Returns `(valid_profiles, unknown_profiles)` — valid profiles are those
/// that exist in the configuration, unknown ones should trigger warnings.
pub(super) fn filter_valid_profiles(
    task_profiles: &[String],
    known_profiles: &[crate::shared::file_config::VerifyProfile],
) -> (Vec<String>, Vec<String>) {
    let mut valid = Vec::new();
    let mut unknown = Vec::new();

    for name in task_profiles {
        if known_profiles.iter().any(|p| &p.name == name) {
            valid.push(name.clone());
        } else {
            unknown.push(name.clone());
        }
    }

    (valid, unknown)
}

/// Get file modification time.
pub(super) fn get_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_setup_command_all_variables() {
        let result = expand_setup_command(
            "cp {ROOT_DIR}/.env {WORKTREE_DIR}/.env && echo {TASK_ID} w{WORKER_ID}",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/home/user/project-ralph-task-1-2-3"),
            "1.2.3",
            1,
            None,
            None,
        );
        assert_eq!(
            result,
            "cp /home/user/project/.env /home/user/project-ralph-task-1-2-3/.env && echo 1.2.3 w1"
        );
    }

    #[test]
    fn test_expand_setup_command_no_variables() {
        let result = expand_setup_command(
            "npm install",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/home/user/project-ralph-task-T01"),
            "T01",
            2,
            None,
            None,
        );
        assert_eq!(result, "npm install");
    }

    #[test]
    fn test_expand_setup_command_worker_id_independent_of_path() {
        // Verify that WORKER_ID is contextual and independent of worktree path
        // Worker #3 working on task T01 should still expand WORKER_ID to "3"
        let result = expand_setup_command(
            "echo 'Worker {WORKER_ID} working in {WORKTREE_DIR} on {TASK_ID}'",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/home/user/project-ralph-task-T01"),
            "T01",
            3,
            None,
            None,
        );
        assert_eq!(
            result,
            "echo 'Worker 3 working in /home/user/project-ralph-task-T01 on T01'"
        );
    }

    #[test]
    fn test_expand_setup_command_dotted_task_id() {
        // Task IDs like "1.2.3" are sanitized to "1-2-3" in worktree path,
        // but {TASK_ID} variable should preserve original format
        let result = expand_setup_command(
            "echo 'Task {TASK_ID} in {WORKTREE_DIR}'",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/home/user/project-ralph-task-1-2-3"),
            "1.2.3",
            1,
            None,
            None,
        );
        assert_eq!(
            result,
            "echo 'Task 1.2.3 in /home/user/project-ralph-task-1-2-3'"
        );
    }

    #[test]
    fn test_expand_setup_command_with_profile_name() {
        let result = expand_setup_command(
            "echo 'Profile: {PROFILE_NAME} in {WORKTREE_DIR}'",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/home/user/project-ralph-task-T01"),
            "T01",
            1,
            Some("frontend"),
            None,
        );
        assert_eq!(
            result,
            "echo 'Profile: frontend in /home/user/project-ralph-task-T01'"
        );
    }

    #[test]
    fn test_expand_setup_command_with_profile_dir() {
        let result = expand_setup_command(
            "cd {PROFILE_DIR} && npm test",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/home/user/project-ralph-task-T01"),
            "T01",
            1,
            Some("frontend"),
            Some(std::path::Path::new(
                "/home/user/project-ralph-task-T01/frontend",
            )),
        );
        assert_eq!(
            result,
            "cd /home/user/project-ralph-task-T01/frontend && npm test"
        );
    }

    #[test]
    fn test_expand_setup_command_with_all_profile_variables() {
        let result = expand_setup_command(
            "echo '{PROFILE_NAME}: {ROOT_DIR} -> {PROFILE_DIR}'",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/home/user/project-ralph-task-1-2-3"),
            "1.2.3",
            2,
            Some("backend"),
            Some(std::path::Path::new(
                "/home/user/project-ralph-task-1-2-3/api",
            )),
        );
        assert_eq!(
            result,
            "echo 'backend: /home/user/project -> /home/user/project-ralph-task-1-2-3/api'"
        );
    }

    #[test]
    fn test_expand_setup_command_profile_vars_not_replaced_when_none() {
        // When profile vars are None, the placeholders should remain unchanged
        let result = expand_setup_command(
            "echo '{PROFILE_NAME} in {PROFILE_DIR}'",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/home/user/project-ralph-task-T01"),
            "T01",
            1,
            None,
            None,
        );
        assert_eq!(result, "echo '{PROFILE_NAME} in {PROFILE_DIR}'");
    }

    #[test]
    fn test_get_mtime_nonexistent() {
        assert!(get_mtime(std::path::Path::new("/nonexistent/file")).is_none());
    }

    #[test]
    fn test_worker_slot_states() {
        let idle = WorkerSlot::Idle;
        assert!(matches!(idle, WorkerSlot::Idle));

        let (message_tx, _message_rx) = mpsc::channel(16);
        let busy = WorkerSlot::Busy {
            task_id: "T01".to_string(),
            worktree: WorktreeInfo {
                path: std::path::PathBuf::from("/tmp/wt1"),
                branch: "ralph/task/T01".to_string(),
                task_id: "T01".to_string(),
            },
            started_at: Instant::now(),
            mcp_session_id: "test-session-id".to_string(),
            message_tx,
        };
        assert!(matches!(busy, WorkerSlot::Busy { .. }));
    }

    #[test]
    fn test_idle_workers_sorted_order() {
        // Test z workerami 5,3,1,4,2 wstawionymi do HashMap
        // Po filtrze idle + sort, kolejność powinna być [1,2,3,4,5]
        use std::collections::HashMap;

        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        worker_slots.insert(5, WorkerSlot::Idle);
        worker_slots.insert(3, WorkerSlot::Idle);
        worker_slots.insert(1, WorkerSlot::Idle);
        worker_slots.insert(4, WorkerSlot::Idle);
        worker_slots.insert(2, WorkerSlot::Idle);

        // Symulacja kodu z assign_tasks() bez sortowania
        let mut idle_workers_unsorted: Vec<u32> = worker_slots
            .iter()
            .filter(|(_, slot)| matches!(slot, WorkerSlot::Idle))
            .map(|(id, _)| *id)
            .collect();

        // Sortowanie workerów w kolejności rosnącącej ID
        idle_workers_unsorted.sort_unstable();

        assert_eq!(idle_workers_unsorted, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_idle_workers_mixed_states_sorted() {
        // Test z workerami idle=[3,5] i busy=[1,2,4]
        // Po filtrze idle + sort, kolejność powinna być [3,5]
        use std::collections::HashMap;

        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();

        // Idle workers
        worker_slots.insert(3, WorkerSlot::Idle);
        worker_slots.insert(5, WorkerSlot::Idle);

        // Busy workers
        let (msg_tx1, _rx1) = mpsc::channel(16);
        worker_slots.insert(
            1,
            WorkerSlot::Busy {
                task_id: "T01".to_string(),
                worktree: WorktreeInfo {
                    path: std::path::PathBuf::from("/tmp/wt1"),
                    branch: "ralph/task/T01".to_string(),
                    task_id: "T01".to_string(),
                },
                started_at: Instant::now(),
                mcp_session_id: "session1".to_string(),
                message_tx: msg_tx1,
            },
        );
        let (msg_tx2, _rx2) = mpsc::channel(16);
        worker_slots.insert(
            2,
            WorkerSlot::Busy {
                task_id: "T02".to_string(),
                worktree: WorktreeInfo {
                    path: std::path::PathBuf::from("/tmp/wt2"),
                    branch: "ralph/task/T02".to_string(),
                    task_id: "T02".to_string(),
                },
                started_at: Instant::now(),
                mcp_session_id: "session2".to_string(),
                message_tx: msg_tx2,
            },
        );
        let (msg_tx4, _rx4) = mpsc::channel(16);
        worker_slots.insert(
            4,
            WorkerSlot::Busy {
                task_id: "T04".to_string(),
                worktree: WorktreeInfo {
                    path: std::path::PathBuf::from("/tmp/wt4"),
                    branch: "ralph/task/T04".to_string(),
                    task_id: "T04".to_string(),
                },
                started_at: Instant::now(),
                mcp_session_id: "session4".to_string(),
                message_tx: msg_tx4,
            },
        );

        // Filtrowanie i sortowanie idle workerów
        let mut idle_workers: Vec<u32> = worker_slots
            .iter()
            .filter(|(_, slot)| matches!(slot, WorkerSlot::Idle))
            .map(|(id, _)| *id)
            .collect();
        idle_workers.sort_unstable();

        assert_eq!(idle_workers, vec![3, 5]);
    }

    #[test]
    fn test_idle_workers_single() {
        // Test z jednym idle workerem zwraca ten worker
        use std::collections::HashMap;

        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        worker_slots.insert(7, WorkerSlot::Idle);
        let (msg_tx, _rx) = mpsc::channel(16);
        worker_slots.insert(
            1,
            WorkerSlot::Busy {
                task_id: "T01".to_string(),
                worktree: WorktreeInfo {
                    path: std::path::PathBuf::from("/tmp/wt1"),
                    branch: "ralph/task/T01".to_string(),
                    task_id: "T01".to_string(),
                },
                started_at: Instant::now(),
                mcp_session_id: "session1".to_string(),
                message_tx: msg_tx,
            },
        );

        let idle_workers: Vec<u32> = worker_slots
            .iter()
            .filter(|(_, slot)| matches!(slot, WorkerSlot::Idle))
            .map(|(id, _)| *id)
            .collect();

        assert_eq!(idle_workers.len(), 1);
        assert_eq!(idle_workers[0], 7);
    }

    // ── Task 47.2: Integracja warning dla nieznanych profili ──────────

    /// Helper: create a minimal VerifyProfile with only a name.
    fn make_profile(name: &str) -> crate::shared::file_config::VerifyProfile {
        crate::shared::file_config::VerifyProfile {
            name: name.to_string(),
            description: None,
            paths: vec![],
            working_dir: None,
            verify_commands: vec![],
            setup_commands: vec![],
        }
    }

    #[test]
    fn test_filter_valid_profiles_all_known() {
        let profiles = [make_profile("frontend"), make_profile("backend")];
        let task = vec!["frontend".into(), "backend".into()];

        let (valid, unknown) = filter_valid_profiles(&task, &profiles);

        assert_eq!(valid, ["frontend", "backend"]);
        assert!(unknown.is_empty());
    }

    #[test]
    fn test_filter_valid_profiles_unknown() {
        let profiles = [make_profile("frontend")];
        let task = vec!["frontend".into(), "nonexistent".into()];

        let (valid, unknown) = filter_valid_profiles(&task, &profiles);

        assert_eq!(valid, ["frontend"]);
        assert_eq!(unknown, ["nonexistent"]);
    }

    #[test]
    fn test_filter_valid_profiles_mixed() {
        let profiles = [make_profile("frontend"), make_profile("backend")];
        let task = vec![
            "frontend".into(),
            "unknown1".into(),
            "backend".into(),
            "unknown2".into(),
        ];

        let (valid, unknown) = filter_valid_profiles(&task, &profiles);

        assert_eq!(valid, ["frontend", "backend"]);
        assert_eq!(unknown, ["unknown1", "unknown2"]);
    }

    #[test]
    fn test_filter_valid_profiles_all_unknown() {
        let profiles: Vec<crate::shared::file_config::VerifyProfile> = vec![];
        let task = vec!["profile1".into(), "profile2".into()];

        let (valid, unknown) = filter_valid_profiles(&task, &profiles);

        assert!(valid.is_empty());
        assert_eq!(unknown, ["profile1", "profile2"]);
    }

    #[test]
    fn test_filter_valid_profiles_empty_task_profiles() {
        let profiles = [make_profile("frontend")];
        let task: Vec<String> = vec![];

        let (valid, unknown) = filter_valid_profiles(&task, &profiles);

        assert!(valid.is_empty());
        assert!(unknown.is_empty());
    }

    // ── Task 42.2: Setup phase z profilami ────────────────────────────

    /// Kolejność setup commands — globalne → profile (w kolejności TOML).
    /// Weryfikuje logikę składania expanded_setup z assign_tasks().
    #[test]
    fn test_setup_commands_order_global_then_profiles() {
        use crate::shared::file_config::{SetupCommand, VerifyProfile};

        // Globalne setup commands
        let global_cmds = vec![
            SetupCommand::Simple("global_cmd_1".to_string()),
            SetupCommand::Simple("global_cmd_2".to_string()),
        ];

        // Profile z setup commands (w kolejności TOML)
        let profiles = [
            VerifyProfile {
                name: "frontend".to_string(),
                description: None,
                paths: vec![],
                working_dir: None,
                verify_commands: vec![],
                setup_commands: vec![
                    SetupCommand::Simple("frontend_cmd_1".to_string()),
                    SetupCommand::Simple("frontend_cmd_2".to_string()),
                ],
            },
            VerifyProfile {
                name: "backend".to_string(),
                description: None,
                paths: vec![],
                working_dir: None,
                verify_commands: vec![],
                setup_commands: vec![
                    SetupCommand::Simple("backend_cmd_1".to_string()),
                    SetupCommand::Simple("backend_cmd_2".to_string()),
                ],
            },
        ];

        // Task profiles w kolejności TOML
        let task_profiles = vec!["frontend".to_string(), "backend".to_string()];

        // Symulacja budowania expanded_setup z assignment.rs
        let mut expanded_setup: Vec<String> = vec![];

        // Append global commands
        for cmd in &global_cmds {
            expanded_setup.push(cmd.command().to_string());
        }

        // Append profile commands (w kolejności TOML)
        for profile_name in &task_profiles {
            if let Some(profile) = profiles.iter().find(|p| &p.name == profile_name) {
                for cmd in &profile.setup_commands {
                    expanded_setup.push(cmd.command().to_string());
                }
            }
        }

        // Verify order: global_1, global_2, frontend_1, frontend_2, backend_1, backend_2
        assert_eq!(expanded_setup.len(), 6);
        assert_eq!(expanded_setup[0], "global_cmd_1");
        assert_eq!(expanded_setup[1], "global_cmd_2");
        assert_eq!(expanded_setup[2], "frontend_cmd_1");
        assert_eq!(expanded_setup[3], "frontend_cmd_2");
        assert_eq!(expanded_setup[4], "backend_cmd_1");
        assert_eq!(expanded_setup[5], "backend_cmd_2");
    }

    /// Test 3: Profil bez setup_commands → pomijany (skip)
    #[test]
    fn test_profile_without_setup_commands_skipped() {
        use crate::shared::file_config::{SetupCommand, VerifyProfile};

        let global_cmds = vec![SetupCommand::Simple("global_cmd".to_string())];

        let profiles = [
            VerifyProfile {
                name: "no_setup".to_string(),
                description: None,
                paths: vec![],
                working_dir: None,
                verify_commands: vec![],
                setup_commands: vec![], // Empty setup_commands
            },
            VerifyProfile {
                name: "with_setup".to_string(),
                description: None,
                paths: vec![],
                working_dir: None,
                verify_commands: vec![],
                setup_commands: vec![SetupCommand::Simple("profile_cmd".to_string())],
            },
        ];

        let task_profiles = vec!["no_setup".to_string(), "with_setup".to_string()];

        let mut expanded_setup: Vec<String> = vec![];

        // Append global commands
        for cmd in &global_cmds {
            expanded_setup.push(cmd.command().to_string());
        }

        // Append profile commands (skip empty)
        for profile_name in &task_profiles {
            if let Some(profile) = profiles.iter().find(|p| &p.name == profile_name) {
                for cmd in &profile.setup_commands {
                    expanded_setup.push(cmd.command().to_string());
                }
            }
        }

        // Only global and with_setup profile commands
        assert_eq!(expanded_setup.len(), 2);
        assert_eq!(expanded_setup[0], "global_cmd");
        assert_eq!(expanded_setup[1], "profile_cmd");
    }

    /// {PROFILE_DIR} bez working_dir ale z profile_name → {PROFILE_DIR} pozostaje nierozwinięty
    #[test]
    fn test_expand_setup_command_profile_dir_without_working_dir() {
        let result = expand_setup_command(
            "cd {PROFILE_DIR} && npm test",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/home/user/project-ralph-task-T01"),
            "T01",
            1,
            Some("frontend"),
            None, // No working_dir
        );
        // {PROFILE_DIR} remains unexpanded
        assert_eq!(result, "cd {PROFILE_DIR} && npm test");
    }
}
