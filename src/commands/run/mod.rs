mod args;
mod config;
mod event_formatting;
mod events;
mod formatting_helpers;
mod once;
pub(crate) mod output;
mod promise;
mod prompt;
pub(crate) mod runner;
pub(crate) mod state;
mod tool_formatting;
pub(crate) mod ui;

pub use args::RunArgs;
pub(crate) use once::{READONLY_TOOLS, RunOnceOptions, run_once};

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use tokio::signal;

use crate::shared::error::{RalphError, Result};
use crate::updater::UpdateState;
use crate::updater::version_checker::UpdateInfo;

use config::Config;
use events::InputThread;
use output::OutputFormatter;
use promise::find_promise;
use prompt::build_system_prompt;
use runner::{ClaudeEvent, ClaudeRunner};
use state::StateManager;
use ui::{StatusData, StatusTerminal};

fn build_task_progress(summary: &crate::shared::progress::ProgressSummary) -> output::TaskProgress {
    let current = crate::shared::progress::current_task(summary);
    output::TaskProgress {
        total: summary.total(),
        done: summary.done,
        in_progress: summary.in_progress,
        blocked: summary.blocked,
        todo: summary.todo,
        current_task_id: current.map(|t| t.id.clone()),
        current_task_name: current.map(|t| t.name.clone()),
        current_task_component: current.map(|t| t.component.clone()),
    }
}

/// Load progress from either .ralph/tasks.yml or PROGRESS.md based on file extension.
fn load_progress_auto(path: &std::path::Path) -> Result<crate::shared::progress::ProgressSummary> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext == "yml" || ext == "yaml" {
        let tf = crate::shared::tasks::TasksFile::load(path)?;
        Ok(tf.to_summary())
    } else {
        crate::shared::progress::load_progress(path)
    }
}

/// Shared state passed between execute() helper functions.
#[derive(Clone)]
struct SharedState {
    formatter: Arc<Mutex<OutputFormatter>>,
    status_terminal: Arc<Mutex<StatusTerminal>>,
    update_info: Arc<Mutex<Option<UpdateInfo>>>,
    update_state: Arc<AtomicU8>,
    resize_flag: Arc<AtomicBool>,
    update_trigger: Arc<AtomicBool>,
    refresh_flag: Arc<AtomicBool>,
}

impl SharedState {
    /// Build StatusData with update_info and update_state populated.
    fn build_status(&self) -> StatusData {
        let mut status = self.formatter.lock().unwrap().get_status();
        status.update_info = self.update_info.lock().unwrap().clone();
        status.update_state = UpdateState::from_u8(self.update_state.load(Ordering::SeqCst));
        status
    }

    /// Handle resize if flagged, then update status bar.
    fn update_status_bar(&self) -> Result<()> {
        let status = self.build_status();
        let mut term = self.status_terminal.lock().unwrap();
        if self.resize_flag.swap(false, Ordering::SeqCst) {
            let _ = term.handle_resize(&status);
        }
        term.update(&status)?;
        Ok(())
    }
}

/// Setup signal handlers for graceful shutdown (SIGINT + SIGTERM).
fn setup_signals(shutdown: &Arc<AtomicBool>) {
    {
        let shutdown_ctrlc = shutdown.clone();
        tokio::spawn(async move {
            let _ = signal::ctrl_c().await;
            shutdown_ctrlc.store(true, Ordering::SeqCst);
        });
    }

    #[cfg(unix)]
    {
        let shutdown_term = shutdown.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                sigterm.recv().await;
                shutdown_term.store(true, Ordering::SeqCst);
            }
        });
    }
}

/// Initialize OutputFormatter with config defaults.
fn setup_formatter(config: &Config) -> Arc<Mutex<OutputFormatter>> {
    let formatter = Arc::new(Mutex::new(OutputFormatter::new(config.use_nerd_font)));
    {
        let mut fmt = formatter.lock().unwrap();
        fmt.set_min_iterations(config.min_iterations);
        fmt.set_max_iterations(config.max_iterations);
    }
    formatter
}

/// Pre-load task progress so progress bar is visible from first iteration.
/// Returns the initial mtime for periodic refresh tracking.
fn init_progress(
    config: &Config,
    formatter: &Arc<Mutex<OutputFormatter>>,
) -> Option<std::time::SystemTime> {
    let progress_file = config.progress_file.as_ref()?;
    let summary = load_progress_auto(progress_file).ok()?;
    let tp = build_task_progress(&summary);
    let mut fmt = formatter.lock().unwrap();
    fmt.set_initial_done_count(summary.done);
    fmt.set_task_progress(Some(tp));
    std::fs::metadata(progress_file)
        .ok()
        .and_then(|m| m.modified().ok())
}

/// Initialize StatusTerminal — use 3-line height in task continue mode.
fn setup_terminal(config: &Config) -> Result<Arc<Mutex<StatusTerminal>>> {
    let terminal = if config.progress_file.is_some() {
        StatusTerminal::with_height(config.use_nerd_font, 3)?
    } else {
        StatusTerminal::new(config.use_nerd_font)?
    };
    Ok(Arc::new(Mutex::new(terminal)))
}

/// Show shutdown message, save state, cleanup terminal, print interrupted stats.
fn cleanup_interrupted(shared: &SharedState, state_manager: &StateManager) -> Result<()> {
    let mut term = shared.status_terminal.lock().unwrap();
    term.show_shutting_down()?;
    term.cleanup()?;
    let lines = shared
        .formatter
        .lock()
        .unwrap()
        .format_interrupted(state_manager.iteration());
    term.print_lines(&lines)?;
    Ok(())
}

/// Print iteration header and update the status bar.
fn print_iteration_header(shared: &SharedState) -> Result<()> {
    let header_lines = shared.formatter.lock().unwrap().format_iteration_header();
    let status = shared.build_status();
    let mut term = shared.status_terminal.lock().unwrap();
    if shared.resize_flag.swap(false, Ordering::SeqCst) {
        let _ = term.handle_resize(&status);
    }
    term.print_lines(&header_lines)?;
    term.update(&status)?;
    Ok(())
}

/// Create a ClaudeRunner for the current iteration.
fn build_runner(config: &Config, state_manager: &StateManager) -> ClaudeRunner {
    let system_prompt = build_system_prompt(
        config.system_prompt_template.as_deref(),
        state_manager.iteration(),
        &config.completion_promise,
        state_manager.min_iterations(),
        config.max_iterations,
    );
    let prompt = config.prompt.clone();
    if state_manager.iteration() == 1 || !config.continue_session {
        ClaudeRunner::new(prompt, system_prompt)
    } else {
        ClaudeRunner::for_continuation(prompt, system_prompt)
    }
}

/// Event callback body: format event → update terminal.
fn handle_event(event: &ClaudeEvent, shared: &SharedState, shutdown: &Arc<AtomicBool>) {
    let (lines, mut status) = {
        let mut fmt = shared.formatter.lock().unwrap();
        let lines = fmt.format_event(event);
        let status = fmt.get_status();
        (lines, status)
    };
    status.update_info = shared.update_info.lock().unwrap().clone();
    status.update_state = UpdateState::from_u8(shared.update_state.load(Ordering::SeqCst));
    let mut term = shared.status_terminal.lock().unwrap();
    if shared.resize_flag.swap(false, Ordering::SeqCst) {
        let _ = term.handle_resize(&status);
    }
    for line in &lines {
        let _ = term.print_line(line);
    }
    if shutdown.load(Ordering::SeqCst) {
        let _ = term.show_shutting_down();
    } else {
        let _ = term.update(&status);
    }
}

/// Idle callback body: handle update trigger, refresh progress, update status bar.
fn handle_idle(
    shared: &SharedState,
    progress_file: &Option<std::path::PathBuf>,
    last_progress_mtime: &mut Option<std::time::SystemTime>,
    last_mtime_check: &mut std::time::Instant,
) {
    if shared.update_trigger.swap(false, Ordering::SeqCst) {
        let state_for_task = shared.update_state.clone();
        tokio::task::spawn_blocking(move || {
            crate::updater::update_in_background(state_for_task);
        });
    }

    let reload_result = check_progress_reload(
        progress_file,
        &shared.refresh_flag,
        last_progress_mtime,
        last_mtime_check,
    );

    let mut status = {
        let mut fmt = shared.formatter.lock().unwrap();
        if let Some(summary) = reload_result {
            fmt.set_task_progress(Some(build_task_progress(&summary)));
        }
        fmt.get_status()
    };
    status.update_info = shared.update_info.lock().unwrap().clone();
    status.update_state = UpdateState::from_u8(shared.update_state.load(Ordering::SeqCst));
    let mut term = shared.status_terminal.lock().unwrap();
    if shared.resize_flag.swap(false, Ordering::SeqCst) {
        let _ = term.handle_resize(&status);
    }
    let _ = term.update(&status);
}

/// Check if progress file should be reloaded (manual 'r' key or 15s mtime poll).
fn check_progress_reload(
    progress_file: &Option<std::path::PathBuf>,
    refresh_flag: &Arc<AtomicBool>,
    last_progress_mtime: &mut Option<std::time::SystemTime>,
    last_mtime_check: &mut std::time::Instant,
) -> Option<crate::shared::progress::ProgressSummary> {
    let progress_path = progress_file.as_ref()?;
    let mut should_reload = refresh_flag.swap(false, Ordering::SeqCst);

    if !should_reload {
        let now = std::time::Instant::now();
        if now.duration_since(*last_mtime_check).as_secs() >= 15 {
            *last_mtime_check = now;
            if let Ok(meta) = std::fs::metadata(progress_path)
                && let Ok(mtime) = meta.modified()
                && last_progress_mtime.as_ref() != Some(&mtime)
            {
                *last_progress_mtime = Some(mtime);
                should_reload = true;
            }
        }
    }

    if should_reload {
        load_progress_auto(progress_path).ok()
    } else {
        None
    }
}

/// Adaptive iterations: re-read progress and adjust min/max + task progress.
fn update_adaptive_iterations(
    config: &Config,
    state_manager: &mut StateManager,
    shared: &SharedState,
) {
    if let Some(ref progress_file) = config.progress_file
        && let Ok(summary) = load_progress_auto(progress_file)
    {
        let remaining = summary.remaining() as u32;
        let new_min = state_manager.iteration() + remaining;
        state_manager.set_min_iterations(new_min);
        state_manager.set_max_iterations(new_min + 5);

        let tp = build_task_progress(&summary);
        let mut fmt = shared.formatter.lock().unwrap();
        fmt.set_min_iterations(state_manager.min_iterations());
        fmt.set_max_iterations(new_min + 5);
        fmt.set_task_progress(Some(tp));
        fmt.record_iteration_end();
    }
}

/// Check for completion promise and handle it. Returns Ok(true) if completed.
fn check_promise(
    last_message: &Option<String>,
    shared: &SharedState,
    state_manager: &StateManager,
) -> Result<bool> {
    let text = match last_message {
        Some(t) => t,
        None => return Ok(false),
    };
    if !find_promise(text, state_manager.completion_promise()) {
        return Ok(false);
    }
    if state_manager.can_accept_promise() {
        let mut term = shared.status_terminal.lock().unwrap();
        term.cleanup()?;
        let lines = shared.formatter.lock().unwrap().format_stats(
            state_manager.iteration(),
            true,
            state_manager.completion_promise(),
        );
        term.print_lines(&lines)?;
        return Ok(true);
    }
    // Promise found but min iterations not reached
    let msg = format!(
        "\n[Promise found but ignoring - need {} more iteration(s) to reach minimum of {}]",
        state_manager.min_iterations() - state_manager.iteration(),
        state_manager.min_iterations()
    );
    shared.status_terminal.lock().unwrap().print_line(&msg)?;
    Ok(false)
}

/// Handle max iterations reached: cleanup and return error.
fn handle_max_iterations(shared: &SharedState, state_manager: &StateManager) -> Result<()> {
    let mut term = shared.status_terminal.lock().unwrap();
    term.cleanup()?;
    let lines = shared.formatter.lock().unwrap().format_stats(
        state_manager.iteration(),
        false,
        state_manager.completion_promise(),
    );
    term.print_lines(&lines)?;
    Ok(())
}

/// Initialize all shared state needed for the run loop.
fn setup_shared_state(config: &Config, shutdown: &Arc<AtomicBool>) -> Result<SharedState> {
    let version_checker = crate::updater::VersionChecker::new();
    let update_info = version_checker.update_info();
    let update_state = version_checker.update_state();
    version_checker.spawn_checker(shutdown.clone());

    let formatter = setup_formatter(config);
    Ok(SharedState {
        formatter,
        status_terminal: setup_terminal(config)?,
        update_info,
        update_state,
        resize_flag: Arc::new(AtomicBool::new(false)),
        update_trigger: Arc::new(AtomicBool::new(false)),
        refresh_flag: Arc::new(AtomicBool::new(false)),
    })
}

/// Run a single Claude iteration with event and idle callbacks.
async fn run_iteration(
    runner: ClaudeRunner,
    shutdown: &Arc<AtomicBool>,
    shared: &SharedState,
    config: &Config,
    last_progress_mtime: &mut Option<std::time::SystemTime>,
    last_mtime_check: &mut std::time::Instant,
) -> Result<Option<String>> {
    let shared_ev = shared.clone();
    let shutdown_ev = shutdown.clone();
    let shared_idle = shared.clone();
    let pf_idle = config.progress_file.clone();

    runner
        .run(
            shutdown.clone(),
            |event| handle_event(event, &shared_ev, &shutdown_ev),
            || {
                handle_idle(
                    &shared_idle,
                    &pf_idle,
                    last_progress_mtime,
                    last_mtime_check,
                )
            },
        )
        .await
}

/// Handle shutdown: save state, cleanup terminal.
async fn handle_shutdown(shared: &SharedState, state_manager: &mut StateManager) -> Result<()> {
    state_manager.save().await?;
    cleanup_interrupted(shared, state_manager)
}

pub async fn execute(args: RunArgs) -> Result<()> {
    let config = Config::build(args)?;
    let mut state_manager = StateManager::new(&config);
    let shutdown = Arc::new(AtomicBool::new(false));
    setup_signals(&shutdown);

    let shared = setup_shared_state(&config, &shutdown)?;
    let mut last_progress_mtime = init_progress(&config, &shared.formatter);
    let mut last_mtime_check = std::time::Instant::now();

    let input_thread = InputThread::spawn(
        shutdown.clone(),
        shared.resize_flag.clone(),
        shared.update_state.clone(),
        shared.update_trigger.clone(),
        shared.refresh_flag.clone(),
    );

    state_manager.save().await?;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            handle_shutdown(&shared, &mut state_manager).await?;
            drop(input_thread);
            return Err(RalphError::Interrupted);
        }

        if state_manager.is_max_reached() {
            handle_max_iterations(&shared, &state_manager)?;
            state_manager.cleanup().await?;
            drop(input_thread);
            return Err(RalphError::MaxIterations(state_manager.iteration()));
        }

        state_manager.increment_iteration().await?;
        {
            let mut fmt = shared.formatter.lock().unwrap();
            fmt.set_iteration(state_manager.iteration());
            fmt.start_iteration();
        }
        print_iteration_header(&shared)?;

        let runner = build_runner(&config, &state_manager);
        if shutdown.load(Ordering::SeqCst) {
            continue;
        }

        let run_result = run_iteration(
            runner,
            &shutdown,
            &shared,
            &config,
            &mut last_progress_mtime,
            &mut last_mtime_check,
        )
        .await;

        let last_message = match run_result {
            Ok(msg) => msg,
            Err(RalphError::Interrupted) => {
                handle_shutdown(&shared, &mut state_manager).await?;
                drop(input_thread);
                return Err(RalphError::Interrupted);
            }
            Err(e) => return Err(e),
        };

        update_adaptive_iterations(&config, &mut state_manager, &shared);
        shared.update_status_bar()?;

        if check_promise(&last_message, &shared, &state_manager)? {
            state_manager.cleanup().await?;
            input_thread.stop();
            return Ok(());
        }

        if shutdown.load(Ordering::SeqCst) {
            handle_shutdown(&shared, &mut state_manager).await?;
            drop(input_thread);
            return Err(RalphError::Interrupted);
        }
    }
}
