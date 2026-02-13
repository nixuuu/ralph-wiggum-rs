use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::commands::mcp::server::McpServer;
use crate::commands::mcp::state::QuestionEnvelope;
use crate::commands::task::orchestrate::dashboard::Dashboard;
use crate::commands::task::orchestrate::dashboard_input::{
    DashboardInputParams, DashboardInputThread,
};
use crate::commands::task::orchestrate::events::{EventLogger, WorkerEvent};
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::scheduler::TaskScheduler;
use crate::commands::task::orchestrate::state::{Lockfile, OrchestrateState};
use crate::commands::task::orchestrate::worktree::WorktreeManager;
use crate::shared::dag::TaskDag;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::{FileConfig, SetupCommand, VerifyCommand};
use crate::shared::tasks::TasksFile;
use crate::templates;

use super::assignment::{WorkerSlot, get_mtime};
use super::config::{InputFlags, ResolvedConfig};
use super::orchestrator_merge::MergeContext;
use super::orchestrator_tui::TuiContext;
use super::run_loop::RunLoopContext;

// ── Orchestrator ────────────────────────────────────────────────────

/// Main orchestrator — coordinates workers, scheduler, merges, and state.
pub struct Orchestrator {
    pub(super) config: ResolvedConfig,
    pub(super) project_root: PathBuf,
    pub(super) tasks_path: PathBuf,
    pub(super) system_prompt: String,
    pub(super) verify_commands: Vec<VerifyCommand>,
    pub(super) setup_commands: Vec<SetupCommand>,
    pub(super) use_nerd_font: bool,
    pub(super) prompt_prefix: Option<String>,
    pub(super) prompt_suffix: Option<String>,
}

impl Orchestrator {
    pub fn new(
        config: ResolvedConfig,
        file_config: &FileConfig,
        project_root: PathBuf,
    ) -> Result<Self> {
        let tasks_path = project_root.join(&file_config.task.tasks_file);

        // Use embedded system prompt template (general part, before task-specific section)
        let base_system_prompt = templates::CONTINUE_SYSTEM_PROMPT
            .split("\n---\n\n# Your Task")
            .next()
            .unwrap_or(templates::CONTINUE_SYSTEM_PROMPT);

        // Prepend custom system prompt if configured
        let system_prompt = if let Some(custom) = &file_config.prompt.system {
            if !custom.trim().is_empty() {
                format!("{}\n\n{}", custom, base_system_prompt)
            } else {
                base_system_prompt.to_string()
            }
        } else {
            base_system_prompt.to_string()
        };

        let verify_commands = file_config.task.orchestrate.verify_commands.clone();

        let setup_commands = file_config.task.orchestrate.setup_commands.clone();

        // Extract prompt prefix/suffix, filtering out empty strings
        let prompt_prefix = file_config
            .prompt
            .prefix
            .clone()
            .filter(|s| !s.trim().is_empty());
        let prompt_suffix = file_config
            .prompt
            .suffix
            .clone()
            .filter(|s| !s.trim().is_empty());

        Ok(Self {
            config,
            project_root,
            tasks_path,
            system_prompt,
            verify_commands,
            setup_commands,
            use_nerd_font: file_config.ui.nerd_font,
            prompt_prefix,
            prompt_suffix,
        })
    }

    /// Main entry point — run the full orchestration session.
    pub async fn execute(&self) -> Result<()> {
        // 1. Load and parse .ralph/tasks.yml
        let tasks_file = TasksFile::load(&self.tasks_path)
            .map_err(|e| RalphError::Orchestrate(format!("Failed to read tasks.yml: {e}")))?;
        let progress = tasks_file.to_summary();

        // 2. Build and validate DAG
        let frontmatter = progress.frontmatter.clone().unwrap_or_default();
        let dag = TaskDag::from_tasks_file(&tasks_file);

        if let Some(cycle) = dag.detect_cycles() {
            return Err(RalphError::DagCycle(cycle));
        }

        // 2b. Dry-run mode — show DAG visualization and exit without side effects
        if self.config.dry_run {
            use crate::commands::task::orchestrate::dry_run;

            let viz = dry_run::visualize_dag(&dag, &progress);
            let dag_output = dry_run::format_dag(&viz, self.config.workers);
            println!("{dag_output}");
            println!();
            let dep_list = dry_run::format_dep_list(&dag, &progress);
            println!("Task dependency list:\n{dep_list}");
            return Ok(());
        }

        // 3. Acquire lockfile
        let ralph_dir = self.project_root.join(".ralph");
        std::fs::create_dir_all(&ralph_dir)?;
        let lock_path = ralph_dir.join("orchestrate.lock");
        let lockfile = Lockfile::acquire(&lock_path)?;

        // 4. Initialize scheduler
        let mut scheduler = TaskScheduler::new(dag, &progress, self.config.max_retries);

        // 5. Initialize worktree manager
        let worktree_manager = WorktreeManager::new(
            self.project_root.clone(),
            self.config.worktree_prefix.clone(),
        );

        // 6. Initialize state
        let state_path = ralph_dir.join("orchestrate.yaml");
        let mut state = if self.config.resume && state_path.exists() {
            OrchestrateState::load(&state_path)?
        } else {
            OrchestrateState::new(self.config.workers, frontmatter.deps.clone())
        };

        // 7. Initialize event channel and logger
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>(1024);
        let log_dir = ralph_dir.join("logs");
        let combined_log_path = ralph_dir.join("orchestrate.log");
        let event_logger = EventLogger::new(log_dir, Some(&combined_log_path))?;

        // 8. Initialize worker slots
        let worker_count = self.config.workers;
        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        for i in 1..=worker_count {
            worker_slots.insert(i, WorkerSlot::Idle);
        }

        // 9. Shutdown signaling
        let shutdown = Arc::new(AtomicBool::new(false));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));
        let resize_flag = Arc::new(AtomicBool::new(false));
        let focused_worker = Arc::new(AtomicU32::new(0));
        let scroll_delta = Arc::new(AtomicI32::new(0));
        let render_notify = Arc::new(tokio::sync::Notify::new());
        let show_task_preview = Arc::new(AtomicBool::new(false));
        let reload_requested = Arc::new(AtomicBool::new(false));
        let quit_state = Arc::new(AtomicU8::new(0)); // 0=running, 1=quit_pending
        let completed = Arc::new(AtomicBool::new(false));
        let restart_worker_id = Arc::new(AtomicU32::new(0)); // 0=none, >0=worker ID
        let restart_confirmed = Arc::new(AtomicBool::new(false));

        // 9b. Start shared MCP server for all workers
        // Dummy channel — workers don't use ask_user, but McpServer requires it
        let (question_tx, _question_rx) = mpsc::channel::<QuestionEnvelope>(1);
        let mcp_cancel_token = CancellationToken::new();
        let mcp_server = McpServer::new(
            self.tasks_path.clone(),
            question_tx,
            mcp_cancel_token.clone(),
        );
        let (mcp_port, mcp_server_handle) = mcp_server.start().await?;
        // Session registry is shared with the server's AppState — we access
        // it through the server to register per-worker sessions later.
        let mcp_session_registry = mcp_server.session_registry();

        // 10. Initialize TUI (fullscreen dashboard)
        let mut tui = TuiContext {
            dashboard: Dashboard::new(worker_count)?,
            mux_output: MultiplexedOutput::new(),
            task_start_times: HashMap::new(),
            task_summaries: Vec::new(),
        };

        // 11. Initialize active_worker_ids (Task 21.3)
        // Start with all workers considered active (will be updated by dashboard on render)
        let active_worker_ids = Arc::new(std::sync::Mutex::new(
            (1..=worker_count).collect::<Vec<u32>>(),
        ));

        // 12. Spawn input thread (dedicated OS thread for crossterm)
        let mut input_thread = DashboardInputThread::spawn(DashboardInputParams {
            shutdown: Arc::clone(&shutdown),
            graceful_shutdown: Arc::clone(&graceful_shutdown),
            resize_flag: Arc::clone(&resize_flag),
            focused_worker: Arc::clone(&focused_worker),
            scroll_delta: Arc::clone(&scroll_delta),
            render_notify: Arc::clone(&render_notify),
            show_task_preview: Arc::clone(&show_task_preview),
            reload_requested: Arc::clone(&reload_requested),
            quit_state: Arc::clone(&quit_state),
            restart_worker_id: Arc::clone(&restart_worker_id),
            restart_confirmed: Arc::clone(&restart_confirmed),
            active_worker_ids: Arc::clone(&active_worker_ids),
        });

        // 13. Build task lookup for merge
        let task_lookup: HashMap<&str, &crate::shared::progress::ProgressTask> =
            progress.tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        // 14. Build run loop context
        let flags = InputFlags {
            shutdown,
            graceful_shutdown,
            resize_flag,
            focused_worker,
            scroll_delta,
            render_notify,
            show_task_preview,
            reload_requested,
            quit_state: Arc::clone(&quit_state),
            completed,
            restart_worker_id,
            restart_confirmed,
            active_worker_ids,
        };

        // Cache TasksFile in Arc for efficient sharing with dashboard preview
        let cached_tasks_file = Arc::new(tasks_file);

        let mut ctx = RunLoopContext {
            scheduler: &mut scheduler,
            worktree_manager: &worktree_manager,
            state: &mut state,
            state_path: &state_path,
            event_tx,
            event_rx,
            event_logger,
            worker_slots,
            join_handles: HashMap::new(),
            tasks_file: cached_tasks_file.as_ref(),
            progress: &progress,
            task_lookup,
            lockfile: Some(lockfile),
            tui: &mut tui,
            merge_ctx: MergeContext::new(),
            progress_mtime: get_mtime(&self.tasks_path),
            flags,
            cached_tasks_file: Arc::clone(&cached_tasks_file),
            last_heartbeat: HashMap::new(),
            mcp_port,
            mcp_session_registry,
            mcp_cancel_token: mcp_cancel_token.clone(),
            mcp_server_handle,
        };

        // 14. Run the main orchestration loop
        let result = self.run_loop(&mut ctx).await;

        // 15. Cleanup TUI
        ctx.tui.dashboard.cleanup()?;
        input_thread.stop();

        result
    }

    /// Resolve the model to use for a specific task (with alias support).
    pub(super) fn resolve_model(&self, task_id: &str, tasks_file: &TasksFile) -> Option<String> {
        let models = tasks_file.models_map();
        let raw: Option<&String> = models
            .get(task_id)
            .or(tasks_file.default_model.as_ref())
            .or(self.config.model.as_ref());
        raw.map(|m| crate::shared::tasks::resolve_model_alias(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_resolve_model_per_task() {
        use crate::shared::progress::TaskStatus;
        use crate::shared::tasks::{TaskNode, TasksFile};

        let tasks_file = TasksFile {
            default_model: Some("claude-sonnet-4-5-20250929".to_string()),
            tasks: vec![TaskNode {
                id: "T01".to_string(),
                name: "Test".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Todo),
                deps: Vec::new(),
                model: Some("claude-opus-4-6".to_string()),
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: Vec::new(),
            }],
        };

        let config = ResolvedConfig {
            workers: 2,
            max_retries: 3,
            model: Some("fallback-model".to_string()),
            worktree_prefix: None,
            verbose: false,
            resume: false,
            dry_run: false,
            no_merge: false,
            max_cost: None,
            timeout: None,
            task_filter: None,
            conflict_resolution_model: "opus".to_string(),
            review_model: "opus".to_string(),
            watchdog_interval_secs: 10,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            merge_timeout: None,
        };

        let orch = Orchestrator {
            config,
            project_root: PathBuf::from("/tmp/test"),
            tasks_path: PathBuf::from("/tmp/test/.ralph/tasks.yml"),
            system_prompt: String::new(),
            verify_commands: Vec::new(),
            setup_commands: Vec::new(),
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
        };

        assert_eq!(
            orch.resolve_model("T01", &tasks_file),
            Some("claude-opus-4-6".to_string())
        );
        assert_eq!(
            orch.resolve_model("T99", &tasks_file),
            Some("claude-sonnet-4-5-20250929".to_string())
        );
    }

    #[test]
    fn test_resolve_model_cli_fallback() {
        use crate::shared::progress::TaskStatus;
        use crate::shared::tasks::{TaskNode, TasksFile};

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![TaskNode {
                id: "T01".to_string(),
                name: "Test".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Todo),
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: Vec::new(),
            }],
        };

        let config = ResolvedConfig {
            workers: 2,
            max_retries: 3,
            model: Some("cli-model".to_string()),
            worktree_prefix: None,
            verbose: false,
            resume: false,
            dry_run: false,
            no_merge: false,
            max_cost: None,
            timeout: None,
            task_filter: None,
            conflict_resolution_model: "opus".to_string(),
            review_model: "opus".to_string(),
            watchdog_interval_secs: 10,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            merge_timeout: None,
        };

        let orch = Orchestrator {
            config,
            project_root: PathBuf::from("/tmp/test"),
            tasks_path: PathBuf::from("/tmp/test/.ralph/tasks.yml"),
            system_prompt: String::new(),
            verify_commands: Vec::new(),
            setup_commands: Vec::new(),
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
        };

        assert_eq!(
            orch.resolve_model("T01", &tasks_file),
            Some("cli-model".to_string())
        );
    }

    #[test]
    fn test_completion_does_not_break_loop() {
        let completed = Arc::new(AtomicBool::new(false));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));

        let mut iterations = 0;

        loop {
            iterations += 1;

            if !completed.load(Ordering::Relaxed) {
                completed.store(true, Ordering::Relaxed);
            }

            if graceful_shutdown.load(Ordering::Relaxed) {
                break;
            }

            if iterations >= 3 {
                graceful_shutdown.store(true, Ordering::Relaxed);
            }
        }

        assert!(completed.load(Ordering::Relaxed));
        assert!(graceful_shutdown.load(Ordering::Relaxed));
        assert!(iterations > 1);
    }

    #[test]
    fn test_quit_confirmation_works_in_completed_state() {
        let quit_state = Arc::new(AtomicU8::new(0));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicBool::new(true));

        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit != 1 {
            quit_state.store(1, Ordering::SeqCst);
        }
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);

        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }

        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_completion_idempotent_flag_set() {
        let completed = Arc::new(AtomicBool::new(false));

        let mut times_set = 0;

        for _ in 0..5 {
            if !completed.load(Ordering::Relaxed) {
                completed.store(true, Ordering::Relaxed);
                times_set += 1;
            }
        }

        assert_eq!(times_set, 1);
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_esc_in_completed_state_closes_preview_or_noop() {
        let _completed = Arc::new(AtomicBool::new(true));
        let show_task_preview = Arc::new(AtomicBool::new(true));
        let quit_state = Arc::new(AtomicU8::new(0));
        let focused_worker = Arc::new(AtomicU32::new(1));

        assert!(_completed.load(Ordering::Relaxed));
        assert!(show_task_preview.load(Ordering::Relaxed));
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);

        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            quit_state.store(0, Ordering::SeqCst);
        } else {
            focused_worker.store(0, Ordering::Relaxed);
        }

        assert!(show_task_preview.load(Ordering::Relaxed));
        assert_eq!(focused_worker.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_p_during_completion_shows_preview_overlay() {
        let _completed = Arc::new(AtomicBool::new(true));
        let show_task_preview = Arc::new(AtomicBool::new(false));
        let quit_state = Arc::new(AtomicU8::new(0));

        assert!(_completed.load(Ordering::Relaxed));
        assert!(!show_task_preview.load(Ordering::Relaxed));

        if quit_state.load(Ordering::SeqCst) == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }
        let current = show_task_preview.load(Ordering::Relaxed);
        show_task_preview.store(!current, Ordering::Relaxed);
        assert!(show_task_preview.load(Ordering::Relaxed));

        if quit_state.load(Ordering::SeqCst) == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }
        let current = show_task_preview.load(Ordering::Relaxed);
        show_task_preview.store(!current, Ordering::Relaxed);
        assert!(!show_task_preview.load(Ordering::Relaxed));
    }

    #[test]
    fn test_q_during_completion_with_preview_open() {
        let _completed = Arc::new(AtomicBool::new(true));
        let show_task_preview = Arc::new(AtomicBool::new(true));
        let quit_state = Arc::new(AtomicU8::new(0));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));

        assert!(show_task_preview.load(Ordering::Relaxed));

        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit != 1 {
            quit_state.store(1, Ordering::SeqCst);
        }
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);
        assert!(show_task_preview.load(Ordering::Relaxed));

        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }

        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert!(show_task_preview.load(Ordering::Relaxed));
    }

    #[test]
    fn test_completed_state_all_transitions() {
        let completed = Arc::new(AtomicBool::new(true));
        let quit_state = Arc::new(AtomicU8::new(0));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));
        let show_task_preview = Arc::new(AtomicBool::new(false));

        assert!(completed.load(Ordering::Relaxed));
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);

        let current = show_task_preview.load(Ordering::Relaxed);
        show_task_preview.store(!current, Ordering::Relaxed);
        assert!(show_task_preview.load(Ordering::Relaxed));

        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit != 1 {
            quit_state.store(1, Ordering::SeqCst);
        }
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);
        assert!(show_task_preview.load(Ordering::Relaxed));

        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }
        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_completed_flag_reset_on_reload() {
        use crate::commands::task::orchestrate::scheduler::TaskScheduler;
        use crate::shared::dag::TaskDag;
        use crate::shared::progress::TaskStatus;
        use crate::shared::tasks::{TaskNode, TasksFile};

        // Setup initial state: all tasks done, completed=true
        let completed = Arc::new(AtomicBool::new(true));

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![TaskNode {
                id: "T01".to_string(),
                name: "First".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Done),
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: Vec::new(),
            }],
        };

        let progress = tasks_file.to_summary();
        let dag = TaskDag::from_tasks_file(&tasks_file);
        let mut scheduler = TaskScheduler::new(dag, &progress, 3);

        assert!(scheduler.is_complete());
        assert!(completed.load(Ordering::Relaxed));

        // Simulate reload with new todo tasks
        let new_tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                TaskNode {
                    id: "T01".to_string(),
                    name: "First".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Done),
                    deps: Vec::new(),
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
                TaskNode {
                    id: "T02".to_string(),
                    name: "Second".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo),
                    deps: Vec::new(),
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
            ],
        };

        let new_dag = TaskDag::from_tasks_file(&new_tasks_file);
        scheduler.add_tasks(new_dag);

        // Scheduler should now be incomplete
        assert!(!scheduler.is_complete());

        // Simulate the completed flag reset logic from apply_reload
        if completed.load(Ordering::Relaxed) && !scheduler.is_complete() {
            completed.store(false, Ordering::Relaxed);
        }

        assert!(!completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_completed_flag_stays_on_empty_reload() {
        use crate::commands::task::orchestrate::scheduler::TaskScheduler;
        use crate::shared::dag::TaskDag;
        use crate::shared::progress::TaskStatus;
        use crate::shared::tasks::{TaskNode, TasksFile};

        // Setup initial state: all tasks done, completed=true
        let completed = Arc::new(AtomicBool::new(true));

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![TaskNode {
                id: "T01".to_string(),
                name: "First".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Done),
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: Vec::new(),
            }],
        };

        let progress = tasks_file.to_summary();
        let dag = TaskDag::from_tasks_file(&tasks_file);
        let mut scheduler = TaskScheduler::new(dag, &progress, 3);

        assert!(scheduler.is_complete());
        assert!(completed.load(Ordering::Relaxed));

        // Simulate reload with NO new tasks (same file)
        let new_dag = TaskDag::from_tasks_file(&tasks_file);
        scheduler.add_tasks(new_dag);

        // Scheduler should still be complete
        assert!(scheduler.is_complete());

        // Simulate the completed flag reset logic from apply_reload
        if completed.load(Ordering::Relaxed) && !scheduler.is_complete() {
            completed.store(false, Ordering::Relaxed);
        }

        // Completed flag should remain true
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_post_loop_cleanup_summary_formatting() {
        use crate::commands::task::orchestrate::summary;
        use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
        use std::time::Duration;

        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.038,
                duration: Duration::from_secs(32),
                retries: 0,
            },
        ];

        let elapsed = Duration::from_secs(50);
        let summary_text = summary::format_summary(&entries, elapsed);

        assert!(summary_text.contains("T01"));
        assert!(summary_text.contains("T02"));
        assert!(summary_text.contains("Done"));
        assert!(summary_text.contains("TOTAL"));
        assert!(summary_text.contains("2/2 done"));
        assert!(summary_text.contains("Parallelism speedup"));
    }

    #[test]
    fn test_post_loop_cleanup_summary_empty_case() {
        use crate::commands::task::orchestrate::summary;
        use std::time::Duration;

        let entries: Vec<crate::commands::task::orchestrate::summary::TaskSummaryEntry> = vec![];
        let elapsed = Duration::from_secs(10);
        let summary_text = summary::format_summary(&entries, elapsed);

        assert!(summary_text.contains("No tasks were executed"));
    }

    #[test]
    fn test_post_loop_cleanup_summary_mixed_results() {
        use crate::commands::task::orchestrate::summary;
        use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
        use std::time::Duration;

        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.038,
                duration: Duration::from_secs(32),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T03".to_string(),
                status: "Blocked".to_string(),
                cost_usd: 0.089,
                duration: Duration::ZERO,
                retries: 3,
            },
        ];

        let elapsed = Duration::from_secs(60);
        let summary_text = summary::format_summary(&entries, elapsed);

        assert!(summary_text.contains("T01"));
        assert!(summary_text.contains("T02"));
        assert!(summary_text.contains("T03"));
        assert!(summary_text.contains("2/3 done"));
        assert!(summary_text.contains("Blocked"));
        assert!(summary_text.contains("$0.1690"));
    }

    #[test]
    fn test_cached_tasks_file_sync_on_hot_reload() {
        // Test: Verify cached_tasks_file is synced with scheduler state during hot-reload
        // This test verifies the implementation from task(12.1.3)
        use crate::commands::task::orchestrate::scheduler::TaskScheduler;
        use crate::shared::dag::TaskDag;
        use crate::shared::progress::TaskStatus;
        use crate::shared::tasks::{TaskNode, TasksFile};

        // Initial tasks file with 3 tasks
        let initial_tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                TaskNode {
                    id: "T01".to_string(),
                    name: "First".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo),
                    deps: Vec::new(),
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
                TaskNode {
                    id: "T02".to_string(),
                    name: "Second".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo),
                    deps: vec!["T01".to_string()],
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
                TaskNode {
                    id: "T03".to_string(),
                    name: "Third".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo),
                    deps: vec!["T02".to_string()],
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
            ],
        };

        let progress = initial_tasks_file.to_summary();
        let dag = TaskDag::from_tasks_file(&initial_tasks_file);
        let mut scheduler = TaskScheduler::new(dag, &progress, 3);

        // Simulate runtime: mark T01 as Done, T02 as InProgress
        scheduler.mark_done("T01");
        // For InProgress, we'd need to assign it to a worker, but for this test
        // we'll simulate the scheduler state manually by tracking done tasks

        // Verify scheduler state
        assert!(scheduler.done_tasks().contains(&"T01".to_string()));

        // Simulate hot-reload: user adds T04, file still shows T01/T02/T03 as Todo
        let fresh_tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                TaskNode {
                    id: "T01".to_string(),
                    name: "First".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo), // File shows Todo
                    deps: Vec::new(),
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
                TaskNode {
                    id: "T02".to_string(),
                    name: "Second".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo), // File shows Todo
                    deps: vec!["T01".to_string()],
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
                TaskNode {
                    id: "T03".to_string(),
                    name: "Third".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo),
                    deps: vec!["T02".to_string()],
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
                TaskNode {
                    id: "T04".to_string(),
                    name: "Fourth".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo),
                    deps: Vec::new(),
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
            ],
        };

        // Apply hot-reload logic from orchestrator.rs:644-666
        let mut synced_tasks_file = fresh_tasks_file.clone();

        // Patch fresh TasksFile with scheduler runtime statuses
        for task_id in scheduler.done_tasks() {
            let _ = synced_tasks_file.update_status(task_id, TaskStatus::Done);
        }
        for task_id in scheduler.in_progress_tasks() {
            let _ = synced_tasks_file.update_status(task_id, TaskStatus::InProgress);
        }
        for task_id in scheduler.blocked_tasks() {
            let _ = synced_tasks_file.update_status(task_id, TaskStatus::Blocked);
        }

        let cached_tasks_file = Arc::new(synced_tasks_file);

        // Verify cached_tasks_file reflects scheduler state
        let t01 = cached_tasks_file
            .find_leaf("T01")
            .expect("T01 should exist");
        assert_eq!(
            t01.status,
            TaskStatus::Done,
            "T01 should be Done from scheduler state"
        );

        let t02 = cached_tasks_file
            .find_leaf("T02")
            .expect("T02 should exist");
        assert_eq!(
            t02.status,
            TaskStatus::Todo,
            "T02 should remain Todo (not started)"
        );

        let t03 = cached_tasks_file
            .find_leaf("T03")
            .expect("T03 should exist");
        assert_eq!(t03.status, TaskStatus::Todo, "T03 should remain Todo");

        let t04 = cached_tasks_file
            .find_leaf("T04")
            .expect("T04 should exist");
        assert_eq!(
            t04.status,
            TaskStatus::Todo,
            "T04 (new task) should be Todo"
        );
    }

    #[test]
    fn test_cached_tasks_file_sync_preserves_new_task_statuses() {
        // Test: Verify that when hot-reloading with tasks that have explicit statuses,
        // scheduler state overrides only tracked tasks, not new ones
        use crate::commands::task::orchestrate::scheduler::TaskScheduler;
        use crate::shared::dag::TaskDag;
        use crate::shared::progress::TaskStatus;
        use crate::shared::tasks::{TaskNode, TasksFile};

        let initial_tasks_file = TasksFile {
            default_model: None,
            tasks: vec![TaskNode {
                id: "T01".to_string(),
                name: "First".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Todo),
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: Vec::new(),
            }],
        };

        let progress = initial_tasks_file.to_summary();
        let dag = TaskDag::from_tasks_file(&initial_tasks_file);
        let mut scheduler = TaskScheduler::new(dag, &progress, 3);
        scheduler.mark_done("T01");

        // Hot-reload: Add T02 with explicit Blocked status
        let fresh_tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                TaskNode {
                    id: "T01".to_string(),
                    name: "First".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo), // File shows Todo but scheduler knows Done
                    deps: Vec::new(),
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
                TaskNode {
                    id: "T02".to_string(),
                    name: "Second".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Blocked), // User explicitly set Blocked
                    deps: Vec::new(),
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                },
            ],
        };

        // Apply hot-reload logic
        let mut synced_tasks_file = fresh_tasks_file.clone();
        for task_id in scheduler.done_tasks() {
            let _ = synced_tasks_file.update_status(task_id, TaskStatus::Done);
        }
        for task_id in scheduler.blocked_tasks() {
            let _ = synced_tasks_file.update_status(task_id, TaskStatus::Blocked);
        }

        let cached_tasks_file = Arc::new(synced_tasks_file);

        // Verify T01 is overridden by scheduler
        let t01 = cached_tasks_file
            .find_leaf("T01")
            .expect("T01 should exist");
        assert_eq!(
            t01.status,
            TaskStatus::Done,
            "T01 should be Done (scheduler override)"
        );

        // Verify T02 preserves its Blocked status from file (scheduler doesn't track it yet)
        let t02 = cached_tasks_file
            .find_leaf("T02")
            .expect("T02 should exist");
        assert_eq!(
            t02.status,
            TaskStatus::Blocked,
            "T02 should preserve Blocked from file"
        );
    }

    #[test]
    fn test_prompt_config_propagation() {
        // Test: Verify that prompt.prefix, prompt.suffix, and prompt.system
        // from .ralph.toml are correctly propagated to Orchestrator struct
        use crate::shared::file_config::{PromptConfig, TaskConfig, UiConfig};

        let file_config = FileConfig {
            prompt: PromptConfig {
                prefix: Some("PREFIX TEXT".to_string()),
                suffix: Some("SUFFIX TEXT".to_string()),
                system: Some("CUSTOM SYSTEM PROMPT".to_string()),
            },
            ui: UiConfig::default(),
            task: TaskConfig::default(),
        };

        let config = ResolvedConfig {
            workers: 2,
            max_retries: 3,
            model: None,
            worktree_prefix: None,
            verbose: false,
            resume: false,
            dry_run: false,
            no_merge: false,
            max_cost: None,
            timeout: None,
            task_filter: None,
            conflict_resolution_model: "opus".to_string(),
            review_model: "opus".to_string(),
            watchdog_interval_secs: 30,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            merge_timeout: None,
        };

        let orch = Orchestrator::new(config, &file_config, PathBuf::from("/tmp/test"))
            .expect("Failed to create orchestrator");

        // Verify custom system prompt is prepended to base template
        assert!(
            orch.system_prompt.starts_with("CUSTOM SYSTEM PROMPT"),
            "Custom system prompt should be prepended"
        );
        assert!(
            orch.system_prompt.contains("# Development Agent"),
            "Base template should be included"
        );

        // Verify prefix and suffix are populated
        assert_eq!(
            orch.prompt_prefix,
            Some("PREFIX TEXT".to_string()),
            "Prefix should be populated"
        );
        assert_eq!(
            orch.prompt_suffix,
            Some("SUFFIX TEXT".to_string()),
            "Suffix should be populated"
        );
    }

    #[test]
    fn test_prompt_config_empty_strings_filtered() {
        // Test: Verify that empty strings are filtered out and treated as None
        use crate::shared::file_config::{PromptConfig, TaskConfig, UiConfig};

        let file_config = FileConfig {
            prompt: PromptConfig {
                prefix: Some("   ".to_string()),    // Only whitespace
                suffix: Some("".to_string()),       // Empty string
                system: Some("  \n  ".to_string()), // Only whitespace
            },
            ui: UiConfig::default(),
            task: TaskConfig::default(),
        };

        let config = ResolvedConfig {
            workers: 2,
            max_retries: 3,
            model: None,
            worktree_prefix: None,
            verbose: false,
            resume: false,
            dry_run: false,
            no_merge: false,
            max_cost: None,
            timeout: None,
            task_filter: None,
            conflict_resolution_model: "opus".to_string(),
            review_model: "opus".to_string(),
            watchdog_interval_secs: 30,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            merge_timeout: None,
        };

        let orch = Orchestrator::new(config, &file_config, PathBuf::from("/tmp/test"))
            .expect("Failed to create orchestrator");

        // Verify empty/whitespace strings are filtered
        assert_eq!(
            orch.prompt_prefix, None,
            "Whitespace-only prefix should be filtered"
        );
        assert_eq!(orch.prompt_suffix, None, "Empty suffix should be filtered");
        // System prompt should not have the whitespace prepended
        assert!(
            !orch.system_prompt.starts_with("  \n  "),
            "Whitespace-only system prompt should be filtered"
        );
    }

    #[test]
    fn test_prompt_config_none_values() {
        // Test: Verify that None values in prompt config work correctly
        use crate::shared::file_config::{PromptConfig, TaskConfig, UiConfig};

        let file_config = FileConfig {
            prompt: PromptConfig {
                prefix: None,
                suffix: None,
                system: None,
            },
            ui: UiConfig::default(),
            task: TaskConfig::default(),
        };

        let config = ResolvedConfig {
            workers: 2,
            max_retries: 3,
            model: None,
            worktree_prefix: None,
            verbose: false,
            resume: false,
            dry_run: false,
            no_merge: false,
            max_cost: None,
            timeout: None,
            task_filter: None,
            conflict_resolution_model: "opus".to_string(),
            review_model: "opus".to_string(),
            watchdog_interval_secs: 30,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            merge_timeout: None,
        };

        let orch = Orchestrator::new(config, &file_config, PathBuf::from("/tmp/test"))
            .expect("Failed to create orchestrator");

        // Verify None values remain None
        assert_eq!(orch.prompt_prefix, None, "Prefix should be None");
        assert_eq!(orch.prompt_suffix, None, "Suffix should be None");
        // System prompt should be just the base template (before ---\n\n# Your Task)
        assert!(
            orch.system_prompt.contains("# Development Agent"),
            "Base template should be present"
        );
        assert!(
            !orch.system_prompt.starts_with("\n\n"),
            "Should not have leading newlines from empty custom system"
        );
    }

    #[test]
    fn test_mcp_session_registration_per_worker() {
        // Verify the pattern used in assignment.rs: each worker gets its own session
        // with a worktree-scoped tasks_path
        use crate::commands::mcp::session::SessionRegistry;
        use std::sync::Arc;

        let registry = Arc::new(SessionRegistry::new());

        // Simulate 3 workers with different worktree paths
        let worker_sessions: Vec<(u32, String)> = (1..=3)
            .map(|id| {
                let worktree_tasks =
                    PathBuf::from(format!("/tmp/project-ralph-task-T0{id}/.ralph/tasks.yml"));
                let session_id = registry.create_session(worktree_tasks, true);
                (id, session_id)
            })
            .collect();

        // Verify all sessions are registered
        assert_eq!(registry.session_count(), 3);

        // Verify each session has correct tasks_path
        for (id, session_id) in &worker_sessions {
            let state = registry
                .validate_session(session_id)
                .expect("Session should exist");
            let expected_path =
                PathBuf::from(format!("/tmp/project-ralph-task-T0{id}/.ralph/tasks.yml"));
            assert_eq!(state.tasks_path, expected_path);
        }

        // Verify sessions have unique IDs
        let ids: Vec<&String> = worker_sessions.iter().map(|(_, id)| id).collect();
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
    }

    #[test]
    fn test_mcp_worktree_tasks_path_resolution() {
        // Verify the strip_prefix + join pattern from assignment.rs
        let project_root = PathBuf::from("/home/user/project");
        let tasks_path = project_root.join(".ralph/tasks.yml");
        let worktree_path = PathBuf::from("/home/user/project-ralph-task-T01");

        // This is the pattern used in assignment.rs to compute worktree-scoped tasks_path
        let relative = tasks_path
            .strip_prefix(&project_root)
            .unwrap_or(&tasks_path);
        let worktree_tasks_path = worktree_path.join(relative);

        assert_eq!(
            worktree_tasks_path,
            PathBuf::from("/home/user/project-ralph-task-T01/.ralph/tasks.yml")
        );
    }
}
