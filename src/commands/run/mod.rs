pub(crate) mod app;
mod args;
mod config;
mod event_formatting;
/// Legacy inline mode support — używane tylko przez once.rs dla task commands.
/// Główny run mode używa app::RunApp z fullscreen TUI.
mod events;
mod formatting_helpers;
mod once;
pub(crate) mod output;
mod promise;
mod prompt;
pub(crate) mod runner;
pub(crate) mod runner_reader;
pub(crate) mod runner_types;
pub(crate) mod state;
mod summary;
/// Legacy inline mode support — używane tylko przez once.rs dla task commands.
/// Główny run mode używa app::RunApp z fullscreen TUI.
mod ui;

#[cfg(test)]
mod responsive_tests;

#[cfg(test)]
mod run_mode_integration_tests;

pub use args::RunArgs;
pub(crate) use once::{DANGEROUS_TOOLS, RunOnceOptions, run_once};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::signal;

use crate::shared::error::{RalphError, Result};

use app::RunApp;
use config::Config;
use promise::find_promise;
use prompt::build_system_prompt;
use runner::{ClaudeEvent, ClaudeRunner};
use state::StateManager;
use summary::{SessionResult, print_final_summary};

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
    run_app: Arc<Mutex<RunApp>>,
}

impl SharedState {
    /// Update RunApp status data (refresh from formatter).
    fn update_status(&self) {
        let mut app = self.run_app.lock().expect("run_app: mutex poisoned");
        app.update_status();
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

/// Initialize RunApp with config defaults.
fn setup_run_app(config: &Config) -> Arc<Mutex<RunApp>> {
    let mut app = RunApp::new(
        &config.command_name,
        "claude-sonnet-4-5", // TODO: read from config/env
        config.use_nerd_font,
        5000,              // ring buffer capacity
        !config.no_splash, // show_splash (negacja no_splash)
    );
    app.output_formatter
        .set_min_iterations(config.min_iterations);
    app.output_formatter
        .set_max_iterations(config.max_iterations);

    // Jeśli jest ustawiony progress_file (task continue), włącz sidebar z task tree
    if let Some(ref tasks_path) = config.progress_file {
        app.set_tasks_file(tasks_path.clone());
    }

    Arc::new(Mutex::new(app))
}

/// Pre-load task progress into RunApp so progress bar is visible from first iteration.
/// Returns the initial mtime for periodic refresh tracking.
fn init_progress(config: &Config, run_app: &Arc<Mutex<RunApp>>) -> Option<std::time::SystemTime> {
    let progress_file = config.progress_file.as_ref()?;
    let summary = load_progress_auto(progress_file).ok()?;
    let tp = build_task_progress(&summary);
    let mut app = run_app.lock().expect("run_app: mutex poisoned");
    app.output_formatter.set_initial_done_count(summary.done);
    app.output_formatter.set_task_progress(Some(tp));
    std::fs::metadata(progress_file)
        .ok()
        .and_then(|m| m.modified().ok())
}

// setup_terminal usunięta — używamy RunApp zamiast StatusTerminal

/// Show interrupted stats in RunApp.
fn cleanup_interrupted(shared: &SharedState, state_manager: &StateManager) {
    let mut app = shared.run_app.lock().expect("run_app: mutex poisoned");
    let lines = app
        .output_formatter
        .format_interrupted(state_manager.iteration());
    app.push_lines(lines);
}

/// Push iteration header to RunApp ring buffer.
fn print_iteration_header(shared: &SharedState) {
    let mut app = shared.run_app.lock().expect("run_app: mutex poisoned");
    let header_lines = app.output_formatter.format_iteration_header();
    app.push_lines(header_lines);
    app.update_status();
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

/// Event callback body: format event → push to ring buffer.
fn handle_event(event: &ClaudeEvent, shared: &SharedState, _shutdown: &Arc<AtomicBool>) {
    let mut app = shared.run_app.lock().expect("run_app: mutex poisoned");
    app.push_event(event);
    app.update_status();
}

/// Idle callback body: periodically reload progress file and refresh status.
fn handle_idle(
    shared: &SharedState,
    progress_file: &Option<std::path::PathBuf>,
    last_progress_mtime: &mut Option<std::time::SystemTime>,
    last_mtime_check: &mut std::time::Instant,
) {
    // Periodyczny reload pliku progress (co 15s lub po zmianie mtime)
    if let Some(summary) =
        check_progress_reload(progress_file, last_progress_mtime, last_mtime_check)
    {
        let tp = build_task_progress(&summary);
        let mut app = shared.run_app.lock().expect("run_app: mutex poisoned");
        app.output_formatter.set_task_progress(Some(tp));
    }
    shared.update_status();
}

/// Check if progress file should be reloaded (15s mtime poll).
fn check_progress_reload(
    progress_file: &Option<std::path::PathBuf>,
    last_progress_mtime: &mut Option<std::time::SystemTime>,
    last_mtime_check: &mut std::time::Instant,
) -> Option<crate::shared::progress::ProgressSummary> {
    let progress_path = progress_file.as_ref()?;
    let now = std::time::Instant::now();
    if now.duration_since(*last_mtime_check).as_secs() < 15 {
        return None;
    }
    *last_mtime_check = now;
    if let Ok(meta) = std::fs::metadata(progress_path)
        && let Ok(mtime) = meta.modified()
        && last_progress_mtime.as_ref() != Some(&mtime)
    {
        *last_progress_mtime = Some(mtime);
        return load_progress_auto(progress_path).ok();
    }
    None
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
        let mut app = shared.run_app.lock().expect("run_app: mutex poisoned");
        app.output_formatter
            .set_min_iterations(state_manager.min_iterations());
        app.output_formatter.set_max_iterations(new_min + 5);
        app.output_formatter.set_task_progress(Some(tp));
        app.output_formatter.record_iteration_end();
    }
}

/// Check for completion promise and handle it. Returns true if completed.
fn check_promise(
    last_message: &Option<String>,
    shared: &SharedState,
    state_manager: &StateManager,
) -> bool {
    let text = match last_message {
        Some(t) => t,
        None => return false,
    };
    if !find_promise(text, state_manager.completion_promise()) {
        return false;
    }
    if state_manager.can_accept_promise() {
        let mut app = shared.run_app.lock().expect("run_app: mutex poisoned");
        let lines = app.output_formatter.format_stats(
            state_manager.iteration(),
            true,
            state_manager.completion_promise(),
        );
        app.push_lines(lines);
        return true;
    }
    // Promise found but min iterations not reached
    let msg = format!(
        "[Promise found but ignoring - need {} more iteration(s) to reach minimum of {}]",
        state_manager.min_iterations() - state_manager.iteration(),
        state_manager.min_iterations()
    );
    let mut app = shared.run_app.lock().expect("run_app: mutex poisoned");
    app.push_lines(vec![ratatui::text::Line::raw(msg)]);
    false
}

/// Handle max iterations reached: push stats to RunApp.
fn handle_max_iterations(shared: &SharedState, state_manager: &StateManager) {
    let mut app = shared.run_app.lock().expect("run_app: mutex poisoned");
    let lines = app.output_formatter.format_stats(
        state_manager.iteration(),
        false,
        state_manager.completion_promise(),
    );
    app.push_lines(lines);
}

/// Initialize all shared state needed for the run loop.
fn setup_shared_state(config: &Config, _shutdown: &Arc<AtomicBool>) -> SharedState {
    let run_app = setup_run_app(config);
    SharedState { run_app }
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

/// Handle shutdown: save state, show interrupted stats.
async fn handle_shutdown(shared: &SharedState, state_manager: &mut StateManager) -> Result<()> {
    state_manager.save().await?;
    cleanup_interrupted(shared, state_manager);
    Ok(())
}

/// Print final session summary to stdout after TUI cleanup.
///
/// IMPORTANT: Must be called AFTER tui_handle.await completes,
/// so that LeaveAlternateScreen has already restored normal terminal.
/// Otherwise println!() output goes to alternate screen and is lost.
fn print_summary_and_exit(
    shared: &SharedState,
    state_manager: &StateManager,
    result: SessionResult,
) {
    let app = shared.run_app.lock().expect("run_app: mutex poisoned");
    let promise = state_manager.completion_promise();
    print_final_summary(
        &app.output_formatter,
        state_manager.iteration(),
        result,
        promise,
    );
}

pub async fn execute(args: RunArgs) -> Result<()> {
    let config = Config::build(args)?;
    let mut state_manager = StateManager::new(&config);
    let shutdown = Arc::new(AtomicBool::new(false));
    setup_signals(&shutdown);

    let shared = setup_shared_state(&config, &shutdown);
    let mut last_progress_mtime = init_progress(&config, &shared.run_app);
    let mut last_mtime_check = std::time::Instant::now();

    // Uruchom fullscreen TUI w tle — run_shared() lockuje Mutex krótkotrwale,
    // pozwalając runner callbackom pushować do ring buffera między renderami.
    // Współdzielona flaga shutdown: SIGINT przerywa zarówno TUI jak i runner.
    let shutdown_tui = shutdown.clone();
    let run_app_clone = shared.run_app.clone();
    let tui_handle = tokio::task::spawn_blocking(move || {
        let mut app = crate::tui::App::new(std::time::Duration::from_millis(100))
            .expect("Failed to initialize TUI")
            .with_shutdown(shutdown_tui.clone());

        let result = app.run_shared(run_app_clone);

        // Gdy TUI kończy (q/Ctrl+C), ustaw shutdown
        shutdown_tui.store(true, Ordering::SeqCst);
        result
    });

    state_manager.save().await?;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            handle_shutdown(&shared, &mut state_manager).await?;
            let _ = tui_handle.await;
            print_summary_and_exit(&shared, &state_manager, SessionResult::Interrupted);
            return Err(RalphError::Interrupted);
        }

        if state_manager.is_max_reached() {
            handle_max_iterations(&shared, &state_manager);
            state_manager.cleanup().await?;
            shutdown.store(true, Ordering::SeqCst);
            let _ = tui_handle.await;
            print_summary_and_exit(&shared, &state_manager, SessionResult::MaxIterations);
            return Err(RalphError::MaxIterations(state_manager.iteration()));
        }

        state_manager.increment_iteration().await?;
        {
            let mut app = shared.run_app.lock().expect("run_app: mutex poisoned");
            app.output_formatter
                .set_iteration(state_manager.iteration());
            app.output_formatter.start_iteration();
            app.header_data.iteration = Some(state_manager.iteration());
            app.header_data.max_iterations = Some(state_manager.max_iterations());
            app.header_data.is_running = true;
        }
        print_iteration_header(&shared);

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
                let _ = tui_handle.await;
                print_summary_and_exit(&shared, &state_manager, SessionResult::Interrupted);
                return Err(RalphError::Interrupted);
            }
            Err(e) => {
                // Cleanup TUI przed propagacją błędu
                shutdown.store(true, Ordering::SeqCst);
                let _ = tui_handle.await;
                return Err(e);
            }
        };

        update_adaptive_iterations(&config, &mut state_manager, &shared);
        shared.update_status();

        if check_promise(&last_message, &shared, &state_manager) {
            state_manager.cleanup().await?;
            // Pozwól TUI pokazać stats przez 2s przed zamknięciem
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            shutdown.store(true, Ordering::SeqCst);
            let _ = tui_handle.await;
            print_summary_and_exit(&shared, &state_manager, SessionResult::PromiseFound);
            return Ok(());
        }

        if shutdown.load(Ordering::SeqCst) {
            handle_shutdown(&shared, &mut state_manager).await?;
            let _ = tui_handle.await;
            print_summary_and_exit(&shared, &state_manager, SessionResult::Interrupted);
            return Err(RalphError::Interrupted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a temporary YAML tasks file with minimal valid content
    fn create_test_tasks_file(path: &std::path::Path) -> std::io::Result<()> {
        let content = r#"default_model: haiku
tasks:
  - id: "1"
    name: "Test task"
    component: "test"
    status: todo
"#;
        std::fs::write(path, content)
    }

    /// Helper: create a temporary PROGRESS.md file with minimal valid content
    fn create_test_progress_file(path: &std::path::Path) -> std::io::Result<()> {
        let content = "- [ ] 1 [test] Test task\n";
        std::fs::write(path, content)
    }

    // Edge cases tests (task 59.3)

    /// Test: plik .yml — ładuje jako tasks (YAML)
    #[test]
    fn test_load_progress_auto_yml_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tasks.yml");
        create_test_tasks_file(&path).unwrap();

        let result = load_progress_auto(&path);
        assert!(result.is_ok(), "Should load .yml file as tasks");
        let summary = result.unwrap();
        assert_eq!(summary.total(), 1, "Should have 1 task from YAML");
    }

    /// Test: plik .yaml — ładuje jako tasks (YAML)
    #[test]
    fn test_load_progress_auto_yaml_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tasks.yaml");
        create_test_tasks_file(&path).unwrap();

        let result = load_progress_auto(&path);
        assert!(result.is_ok(), "Should load .yaml file as tasks");
        let summary = result.unwrap();
        assert_eq!(summary.total(), 1, "Should have 1 task from YAML");
    }

    /// Test: plik .YML (uppercase) — case-sensitive, fallback do markdown parsera
    #[test]
    fn test_load_progress_auto_uppercase_yml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tasks.YML");
        create_test_tasks_file(&path).unwrap();

        let result = load_progress_auto(&path);
        assert!(
            result.is_ok(),
            "Should load .YML file (falls back to markdown)"
        );
        // Case-sensitive: .YML != "yml", so falls back to markdown parser
        // YAML content has no markdown task lines → 0 tasks
        let summary = result.unwrap();
        assert_eq!(
            summary.total(),
            0,
            ".YML should fallback to markdown (0 tasks from YAML content)"
        );
    }

    /// Test: plik .YAML (uppercase) — case-sensitive, fallback do markdown parsera
    #[test]
    fn test_load_progress_auto_uppercase_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tasks.YAML");
        create_test_tasks_file(&path).unwrap();

        let result = load_progress_auto(&path);
        assert!(
            result.is_ok(),
            "Should load .YAML file (falls back to markdown)"
        );
        // Case-sensitive: .YAML != "yaml", so falls back to markdown parser
        let summary = result.unwrap();
        assert_eq!(
            summary.total(),
            0,
            ".YAML should fallback to markdown (0 tasks from YAML content)"
        );
    }

    /// Test: plik .md — ładuje jako progress (Markdown)
    #[test]
    fn test_load_progress_auto_md_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("PROGRESS.md");
        create_test_progress_file(&path).unwrap();

        let result = load_progress_auto(&path);
        assert!(result.is_ok(), "Should load .md file as progress");
        let summary = result.unwrap();
        assert_eq!(summary.total(), 1, "Should have 1 task from markdown");
    }

    /// Test: plik bez rozszerzenia — fallback to markdown parser
    #[test]
    fn test_load_progress_auto_no_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("PROGRESS");
        create_test_progress_file(&path).unwrap();

        let result = load_progress_auto(&path);
        assert!(
            result.is_ok(),
            "Should load file without extension (fallback to markdown)"
        );
        let summary = result.unwrap();
        assert_eq!(summary.total(), 1, "Should parse as markdown");
    }

    /// Test: plik .txt — fallback to markdown parser
    #[test]
    fn test_load_progress_auto_txt_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tasks.txt");
        create_test_progress_file(&path).unwrap();

        let result = load_progress_auto(&path);
        assert!(
            result.is_ok(),
            "Should load .txt file (fallback to markdown)"
        );
        let summary = result.unwrap();
        assert_eq!(summary.total(), 1, "Should parse as markdown");
    }

    /// Test: nieistniejący plik — zwraca błąd
    #[test]
    fn test_load_progress_auto_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.yml");

        let result = load_progress_auto(&path);
        assert!(result.is_err(), "Should fail for missing file");
        assert!(
            matches!(result.unwrap_err(), RalphError::MissingFile(_)),
            "Should return MissingFile error"
        );
    }

    /// Test: nieistniejący plik .md — zwraca błąd MissingFile (markdown path)
    #[test]
    fn test_load_progress_auto_missing_md_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.md");

        let result = load_progress_auto(&path);
        assert!(result.is_err(), "Should fail for missing .md file");
        assert!(
            matches!(result.unwrap_err(), RalphError::MissingFile(_)),
            "Should return MissingFile error for missing .md"
        );
    }

    /// Test: nieprawidłowy YAML w pliku .yml — zwraca błąd parsowania
    #[test]
    fn test_load_progress_auto_invalid_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("invalid.yml");
        std::fs::write(&path, "invalid: yaml: content: {{{").unwrap();

        let result = load_progress_auto(&path);
        assert!(result.is_err(), "Should fail for invalid YAML");
    }

    // ── check_progress_reload tests ──

    /// Test: brak pliku progress — zwraca None
    #[test]
    fn test_check_progress_reload_no_file() {
        let mut mtime = None;
        let mut last_check = std::time::Instant::now() - std::time::Duration::from_secs(20);

        let result = check_progress_reload(&None, &mut mtime, &mut last_check);
        assert!(result.is_none());
    }

    /// Test: plik nie zmieniony — zwraca None
    #[test]
    fn test_check_progress_reload_not_modified() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tasks.yml");
        create_test_tasks_file(&path).unwrap();

        let initial_mtime = std::fs::metadata(&path).unwrap().modified().ok();
        let mut mtime = initial_mtime;
        let mut last_check = std::time::Instant::now() - std::time::Duration::from_secs(20);

        let result = check_progress_reload(&Some(path), &mut mtime, &mut last_check);
        // mtime nie zmienione → brak reload
        assert!(result.is_none());
    }

    /// Test: check w ciągu 15s — nie sprawdza mtime
    #[test]
    fn test_check_progress_reload_too_soon() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tasks.yml");
        create_test_tasks_file(&path).unwrap();

        let mut mtime = None; // Pierwszy check — mtime niezainicjowane
        let mut last_check = std::time::Instant::now(); // Właśnie sprawdzony

        let result = check_progress_reload(&Some(path), &mut mtime, &mut last_check);
        // Za wcześnie na sprawdzenie (< 15s)
        assert!(result.is_none());
    }

    /// Test: pierwszy check po 15s — ładuje progress
    #[test]
    fn test_check_progress_reload_first_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tasks.yml");
        create_test_tasks_file(&path).unwrap();

        let mut mtime = None; // Brak poprzedniego mtime
        let mut last_check = std::time::Instant::now() - std::time::Duration::from_secs(20);

        let result = check_progress_reload(&Some(path), &mut mtime, &mut last_check);
        // Pierwszy load — mtime zmienione z None → Some
        assert!(result.is_some());
        assert!(mtime.is_some());
    }
}
