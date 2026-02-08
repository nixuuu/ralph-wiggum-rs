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

Create a `.ralph.toml` file in your project root to customize prompt handling:

```toml
[prompt]
# Text prepended to every prompt
prefix = "Follow the project coding standards."

# Text appended to every prompt
suffix = "Update CHANGELOG.md with your changes."
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
