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
│   │   ├── args.rs          # TaskCommands enum, PrdArgs, AddArgs, PlanArgs, etc.
│   │   ├── add.rs           # task add
│   │   ├── clean.rs         # task clean
│   │   ├── continue_cmd.rs  # task continue
│   │   ├── edit.rs          # task edit
│   │   ├── generate_deps_cmd.rs # task generate-deps
│   │   ├── input.rs         # Input resolution (file/prompt/stdin)
│   │   ├── migrate.rs       # task migrate
│   │   ├── plan.rs          # task plan
│   │   ├── prd.rs           # task prd
│   │   ├── status.rs        # task status
│   │   └── orchestrate/     # Orchestration subsystem
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
│   ├── mcp/                 # HTTP MCP server
│   │   ├── mod.rs           # MCP module entry point
│   │   ├── ask_user.rs      # AskUserQuestion handler
│   │   ├── handlers.rs      # HTTP request handlers
│   │   ├── middleware.rs    # Origin validation middleware
│   │   ├── protocol.rs      # MCP protocol types
│   │   ├── router.rs        # Axum router setup
│   │   ├── server.rs        # Server lifecycle
│   │   ├── session.rs       # Session management
│   │   ├── state.rs         # Shared server state
│   │   ├── tools.rs         # Task MCP tools (list, get, update, add, edit, etc.)
│   │   └── tui/             # TUI components for user interaction
│   │       ├── mod.rs
│   │       ├── choice_select.rs  # Single-choice picker
│   │       ├── confirm_select.rs # Confirmation dialog
│   │       ├── multi_select.rs   # Multi-choice picker
│   │       └── text_input.rs     # Text input field
│   └── update/              # Self-update command
├── shared/
│   ├── mod.rs
│   ├── banner.rs            # ASCII art banner
│   ├── dag.rs               # DAG algorithms
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
│   ├── prd_prompt.md
│   ├── add_prompt.md
│   ├── edit_prompt.md
│   ├── plan_prompt.md
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
