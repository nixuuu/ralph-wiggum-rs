mod args;
mod config;
mod events;
mod once;
pub(crate) mod output;
mod prompt;
pub(crate) mod runner;
pub(crate) mod state;
pub(crate) mod ui;

pub use args::RunArgs;
pub(crate) use once::{RunOnceOptions, run_once};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::signal;

use crate::shared::error::{RalphError, Result};
use crate::updater::UpdateState;

use config::Config;
use events::InputThread;
use output::{OutputFormatter, find_promise};
use prompt::build_system_prompt;
use runner::ClaudeRunner;
use state::StateManager;
use ui::StatusTerminal;

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

pub async fn execute(args: RunArgs) -> Result<()> {
    // Build config
    let config = Config::build(args)?;

    // Initialize state manager
    let mut state_manager = StateManager::new(&config);

    // Setup signal handler for graceful shutdown
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    tokio::spawn(async move {
        let _ = signal::ctrl_c().await;
        shutdown_clone.store(true, Ordering::SeqCst);
    });

    // Initialize background version checker (non-blocking)
    let version_checker = crate::updater::VersionChecker::new();
    let update_info = version_checker.update_info();
    let update_state = version_checker.update_state();
    version_checker.spawn_checker(shutdown.clone());

    // Update trigger flag: set by InputThread on Ctrl+U, consumed by on_idle to spawn_blocking
    let update_trigger = Arc::new(AtomicBool::new(false));

    // Initialize output formatter
    let formatter = Arc::new(Mutex::new(OutputFormatter::new(config.use_nerd_font)));
    {
        let mut fmt = formatter.lock().unwrap();
        fmt.set_min_iterations(config.min_iterations);
        fmt.set_max_iterations(config.max_iterations);
    }

    // Track PROGRESS.md mtime for auto-refresh in idle callback
    let mut last_progress_mtime: Option<std::time::SystemTime> = None;
    let mut last_mtime_check = std::time::Instant::now();

    // Pre-load task progress so progress bar is visible from first iteration
    if let Some(ref progress_file) = config.progress_file
        && let Ok(summary) = crate::shared::progress::load_progress(progress_file)
    {
        let tp = build_task_progress(&summary);
        let mut fmt = formatter.lock().unwrap();
        fmt.set_initial_done_count(summary.done);
        fmt.set_task_progress(Some(tp));
        if let Ok(meta) = std::fs::metadata(progress_file) {
            last_progress_mtime = meta.modified().ok();
        }
    }

    // Initialize status terminal (enables raw mode)
    // Use 3-line height when progress_file is set (task continue mode)
    let status_terminal = if config.progress_file.is_some() {
        Arc::new(Mutex::new(StatusTerminal::with_height(
            config.use_nerd_font,
            3,
        )?))
    } else {
        Arc::new(Mutex::new(StatusTerminal::new(config.use_nerd_font)?))
    };

    // Resize flag for terminal resize detection
    let resize_flag = Arc::new(AtomicBool::new(false));

    // Refresh flag for manual PROGRESS.md re-read (r key)
    let refresh_flag = Arc::new(AtomicBool::new(false));

    // Spawn dedicated OS thread for keyboard input (after raw mode is enabled)
    let input_thread = InputThread::spawn(
        shutdown.clone(),
        resize_flag.clone(),
        update_state.clone(),
        update_trigger.clone(),
        refresh_flag.clone(),
    );

    // Save initial state
    state_manager.save().await?;

    // Main loop
    loop {
        // Check shutdown signal
        if shutdown.load(Ordering::SeqCst) {
            status_terminal.lock().unwrap().show_shutting_down()?;
            state_manager.save().await?;
            {
                let mut term = status_terminal.lock().unwrap();
                term.cleanup()?;
                let lines = formatter
                    .lock()
                    .unwrap()
                    .format_interrupted(state_manager.iteration());
                term.print_lines(&lines)?;
            }
            drop(input_thread);
            return Err(RalphError::Interrupted);
        }

        // Check max iterations (before incrementing)
        if state_manager.is_max_reached() {
            {
                let mut term = status_terminal.lock().unwrap();
                term.cleanup()?;
                let lines = formatter.lock().unwrap().format_stats(
                    state_manager.iteration(),
                    false,
                    state_manager.completion_promise(),
                );
                term.print_lines(&lines)?;
            }
            // Cleanup state file on max iterations
            state_manager.cleanup().await?;
            drop(input_thread);
            return Err(RalphError::MaxIterations(state_manager.iteration()));
        }

        // Increment iteration
        state_manager.increment_iteration().await?;
        {
            let mut fmt = formatter.lock().unwrap();
            fmt.set_iteration(state_manager.iteration());
            fmt.start_iteration();
        }

        // Print iteration header and update status bar
        {
            let header_lines = formatter.lock().unwrap().format_iteration_header();
            let mut status = formatter.lock().unwrap().get_status();
            status.update_info = update_info.lock().unwrap().clone();
            status.update_state = UpdateState::from_u8(update_state.load(Ordering::SeqCst));
            let mut term = status_terminal.lock().unwrap();
            if resize_flag.swap(false, Ordering::SeqCst) {
                let _ = term.handle_resize(&status);
            }
            term.print_lines(&header_lines)?;
            term.update(&status)?;
        }

        // Build system prompt with optional custom prefix
        let system_prompt = build_system_prompt(
            config.system_prompt_template.as_deref(),
            state_manager.iteration(),
            &config.completion_promise,
            state_manager.min_iterations(),
            config.max_iterations,
        );

        // Create runner - first iteration starts new session, others may continue
        // When continue_session is set, subsequent iterations continue the conversation
        // User prompt is passed as positional argument, system prompt via --append-system-prompt
        let runner = if state_manager.iteration() == 1 || !config.continue_session {
            ClaudeRunner::new(config.prompt.clone(), system_prompt)
        } else {
            ClaudeRunner::for_continuation(config.prompt.clone(), system_prompt)
        };

        // Check shutdown before spawning claude process
        if shutdown.load(Ordering::SeqCst) {
            continue; // Will be caught by shutdown check at top of loop
        }

        // Clone Arc for use in event closure
        let formatter_clone = formatter.clone();
        let status_terminal_clone = status_terminal.clone();
        let shutdown_for_callback = shutdown.clone();
        let update_info_clone = update_info.clone();
        let resize_flag_clone = resize_flag.clone();
        let update_state_event = update_state.clone();

        // Clone Arc for use in idle closure
        let resize_flag_idle = resize_flag.clone();
        let formatter_idle = formatter.clone();
        let status_terminal_idle = status_terminal.clone();
        let update_info_idle = update_info.clone();
        let update_state_idle = update_state.clone();
        let update_trigger_idle = update_trigger.clone();
        let refresh_flag_idle = refresh_flag.clone();
        let progress_file_idle = config.progress_file.clone();

        // Run claude with consolidated lock acquisitions in callback
        let run_result = runner
            .run(
                shutdown.clone(),
                |event| {
                    // Lock formatter once: format event + get status
                    let (lines, mut status) = {
                        let mut fmt = formatter_clone.lock().unwrap();
                        let lines = fmt.format_event(event);
                        let status = fmt.get_status();
                        (lines, status)
                    };
                    status.update_info = update_info_clone.lock().unwrap().clone();
                    status.update_state =
                        UpdateState::from_u8(update_state_event.load(Ordering::SeqCst));
                    // Lock terminal once: print all lines + update status bar
                    {
                        let mut term = status_terminal_clone.lock().unwrap();
                        // Handle resize: clear viewport and redraw status bar
                        if resize_flag_clone.swap(false, Ordering::SeqCst) {
                            let _ = term.handle_resize(&status);
                        }
                        for line in &lines {
                            let _ = term.print_line(line);
                        }
                        if shutdown_for_callback.load(Ordering::SeqCst) {
                            let _ = term.show_shutting_down();
                        } else {
                            let _ = term.update(&status);
                        }
                    }
                },
                || {
                    // Check if Ctrl+U triggered an update
                    if update_trigger_idle.swap(false, Ordering::SeqCst) {
                        let state_for_task = update_state_idle.clone();
                        tokio::task::spawn_blocking(move || {
                            crate::updater::update_in_background(state_for_task);
                        });
                    }

                    // Progress refresh: manual 'r' key OR periodic mtime check (15s)
                    let reload_result = if let Some(ref progress_path) = progress_file_idle {
                        let mut should_reload = refresh_flag_idle.swap(false, Ordering::SeqCst);

                        if !should_reload {
                            let now = std::time::Instant::now();
                            if now.duration_since(last_mtime_check).as_secs() >= 15 {
                                last_mtime_check = now;
                                if let Ok(meta) = std::fs::metadata(progress_path)
                                    && let Ok(mtime) = meta.modified()
                                    && last_progress_mtime.as_ref() != Some(&mtime)
                                {
                                    last_progress_mtime = Some(mtime);
                                    should_reload = true;
                                }
                            }
                        }

                        if should_reload {
                            crate::shared::progress::load_progress(progress_path).ok()
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Consolidated lock: set progress (if reloaded) + get status
                    let mut status = {
                        let mut fmt = formatter_idle.lock().unwrap();
                        if let Some(summary) = reload_result {
                            fmt.set_task_progress(Some(build_task_progress(&summary)));
                        }
                        fmt.get_status()
                    };
                    status.update_info = update_info_idle.lock().unwrap().clone();
                    status.update_state =
                        UpdateState::from_u8(update_state_idle.load(Ordering::SeqCst));
                    let mut term = status_terminal_idle.lock().unwrap();
                    if resize_flag_idle.swap(false, Ordering::SeqCst) {
                        let _ = term.handle_resize(&status);
                    }
                    let _ = term.update(&status);
                },
            )
            .await;

        // Handle interrupted - do cleanup before returning error
        let last_message = match run_result {
            Ok(msg) => msg,
            Err(RalphError::Interrupted) => {
                status_terminal.lock().unwrap().show_shutting_down()?;
                state_manager.save().await?;
                {
                    let mut term = status_terminal.lock().unwrap();
                    term.cleanup()?;
                    let lines = formatter
                        .lock()
                        .unwrap()
                        .format_interrupted(state_manager.iteration());
                    term.print_lines(&lines)?;
                }
                drop(input_thread);
                return Err(RalphError::Interrupted);
            }
            Err(e) => return Err(e),
        };

        // Adaptive iterations: re-read PROGRESS.md and adjust min/max + task progress
        if let Some(ref progress_file) = config.progress_file
            && let Ok(summary) = crate::shared::progress::load_progress(progress_file)
        {
            let remaining = summary.remaining() as u32;
            let new_min = state_manager.iteration() + remaining;
            state_manager.set_min_iterations(new_min);
            state_manager.set_max_iterations(new_min + 5);

            let tp = build_task_progress(&summary);
            {
                let mut fmt = formatter.lock().unwrap();
                fmt.set_min_iterations(state_manager.min_iterations());
                fmt.set_max_iterations(new_min + 5);
                fmt.set_task_progress(Some(tp));
                fmt.record_iteration_end();
            }
        }

        // Update status bar after iteration completes
        {
            let mut status = formatter.lock().unwrap().get_status();
            status.update_info = update_info.lock().unwrap().clone();
            status.update_state = UpdateState::from_u8(update_state.load(Ordering::SeqCst));
            let mut term = status_terminal.lock().unwrap();
            if resize_flag.swap(false, Ordering::SeqCst) {
                let _ = term.handle_resize(&status);
            }
            term.update(&status)?;
        }

        // Check for completion promise in last message
        if let Some(text) = last_message
            && find_promise(&text, state_manager.completion_promise())
        {
            if state_manager.can_accept_promise() {
                {
                    let mut term = status_terminal.lock().unwrap();
                    term.cleanup()?;
                    let lines = formatter.lock().unwrap().format_stats(
                        state_manager.iteration(),
                        true,
                        state_manager.completion_promise(),
                    );
                    term.print_lines(&lines)?;
                }
                // Cleanup state file on successful completion
                state_manager.cleanup().await?;
                input_thread.stop();
                return Ok(());
            } else {
                // Promise found but min iterations not reached - continue
                let msg = format!(
                    "\n[Promise found but ignoring - need {} more iteration(s) to reach minimum of {}]",
                    state_manager.min_iterations() - state_manager.iteration(),
                    state_manager.min_iterations()
                );
                status_terminal.lock().unwrap().print_line(&msg)?;
            }
        }

        // Check shutdown after iteration
        if shutdown.load(Ordering::SeqCst) {
            status_terminal.lock().unwrap().show_shutting_down()?;
            state_manager.save().await?;
            {
                let mut term = status_terminal.lock().unwrap();
                term.cleanup()?;
                let lines = formatter
                    .lock()
                    .unwrap()
                    .format_interrupted(state_manager.iteration());
                term.print_lines(&lines)?;
            }
            drop(input_thread);
            return Err(RalphError::Interrupted);
        }
    }
}
