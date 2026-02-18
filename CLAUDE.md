# Ralph-Wiggum-RS

Rust TUI application that runs Claude CLI in a loop until a completion promise is found. Supports task management with PRD generation, progress tracking, and parallel orchestration of multiple Claude workers.

## Tech Stack
- **Language**: Rust 2024 edition (1.85+)
- **Async**: tokio (full features + signal)
- **CLI**: clap 4.5 (derive macros)
- **TUI**: ratatui 0.30 + crossterm 0.29
- **Markdown**: termimad 0.34
- **Serialization**: serde + serde_json + serde_yaml + toml
- **Error handling**: thiserror 2.0
- **HTTP**: reqwest 0.13 (for self-update)

## Directory Structure
```
src/
├── main.rs                  # Entry point, CLI routing
├── cli.rs                   # Clap CLI definitions (Cli, Commands)
├── commands/
│   ├── mod.rs
│   ├── run/                 # Core loop: run Claude iteratively
│   │   ├── mod.rs           # execute() entry point
│   │   ├── app.rs           # RunApp: AppState for run mode TUI
│   │   ├── args.rs          # RunArgs
│   │   ├── config.rs        # Config struct
│   │   ├── event_formatting.rs # Event formatting utilities
│   │   ├── events.rs        # Keyboard input thread (OS thread!)
│   │   ├── formatting_helpers.rs # Output formatting helpers
│   │   ├── once.rs          # One-shot Claude invocation
│   │   ├── output.rs        # OutputFormatter (tokens, cost, tools)
│   │   ├── promise.rs       # Promise detection logic
│   │   ├── prompt.rs        # System prompt builder
│   │   ├── runner_reader.rs # Runner output reader
│   │   ├── runner_types.rs  # Runner type definitions
│   │   ├── runner.rs        # ClaudeRunner (process management)
│   │   ├── state.rs         # StateManager (iterations, resume)
│   │   ├── summary.rs       # Run session summary
│   │   └── ui.rs            # StatusTerminal (ratatui inline)
│   ├── task/                # Task management commands
│   │   ├── mod.rs           # Task command routing
│   │   ├── args.rs          # TaskCommands enum, PrdArgs, AddArgs, PlanArgs, etc.
│   │   ├── add.rs           # task add
│   │   ├── clean.rs         # task clean
│   │   ├── command_app.rs   # Task command AppState
│   │   ├── continue_cmd.rs  # task continue
│   │   ├── edit.rs          # task edit
│   │   ├── generate_deps_cmd.rs # task generate-deps
│   │   ├── input.rs         # Input resolution (file/prompt/stdin)
│   │   ├── migrate.rs       # task migrate
│   │   ├── plan.rs          # task plan
│   │   ├── prd.rs           # task prd
│   │   ├── state_helper.rs  # Task state helpers
│   │   ├── status.rs        # task status
│   │   ├── task_runner.rs   # Task runner utilities
│   │   ├── explorer/        # Task explorer TUI
│   │   │   ├── mod.rs       # Explorer entry point
│   │   │   ├── drawing.rs   # Explorer rendering
│   │   │   ├── keys.rs      # Explorer keybindings
│   │   │   └── state.rs     # Explorer state
│   │   └── orchestrate/     # Orchestration subsystem
│   │       ├── mod.rs       # Orchestrator module exports
│   │       ├── ai.rs        # AI-assisted deps/conflict resolution
│   │       ├── app.rs       # Orchestrate AppState
│   │       ├── app_keys.rs  # Orchestrate keybindings
│   │       ├── app_render.rs # Orchestrate rendering
│   │       ├── assignment.rs # Task assignment logic
│   │       ├── cleanup.rs   # Cleanup utilities
│   │       ├── completion_summary.rs # Completion summary
│   │       ├── config.rs    # Orchestration config
│   │       ├── dry_run.rs   # DAG visualization
│   │       ├── events.rs    # Worker event protocol
│   │       ├── git_helpers.rs # Git utilities
│   │       ├── merge.rs     # Squash merge engine
│   │       ├── orchestrator.rs # Core orchestrator logic
│   │       ├── orchestrator_events.rs # Orchestrator event handling
│   │       ├── orchestrator_merge.rs  # Orchestrator merge logic
│   │       ├── orchestrator_tui.rs    # Orchestrator TUI integration
│   │       ├── output.rs    # Multiplexed worker output
│   │       ├── profile_matcher.rs # Verification profile matching
│   │       ├── run_loop.rs  # Main orchestration loop
│   │       ├── scheduler.rs # Task queue with DAG awareness
│   │       ├── shared_types.rs # Shared type definitions
│   │       ├── shutdown_types.rs # Shutdown signal types
│   │       ├── state.rs     # Session state & lockfile
│   │       ├── summary.rs   # End-of-session report
│   │       ├── verify.rs    # Task verification
│   │       ├── worker.rs    # Worker 3-phase executor
│   │       ├── worker_panel.rs  # Worker TUI panel
│   │       ├── worker_runner.rs # Adapted ClaudeRunner
│   │       ├── worker_status.rs # Worker status tracking
│   │       └── worktree.rs  # Git worktree manager
│   ├── mcp/                 # HTTP MCP server
│   │   ├── mod.rs           # MCP module entry point
│   │   ├── ask_user.rs      # AskUserQuestion handler
│   │   ├── handlers.rs      # HTTP request handlers
│   │   ├── middleware.rs     # Origin validation middleware
│   │   ├── protocol.rs      # MCP protocol types
│   │   ├── router.rs        # Axum router setup
│   │   ├── server.rs        # Server lifecycle
│   │   ├── session.rs       # Session management
│   │   ├── state.rs         # Shared server state
│   │   └── tools.rs         # Task MCP tools (list, get, update, add, edit, etc.)
│   └── update/              # Self-update command
├── tui/                     # Terminal UI framework
│   ├── mod.rs               # Public API exports
│   ├── app.rs               # App struct: terminal manager + event loop
│   ├── events.rs            # EventDispatcher: OS thread keyboard polling
│   ├── theme.rs             # Theme: centralna paleta kolorów
│   ├── formatter.rs         # RatuiFormatter: Span-based output helpers
│   ├── formatting.rs        # Format helpers (duration, tokens)
│   ├── tool_formatting.rs   # Tool call formatting utilities
│   ├── responsive.rs        # Responsive layout (Breakpoint, LayoutAreas)
│   ├── ring_buffer.rs       # OutputRingBuffer: bounded output storage
│   ├── test_helpers.rs      # TUI testing utilities
│   └── widgets/             # Reusable TUI components
│       ├── mod.rs
│       ├── header.rs        # Header: panel title bar
│       ├── status_bar.rs    # StatusBar: bottom progress/stats
│       ├── splash.rs        # SplashScreen: startup banner
│       ├── output_view.rs   # OutputView: scrollable text panel
│       ├── task_tree.rs     # TaskTreeWidget: hierarchical task list
│       ├── task_sidebar.rs  # TaskSidebar: collapsible task panel
│       ├── task_preview.rs  # Task preview rendering
│       ├── task_detail.rs   # TaskDetail: focused task view
│       ├── text_input_overlay.rs # TextInputOverlay: modal input
│       ├── ask_user.rs      # AskUserWidget: unified question UI
│       ├── ask_user_choice.rs   # ChoiceWidget: single-select
│       ├── ask_user_confirm.rs  # ConfirmWidget: yes/no dialog
│       ├── ask_user_multi.rs    # MultiSelectWidget: multi-select
│       └── ask_user_text.rs     # TextInputWidget: text input field
├── shared/
│   ├── mod.rs
│   ├── banner.rs            # ASCII art banner
│   ├── dag.rs               # DAG algorithms
│   ├── diagnostics.rs       # Diagnostic utilities
│   ├── error.rs             # RalphError enum
│   ├── file_config.rs       # .ralph.toml config
│   ├── icons.rs             # Nerd Font / ASCII icons
│   ├── markdown.rs          # Terminal markdown rendering
│   ├── mcp.rs               # MCP shared utilities
│   ├── progress.rs          # PROGRESS.md parser (legacy)
│   └── tasks/               # Task tree data structures
│       ├── mod.rs           # TaskTree, TaskNode exports
│       ├── helpers.rs       # Utility functions
│       ├── node.rs          # TaskNode struct
│       ├── tree_ops.rs      # Tree operations (add, delete, move)
│       └── validation.rs    # Tree validation (cycles, deps)
├── templates/               # Embedded prompt templates
│   ├── mod.rs               # include_str! constants
│   ├── prd_prompt_yaml.md   # PRD generation prompt
│   ├── add_prompt_yaml.md   # Task add prompt
│   ├── edit_prompt_yaml.md  # Task edit prompt
│   ├── plan_prompt.md       # Task plan prompt
│   ├── continue_system_prompt.md # Continue system prompt
│   └── deps_generation_prompt_yaml.md # Deps generation prompt
└── updater/                 # GitHub release updater
    ├── mod.rs               # Updater module exports
    ├── executable_manager.rs # Binary replacement logic
    ├── github_release.rs    # GitHub API release fetcher
    ├── platform_detector.rs # OS/arch detection
    ├── self_updater.rs      # Self-update orchestrator
    └── version_checker.rs   # Version comparison
```

## Key Patterns
- **Error handling**: `thiserror::Error` in `RalphError`, propagate with `?`, `type Result<T>`
- **Config**: `FileConfig` loaded from `.ralph.toml`, nested serde structs, `#[serde(default)]` for backward compat
- **Shared state**: `Arc<Mutex<T>>` — consolidate locks (max 2 per event), `Arc<AtomicBool>` for shutdown
- **Caching**: `LazyLock` for expensive singletons (MadSkin, paths)
- **Input thread**: Dedicated `std::thread::spawn` for crossterm — NEVER `tokio::spawn`
- **TUI framework**:
  - `App` struct manages terminal lifecycle (raw mode, AlternateScreen, event loop)
  - `EventDispatcher` on dedicated OS thread polls crossterm events
  - `AppState` trait: per-command state implements `draw()` + `handle_event()`
  - `Theme` singleton (`DEFAULT_THEME`) for centralized color palette
  - Widgets in `src/tui/widgets/`: Header, StatusBar, TaskSidebar, AskUserWidget, etc.
- **Templates**: `include_str!()` for embedded prompts
- **Tests**: `#[cfg(test)] mod tests` inline in each source file
- **Git convention**: Angular commit format — `type(scope): description`

## TUI Architecture

Ralph używa frameworka TUI zbudowanego na ratatui + crossterm, z centralnym event loop i reużywalnymi widgetami.

### Core Components

**App (src/tui/app.rs)**
- Manager terminala i event loop
- Enkapsuluje `Terminal<CrosstermBackend<Stdout>>`, `EventDispatcher`, raw mode lifecycle
- Metody: `App::new()` → `app.run(state)` → automatic cleanup in Drop
- Variants: `run(&mut impl AppState)` dla owned state, `run_shared(Arc<Mutex<S>>)` dla shared state
- `with_shutdown(Arc<AtomicBool>)` do współdzielenia flagi shutdown między TUI a async runtime

**AppState trait**
```rust
pub trait AppState {
    fn draw(&mut self, frame: &mut Frame, area: Rect);
    fn handle_event(&mut self, event: AppEvent) -> EventResult;
}
```
- Per-command state implementuje ten trait
- `draw()` — rysuje UI, `&mut self` dla StatefulWidget
- `handle_event()` — obsługuje zdarzenia (Key, Resize, Tick), zwraca `EventResult` (Consumed, Ignored, Quit, Shutdown)

**EventDispatcher (src/tui/events.rs)**
- Dedykowany OS thread (`std::thread::spawn`, nigdy `tokio::spawn`) polluje crossterm events
- Kanał `std::sync::mpsc` przekazuje eventy do event loop
- Emituje `AppEvent`: `Key(KeyEvent)`, `Resize(u16, u16)`, `Tick`
- Priorytet: Ctrl+C (shutdown) → per-command handler
- Flaga `paused` (`Arc<AtomicBool>`) — gdy true, dispatcher ignoruje key events (dla interaktywnych widgetów, które same czytają crossterm)

**Theme (src/tui/theme.rs)**
- Centralna paleta kolorów: `DEFAULT_THEME` singleton
- Kolory: `primary`, `success`, `warning`, `error`, `muted`, `border_focused`, `border_normal`, `header_bg`, `status_bar_bg`
- Metody: `border_style(focused)`, `header_style()`, `success_style()`, `state_color(TaskStatus)`, etc.

### Widgets (src/tui/widgets/)

Reużywalne komponenty UI implementujące ratatui `Widget` trait:

**Layout widgets:**
- `Header` — pasek tytułu panelu (z theme)
- `StatusBar` — dolny status bar (progress, stats, keybindings)
- `SplashScreen` — ekran powitalny z ASCII banner

**Task widgets:**
- `TaskTreeWidget` — hierarchiczne drzewo tasków (scroll, expand/collapse)
- `TaskSidebar` — zwijany panel z task tree + sidebar state
- `TaskDetail` — szczegółowy widok focused tasku
- `render_task_preview()` — render podglądu tasku (opis, deps, files)

**Input widgets:**
- `AskUserWidget` — zunifikowany interfejs do pytań (deleguje do choice/confirm/multi/text)
- `ChoiceWidget` — single-select picker
- `ConfirmWidget` — yes/no dialog
- `MultiSelectWidget` — multi-select picker (checkbox list)
- `TextInputWidget` — pole tekstowe z kursorem
- `TextInputOverlay` — modal overlay z text input

**Output widgets:**
- `OutputView` — scrollowalny panel z tekstem (ring buffer, auto-scroll)

### Responsive Layout (src/tui/responsive.rs)

- `Breakpoint` enum: `Small` (<80 cols), `Medium` (80-119), `Large` (≥120)
- `LayoutAreas` struct: grid layout z sidebar (4 obszary: header, sidebar, content, status_bar)
- Adaptive: sidebar zwija się automatycznie w trybie Small

### Output Management (src/tui/ring_buffer.rs)

- `OutputRingBuffer` — bounded circular buffer dla output lines
- Auto-scrolling: `auto_scroll: bool` (disabled gdy user scrolluje manualnie)
- Scroll state: `scroll_offset`, `clamp_scroll()`, `scroll_to_bottom()`
- Używany w `OutputView` i run mode UI

### Typical Flow

```rust
// 1. Utwórz App
let mut app = App::new(Duration::from_millis(100))?
    .with_shutdown(shutdown_flag);

// 2. Utwórz per-command state (implementuje AppState)
let mut state = MyCommandState::new();

// 3. Uruchom event loop
app.run(&mut state)?;

// 4. Drop app → automatic cleanup (raw mode, AlternateScreen)
```

**Shared state variant:**
```rust
let state = Arc::new(Mutex::new(MySharedState::new()));
let state_clone = state.clone();

// Runner callbacki mogą pushować do state (np. ring buffer)
app.run_shared(state)?;
```

### Testing TUI

- `src/tui/test_helpers.rs` — utilities do testowania widgetów
- Mock state implementujący `AppState`
- Snapshot testing dla widget rendering (insta crate)
- Event simulation bez prawdziwego terminala

## Commands
```bash
# Build
cargo build

# Test
cargo test

# Lint
cargo clippy --all-targets -- -D warnings

# Run
cargo run -- --prompt "your prompt"
cargo run -- task prd --file PRD.md
cargo run -- task plan --prompt "Plan task execution"
cargo run -- task add --prompt "Add new feature"
cargo run -- task edit --prompt "Update task 2.3"
cargo run -- task continue
cargo run -- task status
cargo run -- task generate-deps
cargo run -- task orchestrate --workers 3
cargo run -- task orchestrate --dry-run
cargo run -- task clean
cargo run -- task migrate
cargo run -- update
```
