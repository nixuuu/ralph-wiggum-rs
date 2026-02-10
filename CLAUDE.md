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
│   │   ├── args.rs          # RunArgs
│   │   ├── config.rs        # Config struct
│   │   ├── events.rs        # Keyboard input thread (OS thread!)
│   │   ├── once.rs          # One-shot Claude invocation
│   │   ├── output.rs        # OutputFormatter (tokens, cost, tools)
│   │   ├── prompt.rs        # System prompt builder
│   │   ├── runner.rs        # ClaudeRunner (process management)
│   │   ├── state.rs         # StateManager (iterations, resume)
│   │   └── ui.rs            # StatusTerminal (ratatui inline)
│   ├── task/                # Task management commands
│   │   ├── mod.rs           # Task command routing
│   │   ├── args.rs          # TaskCommands enum, PrdArgs, AddArgs, etc.
│   │   ├── add.rs           # task add
│   │   ├── clean.rs         # task clean (new)
│   │   ├── continue_cmd.rs  # task continue
│   │   ├── edit.rs          # task edit
│   │   ├── input.rs         # Input resolution (file/prompt/stdin)
│   │   ├── prd.rs           # task prd
│   │   ├── status.rs        # task status
│   │   └── orchestrate/     # Orchestration subsystem (new)
│   │       ├── mod.rs       # Orchestrator core loop
│   │       ├── ai.rs        # AI-assisted deps/conflict resolution
│   │       ├── dry_run.rs   # DAG visualization
│   │       ├── events.rs    # Worker event protocol
│   │       ├── merge.rs     # Squash merge engine
│   │       ├── output.rs    # Multiplexed worker output
│   │       ├── scheduler.rs # Task queue with DAG awareness
│   │       ├── state.rs     # Session state & lockfile
│   │       ├── status.rs    # Dynamic N+2 status bar
│   │       ├── summary.rs   # End-of-session report
│   │       ├── worker.rs    # Worker 3-phase executor
│   │       ├── worker_runner.rs # Adapted ClaudeRunner
│   │       └── worktree.rs  # Git worktree manager
│   └── update/              # Self-update command
├── shared/
│   ├── mod.rs
│   ├── banner.rs            # ASCII art banner
│   ├── dag.rs               # DAG algorithms (new)
│   ├── error.rs             # RalphError enum
│   ├── file_config.rs       # .ralph.toml config
│   ├── icons.rs             # Nerd Font / ASCII icons
│   ├── markdown.rs          # Terminal markdown rendering
│   └── progress.rs          # PROGRESS.md parser
├── templates/               # Embedded prompt templates
│   ├── mod.rs               # include_str! constants
│   ├── prd_prompt.md
│   ├── add_prompt.md
│   ├── edit_prompt.md
│   ├── changenotes.md
│   ├── implementation_issues.md
│   ├── open_questions.md
│   └── (orchestration templates)
└── updater/                 # GitHub release updater
```

## Key Patterns
- **Error handling**: `thiserror::Error` in `RalphError`, propagate with `?`, `type Result<T>`
- **Config**: `FileConfig` loaded from `.ralph.toml`, nested serde structs, `#[serde(default)]` for backward compat
- **Shared state**: `Arc<Mutex<T>>` — consolidate locks (max 2 per event), `Arc<AtomicBool>` for shutdown
- **Caching**: `LazyLock` for expensive singletons (MadSkin, paths)
- **Input thread**: Dedicated `std::thread::spawn` for crossterm — NEVER `tokio::spawn`
- **TUI**: `Viewport::Inline(N)`, `insert_before` for content above status bar, iterate `area.y..area.y+height`
- **Templates**: `include_str!()` for embedded prompts
- **Tests**: `#[cfg(test)] mod tests` inline in each source file
- **Git convention**: Angular commit format — `type(scope): description`

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
cargo run -- task continue
cargo run -- task status
cargo run -- task orchestrate --workers 3
cargo run -- task orchestrate --dry-run
cargo run -- task clean
```
