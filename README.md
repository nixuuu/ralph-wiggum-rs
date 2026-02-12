# Ralph Wiggum

A CLI tool that runs Claude Code in an iterative loop until a completion promise is found.

## Overview

Ralph Wiggum wraps the `claude` CLI and executes your prompt in a loop. Each iteration verifies whether the previous one completed the task correctly. The loop continues until Claude signals completion with a promise tag (e.g., `<promise>DONE</promise>`), ensuring tasks are truly finished before stopping.

## Installation

### Prerequisites

- [Claude Code CLI](https://github.com/anthropics/claude-code) installed and configured

### Quick install (recommended)

**Linux / macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/nixuuu/ralph-wiggum-rs/master/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/nixuuu/ralph-wiggum-rs/master/install.ps1 | iex
```

The scripts download the latest prebuilt binary for your platform and install it to `~/.local/bin/`. If a prebuilt binary is not available, they fall back to building from source.

### Building from source

Requires Rust 1.85+ (edition 2024).

```bash
cargo build --release
```

The binary will be available at `target/release/ralph-wiggum`.

### Updating

```bash
ralph-wiggum --update
```

## Usage

```bash
ralph-wiggum --prompt "Your task description here"
```

### Command Line Options

| Option | Short | Default | Description |
|--------|-------|---------|-------------|
| `--prompt` | `-p` | (required) | The prompt/task to send to Claude |
| `--min-iterations` | `-m` | `1` | Minimum iterations before accepting promise |
| `--max-iterations` | `-n` | `0` (unlimited) | Maximum iterations (0 = unlimited) |
| `--promise` | | `done` | Completion promise text to look for |
| `--resume` | `-r` | | Resume from state file |
| `--state-file` | | `.claude/ralph-loop.local.md` | Path to state file |
| `--config` | `-c` | `.ralph.toml` | Path to config file |
| `--continue-session` | | | Continue conversation from previous iteration |
| `--update` | | | Update to the latest version |

### Examples

Basic usage:
```bash
ralph-wiggum --prompt "Write unit tests for the auth module"
```

With iteration limits:
```bash
ralph-wiggum --prompt "Refactor the database layer" --min-iterations 2 --max-iterations 5
```

Custom completion promise:
```bash
ralph-wiggum --prompt "Fix all linting errors" --promise "COMPLETE"
```

Resume an interrupted session:
```bash
ralph-wiggum --resume
```

## Configuration File

Create a `.ralph.toml` file in your project root. All sections and fields are optional — sensible defaults are used when omitted.

### `[prompt]` — Prompt customization

```toml
[prompt]
# Text prepended to every prompt
prefix = "Follow the project coding standards."

# Text appended to every prompt
suffix = "Update CHANGELOG.md with your changes."

# Custom system prompt with placeholders
system = "Iteration {iteration}/{max_iterations}. Promise: {promise}."
```

| Field | Default | Description |
|-------|---------|-------------|
| `prefix` | — | Text prepended before every user prompt |
| `suffix` | — | Text appended after every user prompt |
| `system` | — | Custom system prompt. Placeholders: `{iteration}`, `{promise}`, `{min_iterations}`, `{max_iterations}` |

### `[ui]` — Interface settings

```toml
[ui]
nerd_font = false  # Set to false for ASCII-only icons
```

| Field | Default | Description |
|-------|---------|-------------|
| `nerd_font` | `true` | Use Nerd Font icons in status bar. Set `false` for plain ASCII fallback |

### `[task]` — Task management

```toml
[task]
tasks_file = ".ralph/tasks.yml"
progress_file = "PROGRESS.md"
system_prompt_file = "SYSTEM_PROMPT.md"
output_dir = "."
default_model = "claude-sonnet-4-5-20250929"
adaptive_iterations = true
```

| Field | Default | Description |
|-------|---------|-------------|
| `tasks_file` | `.ralph/tasks.yml` | Path to hierarchical YAML task file |
| `progress_file` | `PROGRESS.md` | Path to legacy Markdown task file |
| `system_prompt_file` | `SYSTEM_PROMPT.md` | Path to system prompt used in `task continue` |
| `output_dir` | current directory | Output directory for generated files (`task prd`) |
| `default_model` | — | Default Claude model for task commands |
| `adaptive_iterations` | `true` | Auto-adjust iteration limits based on remaining tasks |

### `[task.orchestrate]` — Parallel orchestration

```toml
[task.orchestrate]
workers = 4
max_retries = 2
default_model = "claude-sonnet-4-5-20250929"
worktree_prefix = "my-project"
verify_commands = "cargo test && cargo clippy --all-targets -- -D warnings"
conflict_resolution_model = "opus"
```

| Field | Default | Description |
|-------|---------|-------------|
| `workers` | `2` | Number of parallel Claude workers |
| `max_retries` | `3` | Max retries per task before marking as blocked |
| `default_model` | — | Override Claude model for workers (takes precedence over `[task].default_model`) |
| `worktree_prefix` | — | Prefix for Git worktree directory names |
| `verify_commands` | — | Shell commands run in verify phase after implementation |
| `conflict_resolution_model` | `"opus"` | Claude model used for AI-assisted merge conflict resolution |

#### Setup commands

Commands to run after creating each worktree, before Claude starts working. Useful for installing dependencies, copying config files, or preparing the environment.

```toml
# Simple form — list of shell commands
[task.orchestrate]
setup_commands = ["npm install", "cp .env.example .env"]

# Detailed form — with display labels
[[task.orchestrate.setup_commands]]
run = "cp {ROOT_DIR}/.env {WORKTREE_DIR}/.env"
name = "Copy env file"

[[task.orchestrate.setup_commands]]
run = "npm install --prefix {WORKTREE_DIR}"
name = "Install dependencies"
```

Available template variables in setup commands:

| Variable | Description |
|----------|-------------|
| `{ROOT_DIR}` | Path to the main repository root |
| `{WORKTREE_DIR}` | Path to the worker's worktree directory |
| `{TASK_ID}` | ID of the task assigned to the worker |
| `{WORKER_ID}` | Numeric worker ID (0-based) |

### Full example

```toml
[prompt]
prefix = "Follow coding standards from CLAUDE.md."
suffix = "Run tests after changes."

[ui]
nerd_font = true

[task]
tasks_file = ".ralph/tasks.yml"
default_model = "claude-sonnet-4-5-20250929"

[task.orchestrate]
workers = 3
max_retries = 2
verify_commands = "cargo test && cargo clippy -- -D warnings"

[[task.orchestrate.setup_commands]]
run = "cp {ROOT_DIR}/.env {WORKTREE_DIR}/.env"
name = "Copy env"
```

## Task-Based Development

Beyond single-prompt loops, Ralph Wiggum supports a full task-based workflow: define a PRD, generate a structured task list, and let Claude iterate through each task until the project is complete.

**Workflow:** PRD → `.ralph/tasks.yml` → iterative execution or parallel orchestration → finished project.

### Task Format

Tasks are stored in `.ralph/tasks.yml` — a hierarchical YAML format with inline dependencies, per-task model overrides, and rich metadata.

```yaml
default_model: sonnet
tasks:
- id: '1'
  name: Authentication system
  component: auth
  subtasks:
  - id: '1.1'
    name: Set up JWT middleware
    status: done
    model: sonnet
    description: Implement JWT validation middleware
    related_files:
    - src/middleware/auth.rs
  - id: '1.2'
    name: Add role-based access control
    status: todo
    deps: ['1.1']
    model: opus
```

| Field | Required | Description |
|-------|----------|-------------|
| `id` | yes | Dot-delimited hierarchical ID (e.g., `1.2.3`) |
| `name` | yes | Task description |
| `status` | leaf only | `todo`, `done`, `in_progress`, `blocked` |
| `component` | no | Component/module tag |
| `deps` | no | List of task IDs this depends on |
| `model` | no | Override default model for this task |
| `description` | no | Detailed description |
| `related_files` | no | Files relevant to this task |
| `subtasks` | no | Nested child tasks |

### Commands

#### `task prd` — Generate project files from a PRD

Parses your requirements document and generates `.ralph/tasks.yml` and supporting files.

```bash
# From a file
ralph-wiggum task prd --file requirements.md

# From inline text
ralph-wiggum task prd --prompt "Build a REST API with auth and rate limiting"

# From stdin
cat PRD.md | ralph-wiggum task prd

# With custom output directory and model
ralph-wiggum task prd --file requirements.md --output-dir ./project --model claude-sonnet-4-5-20250929
```

| Option | Short | Description |
|--------|-------|-------------|
| `--file` | `-f` | Path to PRD file |
| `--prompt` | `-p` | PRD content as text |
| `--output-dir` | `-o` | Output directory (default: current directory) |
| `--model` | `-m` | Claude model to use |

**Generated files:**
- `.ralph/tasks.yml` — hierarchical task list with statuses and dependencies
- `SYSTEM_PROMPT.md` — system prompt for the development loop
- `CLAUDE.md` — project conventions (only if not already present)

Input priority: `--file` > `--prompt` > stdin.

#### `task continue` — Run the development loop

Reads `.ralph/tasks.yml` (or `PROGRESS.md`) and `SYSTEM_PROMPT.md`, then enters the iterative loop — picking up the next task, executing it, and moving on until all tasks are complete.

```bash
ralph-wiggum task continue
```

No arguments needed. Iteration limits are computed automatically from remaining tasks. The status bar displays iteration metrics, current task info, and a progress gauge.

#### `task orchestrate` — Parallel worker orchestration

Runs multiple Claude workers in parallel, each in its own Git worktree. Tasks are scheduled based on their dependency graph (DAG), merged back automatically, and displayed in a fullscreen dashboard.

```bash
# Run with 3 parallel workers
ralph-wiggum task orchestrate --workers 3

# Preview the execution plan without running
ralph-wiggum task orchestrate --dry-run

# Resume a previous session
ralph-wiggum task orchestrate --resume

# Run specific tasks only
ralph-wiggum task orchestrate --tasks "1.1,1.2,2.1"

# Set cost and time limits
ralph-wiggum task orchestrate --max-cost 5.0 --timeout 2h
```

| Option | Description |
|--------|-------------|
| `--workers N` | Number of parallel workers |
| `--model NAME` | Default Claude model for workers |
| `--max-retries N` | Max retries per task before marking blocked |
| `--dry-run` | Show DAG plan without running workers |
| `--resume` | Resume a previous orchestration session |
| `--no-merge` | Skip merging — keep worktrees intact |
| `--max-cost AMOUNT` | Maximum session cost in USD |
| `--timeout DURATION` | Max session duration (e.g., `2h`, `30m`) |
| `--tasks IDS` | Filter specific tasks by comma-separated IDs |
| `--verbose` | Enable verbose JSONL event logging |
| `--worktree-prefix PREFIX` | Custom prefix for worktree directories |
| `--conflict-model NAME` | Claude model for merge conflict resolution (default: `opus`) |

**Dashboard keyboard shortcuts:**

| Key | Action |
|-----|--------|
| `Tab` / `Shift+Tab` | Cycle focus between worker panels |
| `1`-`9` | Jump to worker N |
| `Esc` | Unfocus current panel |
| `Up` / `Down` | Scroll focused worker output |
| `p` | Toggle task preview overlay |
| `r` | Reload tasks.yml |
| `q` | Quit (with confirmation — press twice to force) |

After completion, a summary table is printed with per-task status, cost, duration, and retry count.

#### `task add` — Add new tasks

Appends new tasks to `.ralph/tasks.yml`, preserving existing numbering and states.

```bash
ralph-wiggum task add --file new-requirements.md
ralph-wiggum task add --prompt "Add WebSocket support and real-time notifications"
echo "Add logging middleware" | ralph-wiggum task add
```

| Option | Short | Description |
|--------|-------|-------------|
| `--file` | `-f` | Path to requirements file |
| `--prompt` | `-p` | Requirements as text |
| `--model` | `-m` | Claude model to use |

#### `task edit` — Edit existing tasks

Modifies existing tasks — change descriptions, statuses, reorder, remove, split, or merge.

```bash
ralph-wiggum task edit --file edit-instructions.md
ralph-wiggum task edit --prompt "Remove task 2.3 and split task 3.1 into frontend and backend parts"
```

| Option | Short | Description |
|--------|-------|-------------|
| `--file` | `-f` | Path to file with edit instructions |
| `--prompt` | `-p` | Edit instructions as text |
| `--model` | `-m` | Claude model to use |

#### `task generate-deps` — AI-assisted dependency generation

Invokes Claude to analyze tasks and automatically generate dependencies. Validates the resulting DAG for cycles.

```bash
ralph-wiggum task generate-deps
ralph-wiggum task generate-deps --model claude-sonnet-4-5-20250929
```

#### `task status` — Show progress dashboard

Displays a quick overview of project progress.

```bash
ralph-wiggum task status
```

#### `task migrate` — Migrate from PROGRESS.md to tasks.yml

Converts a legacy `PROGRESS.md` file to the new `.ralph/tasks.yml` format, preserving all dependencies, model overrides, and statuses.

```bash
ralph-wiggum task migrate
```

#### `task clean` — Clean up orchestration resources

Removes leftover worktrees, branches, state files, and logs from previous orchestration sessions.

```bash
ralph-wiggum task clean
```

### Quick Start

```bash
# 1. Generate project files from your PRD
ralph-wiggum task prd --file requirements.md

# 2. (Optional) Auto-generate task dependencies
ralph-wiggum task generate-deps

# 3a. Run tasks sequentially
ralph-wiggum task continue

# 3b. Or run tasks in parallel with orchestration
ralph-wiggum task orchestrate --workers 3

# 4. Check progress at any time
ralph-wiggum task status

# 5. Add new requirements mid-project
ralph-wiggum task add --prompt "Add logging and monitoring"

# 6. Clean up after orchestration
ralph-wiggum task clean
```

## How It Works

1. **Prompt Injection**: Your prompt is wrapped with system instructions that tell Claude it's in an iterative loop and should only emit the promise tag when the task is truly complete.

2. **Loop Execution**: Ralph Wiggum runs `claude` with your prompt. After each iteration:
   - It checks for the completion promise in Claude's response
   - If found and minimum iterations reached, the loop ends successfully
   - Otherwise, it continues to the next iteration (starts fresh by default, use `--continue-session` to maintain conversation)

3. **State Management**: Progress is saved to a state file, allowing you to resume interrupted sessions.

4. **Graceful Shutdown**: Press `q` or `Ctrl+C` to interrupt. State is saved automatically.

## Status Bar

During execution, a status bar shows:
- Current iteration number
- Elapsed time
- Token usage (input/output)
- Cost estimate

### Live Progress Refresh

In `task continue` mode, the status bar displays task progress from `PROGRESS.md`. This data is refreshed:
- **Automatically** every 15 seconds (only when the file has actually changed)
- **Manually** by pressing `r` at any time

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success - completion promise found |
| `1` | Error or max iterations reached |
| `130` | Interrupted by user (Ctrl+C) |

## Why "Ralph Wiggum"?

Like the character who famously says "I'm helping!", this tool keeps iterating and checking until Claude is confident the task is actually done - not just claiming to be done.

## License

MIT
