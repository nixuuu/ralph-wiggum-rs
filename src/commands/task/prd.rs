use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::style::Stylize;

use super::args::PrdArgs;
use super::command_app::TaskCommandApp;
use super::input::resolve_input;
use crate::commands::run::output::OutputFormatter;
use crate::commands::run::runner::ClaudeRunner;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::tasks::TasksFile;
use crate::templates;
use crate::tui::app::App;

pub async fn execute(args: PrdArgs, file_config: &FileConfig) -> Result<()> {
    // Resolve input
    let input = resolve_input(
        args.file.as_ref(),
        args.prompt.as_deref(),
        Some("Opisz wymagania projektu (PRD)..."),
    )?;

    // Determine output dir
    let output_dir = args
        .output_dir
        .clone()
        .or_else(|| file_config.task.output_dir.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Ensure .ralph/ exists
    let ralph_dir = output_dir.join(".ralph");
    std::fs::create_dir_all(&ralph_dir)?;

    // Build prompt (YAML template)
    let prompt = templates::PRD_PROMPT_YAML
        .replace("{prd_content}", &input)
        .replace("{output_dir}", &output_dir.display().to_string());

    // Determine model
    let model = args
        .model
        .clone()
        .or_else(|| file_config.task.default_model.clone());

    // Initialize TaskCommandApp with shared state
    let app_state = Arc::new(Mutex::new(
        TaskCommandApp::new("task prd").with_scroll_step(file_config.tui.scroll_step),
    ));
    if let Some(ref m) = model {
        app_state.lock().unwrap().set_model(m);
    }
    app_state.lock().unwrap().set_status("Initializing...");

    // Setup shutdown flag
    let shutdown = Arc::new(AtomicBool::new(false));

    // Configure ClaudeRunner
    let runner = ClaudeRunner::oneshot(prompt, model, Some(output_dir.clone()));

    // Create OutputFormatter for event formatting
    let formatter = Arc::new(Mutex::new(OutputFormatter::new(file_config.ui.nerd_font)));
    formatter.lock().unwrap().start_iteration();

    // Spawn ClaudeRunner in background task
    let runner_handle = {
        let app_state = app_state.clone();
        let shutdown = shutdown.clone();
        let formatter = formatter.clone();

        tokio::spawn(async move {
            let result = runner
                .run(
                    shutdown.clone(),
                    |event| {
                        // Format event to Lines
                        let lines = {
                            let mut fmt = formatter.lock().unwrap();
                            fmt.format_event(event)
                        };

                        // Push to app ring buffer
                        if !lines.is_empty() {
                            let mut app = app_state.lock().unwrap();
                            app.push_event(lines);
                            app.set_status("Generating PRD...");
                        }
                    },
                    || {
                        // Idle callback — update status
                        let mut app = app_state.lock().unwrap();
                        app.set_status("Generating PRD...");
                    },
                )
                .await;

            // Mark completion and signal TUI to exit
            {
                let mut app = app_state.lock().unwrap();
                if result.is_ok() {
                    app.set_status("PRD generation complete — press q to exit");
                } else {
                    app.set_status("PRD generation failed — press q to exit");
                }
            }
            // Brief delay so user sees final status before TUI closes
            tokio::time::sleep(Duration::from_secs(2)).await;
            shutdown.store(true, Ordering::SeqCst);

            result
        })
    };

    // Run TUI event loop on main thread (exits on shutdown flag or 'q' key)
    let tui_result = {
        let mut tui_app = App::new(Duration::from_millis(100))?.with_shutdown(shutdown.clone());
        tui_app.run_shared(app_state.clone())
    };

    // Wait for runner to complete (may already be done if TUI exited via shutdown)
    let runner_result = runner_handle
        .await
        .map_err(|e| RalphError::ClaudeProcess(format!("ClaudeRunner task panicked: {}", e)))?;

    // Handle TUI errors
    tui_result?;

    // Propagate runner errors (Interrupted from Ctrl+C is expected on manual quit)
    match runner_result {
        Ok(_) => {}
        Err(RalphError::Interrupted) => {}
        Err(e) => return Err(e),
    }

    // Verify required outputs exist
    let tasks_path = output_dir.join(&file_config.task.tasks_file);

    if !tasks_path.exists() {
        return Err(RalphError::TaskSetup(format!(
            "{} was not created by Claude. Please check the PRD and try again.",
            file_config.task.tasks_file.display()
        )));
    }

    // Print summary via TasksFile adapter
    let tasks_file = TasksFile::load(&tasks_path)?;
    let summary = tasks_file.to_summary();
    println!("{}", "━".repeat(60).dark_grey());
    println!(
        "{} Project files generated successfully!",
        "✓".green().bold()
    );
    println!("{}", "━".repeat(60).dark_grey());
    println!(
        "  {} {} tasks ({} todo, {} in progress)",
        "Tasks:".dark_grey(),
        summary.total(),
        summary.todo,
        summary.in_progress
    );

    if let Some(current) = tasks_file.current_task() {
        println!(
            "  {} {} [{}] {}",
            "Next:".dark_grey(),
            current.id.as_str().cyan(),
            current.component.as_str().yellow(),
            current.name
        );
    }

    println!();
    println!(
        "  {} {}",
        "Run:".dark_grey(),
        "ralph-wiggum task continue".cyan()
    );
    println!("{}", "━".repeat(60).dark_grey());

    Ok(())
}
