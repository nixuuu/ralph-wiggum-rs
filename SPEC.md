# SPEC: Task Management Subcommands (`ralph-wiggum task`)

## Requirements

### Overview
Nowy sektor subkomend `task` integrujący ralph-wiggum z workflow opisanym w skillu `ralph-wiggum-setup`. Cztery zagnieżdżone subkomendy: `prd`, `continue`, `add`, `status`.

### `task prd` — Generowanie projektu z PRD
- **Input**: `--file <path>` lub `--prompt <text>` lub stdin (pipe). Brak wszystkich trzech = błąd.
- **Output dir**: domyślnie CWD, overridowalny przez `--output-dir <path>`
- **Model**: flaga `--model <name>` (domyślnie: system default Claude CLI)
- **Proces**:
  1. Ralph tworzy boilerplate pliki z embedded templateów (`include_str!`) — TYLKO jeśli nie istnieją: CHANGENOTES.md, IMPLEMENTATION_ISSUES.md, OPEN_QUESTIONS.md
  2. Ralph wywołuje Claude CLI jednorazowo (`claude -p --dangerously-skip-permissions`) z pełnym FS read
  3. Claude generuje: PROGRESS.md, SYSTEM_PROMPT.md, CURRENT_TASK.md, CLAUDE.md (CLAUDE.md TYLKO jeśli nie istnieje)
  4. Claude automatycznie wykrywa stack projektu (Cargo.toml, package.json, mix.exs, pyproject.toml) i wypełnia komendy weryfikacyjne
  5. Output Claude'a streamowany na terminal w real-time
  6. Po zakończeniu: auto-save + wyświetlenie podsumowania (ile tasków, jakie pliki, wykryty stack)
  7. Sprawdzenie czy PROGRESS.md i SYSTEM_PROMPT.md zostały utworzone (błąd jeśli nie)
  8. Brak timeout — Claude sam kończy
  9. NIE uruchamiamy automatycznie `task continue` — osobny krok
- **Prompt do Claude**: embedded jako `include_str!("templates/prd_prompt.md")`

### `task continue` — Kontynuacja pracy
- **Zero flag override** — pełny automat:
  1. Sprawdź czy PROGRESS.md istnieje → błąd z sugestią `task prd` jeśli brak
  2. Sprawdź czy SYSTEM_PROMPT.md istnieje → błąd z sugestią `task prd` jeśli brak
  3. Czytaj SYSTEM_PROMPT.md jako `--prompt`
  4. Parsuj PROGRESS.md → policz `[ ]` tasks → ustaw `min_iterations = remaining_tasks`
  5. Ustaw `max_iterations = min_iterations + 5`
  6. Włącz `--continue-session` i `--resume` (jeśli state file istnieje)
  7. Uruchom `run::execute()` z wyliczonymi parametrami
- **Adaptacja min_iterations w locie**: po każdej iteracji ralph czyta PROGRESS.md, re-count `[ ]` tasks, aktualizuje `state_manager.min_iterations` = `current_iteration + remaining`. Nie rusza CURRENT_TASK.md — Claude sam aktualizuje.
- **Dodatkowe info w status bar**: 3 linie (Inline(3)):
  - Linia 1: istniejący status (iteration, tokens, cost, time)
  - Linia 2: current task info z PROGRESS.md (pierwszy `[~]` lub `[ ]`) + avg time/task
  - Linia 3: ratatui Gauge progress bar (done/total tasks)
- **Odczyt PROGRESS.md**: po każdej iteracji + on-demand (key shortcut do odświeżenia)

### `task add` — Dodawanie nowych zadań
- **Input**: `--file <path>` lub `--prompt <text>` lub stdin (pipe)
- **Model**: flaga `--model <name>`
- **Proces**:
  1. Ralph wywołuje Claude CLI z pełnym FS read+write (`--dangerously-skip-permissions`)
  2. Claude czyta istniejący PROGRESS.md, nowe wymagania, generuje nowe taski z poprawnymi ID (kontynuacja numeracji)
  3. Claude bezpośrednio modyfikuje PROGRESS.md i CURRENT_TASK.md
  4. Output streamowany na terminal
  5. Po zakończeniu: ralph parsuje PROGRESS.md, przelicza min_iterations, aktualizuje state file (`.claude/ralph-loop.local.md`)
  6. Wyświetla podsumowanie (nowe taski, nowy count)
- **Prompt do Claude**: embedded jako `include_str!("templates/add_prompt.md")`

### `task status` — Dashboard stanu
- **TUI**: Viewport::Inline multiline (kilka linii, nie fullscreen)
- **Zawartość**:
  - Progress bar (ratatui Gauge): done/total
  - Breakdown: Done / In-progress / Blocked / Todo counts
  - Current task: ID, component, name (z PROGRESS.md — pierwszy `[~]` lub `[ ]`)
  - Avg time per task (z CHANGENOTES.md timestamps jeśli dostępne)
- **Wyjście**: q/Esc/Ctrl+C

### PROGRESS.md Parser
- **Pełny parser** ze strukturą: `Task { id: String, component: String, name: String, status: TaskStatus, epic: String }`
- **Dowolna głębokość ID**: `1.1`, `1.1.1`, `1.1.1.1` + `H.1`, `H.2` dla housekeeping
- **Tolerancyjny**: ignoruj linie niepasujące do wzorca, parsuj co się da. Żadnych błędów na nieoczekiwanym formacie.
- **Format linii**: `- [status] ID [component] name` gdzie status ∈ {` `, `x`, `~`, `!`}

### Konfiguracja `.ralph.toml` — sekcja `[task]`
```toml
[task]
progress_file = "PROGRESS.md"           # default
system_prompt_file = "SYSTEM_PROMPT.md" # default
current_task_file = "CURRENT_TASK.md"   # default
output_dir = "."                        # default
default_model = ""                      # empty = system default
auto_continue = false                   # reserved for future
adaptive_iterations = true              # default

[task.files]
changenotes = "CHANGENOTES.md"
issues = "IMPLEMENTATION_ISSUES.md"
questions = "OPEN_QUESTIONS.md"
```

### Core Loop Modification
- Nowe pole `Config.progress_file: Option<PathBuf>`
- Po każdej iteracji: jeśli `progress_file` ustawiony → czytaj + parsuj → aktualizuj `state_manager.min_iterations`
- Key shortcut (np. `r`) do ręcznego odświeżenia stanu PROGRESS.md

### Struktura CLI (clap)
```
ralph-wiggum
├── run [args]          # istniejąca komenda
├── update              # istniejąca komenda
├── task
│   ├── prd [--file|--prompt|stdin] [--output-dir] [--model]
│   ├── continue        # zero flags
│   ├── add [--file|--prompt|stdin] [--model]
│   └── status
└── (no subcommand)     # backward compat → run
```

### Pliki tworzone przez `task prd`
| Plik | Kto tworzy | Nadpisywanie |
|------|-----------|--------------|
| PROGRESS.md | Claude | Zawsze (nowy projekt) |
| SYSTEM_PROMPT.md | Claude | Zawsze (nowy projekt) |
| CURRENT_TASK.md | Claude | Zawsze (nowy projekt) |
| CLAUDE.md | Claude | TYLKO jeśli nie istnieje |
| CHANGENOTES.md | Ralph (template) | TYLKO jeśli nie istnieje |
| IMPLEMENTATION_ISSUES.md | Ralph (template) | TYLKO jeśli nie istnieje |
| OPEN_QUESTIONS.md | Ralph (template) | TYLKO jeśli nie istnieje |

---

## Project Conventions

### Wzorce kodu (z analizy codebase)
- **Modularyzacja**: `src/commands/<name>/mod.rs` + args.rs, config.rs, itd.
- **Error handling**: `thiserror::Error` enum `RalphError` w `shared/error.rs`, `type Result<T> = std::result::Result<T, RalphError>`
- **Config**: `FileConfig` z `serde::Deserialize` na `.ralph.toml`, `Config::build(args)` w module komendy
- **Clap**: derive macros z `#[derive(Parser)]` / `#[derive(Args)]` / `#[derive(Subcommand)]`
- **Shared utilities**: `src/shared/` (error, icons, markdown, banner, file_config)
- **Async**: tokio runtime, `async fn execute()` w modułach komend
- **State**: YAML frontmatter w markdown files, `serde_yaml` parsing
- **TUI**: ratatui `Viewport::Inline(N)` + crossterm
- **Templates**: `include_str!()` w `src/` (nowy katalog `src/templates/`)
- **Tests**: `#[cfg(test)] mod tests` inline w plikach źródłowych
- **Edition**: Rust 2024 (1.85+)

### Git
- Angular commit convention
- Branch: feature branches z master

---

## Complexity Assessment
- **Scope**: Large
- **Areas affected**:
  - `src/cli.rs` — nowe subkomendy Task + TaskCommands
  - `src/commands/mod.rs` — dodanie `pub mod task`
  - `src/commands/task/` — nowy katalog z 4 subkomendami
  - `src/commands/run/mod.rs` — hook post-iteration dla adaptive min_iterations
  - `src/commands/run/config.rs` — nowe pole `progress_file`
  - `src/commands/run/state.rs` — metoda `set_min_iterations()`
  - `src/commands/run/ui.rs` — Viewport::Inline(3) warunkowy, Gauge widget
  - `src/commands/run/output.rs` — task progress info w StatusData
  - `src/shared/file_config.rs` — sekcja `[task]` w FileConfig
  - `src/shared/error.rs` — nowe warianty błędów
  - `src/shared/` — nowy moduł `progress.rs` (parser PROGRESS.md)
  - `src/templates/` — embedded prompts + boilerplate templates
  - `src/main.rs` — routing nowej komendy Task

---

## Implementation Plan

### Phase 1: PROGRESS.md Parser
**Goal:** Pełny parser PROGRESS.md ze strukturami danych.

**Locations:** `src/shared/`

**Tasks:**
- [ ] Dodać `progress.rs` do `src/shared/mod.rs`
- [ ] Zdefiniować `TaskStatus` enum: `Todo`, `Done`, `InProgress`, `Blocked`
- [ ] Zdefiniować `ProgressTask` struct: `id: String`, `component: String`, `name: String`, `status: TaskStatus`
- [ ] Zdefiniować `ProgressSummary` struct: `tasks: Vec<ProgressTask>`, `total: usize`, `done: usize`, `in_progress: usize`, `blocked: usize`, `todo: usize`
- [ ] Implementować `parse_progress(content: &str) -> ProgressSummary` — tolerancyjny parser
- [ ] Implementować `current_task(summary: &ProgressSummary) -> Option<&ProgressTask>` — pierwszy `[~]` lub `[ ]`
- [ ] Implementować `load_progress(path: &Path) -> Result<ProgressSummary>` — czytaj plik + parsuj
- [ ] Unit testy: poprawny format, brakujące pola, puste linie, komentarze, housekeeping tasks, dowolna głębokość ID

**Patterns:**
```rust
// src/shared/progress.rs
#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus { Todo, Done, InProgress, Blocked }

#[derive(Debug, Clone)]
pub struct ProgressTask {
    pub id: String,        // "1.2.3" or "H.1"
    pub component: String, // "api", "infra", "all"
    pub name: String,      // "Create user endpoint"
    pub status: TaskStatus,
}

#[derive(Debug, Clone)]
pub struct ProgressSummary {
    pub tasks: Vec<ProgressTask>,
    pub done: usize,
    pub in_progress: usize,
    pub blocked: usize,
    pub todo: usize,
}

/// Tolerancyjny parser — linie niepasujące są ignorowane
/// Format: `- [x] 1.2.3 [api] Task name here`
pub fn parse_progress(content: &str) -> ProgressSummary { ... }
```

**Notes:**
- Regex pattern: `^- \[([ x~!])\] ([\w.]+) \[(\w+)\] (.+)$`
- Housekeeping ID: `H.1`, `H.2` — traktowane jak normalne taski
- Tolerancyjny = linie nie matchujące pattern → skip (nagłówki, separatory, komentarze)
- Nie używamy crate `regex` (removed z projektu) — string matching / manual parsing

---

### Phase 2: FileConfig — sekcja `[task]`
**Goal:** Rozszerzyć `.ralph.toml` o konfigurację task management.

**Locations:** `src/shared/file_config.rs`

**Tasks:**
- [ ] Dodać `TaskConfig` struct z polami: `progress_file`, `system_prompt_file`, `current_task_file`, `output_dir`, `default_model`, `auto_continue`, `adaptive_iterations`
- [ ] Dodać `TaskFilesConfig` struct: `changenotes`, `issues`, `questions`
- [ ] Dodać `task: TaskConfig` do `FileConfig` z `#[serde(default)]`
- [ ] Unit testy: parsowanie pełnej konfiguracji, domyślne wartości, częściowa konfiguracja

**Patterns:**
```rust
#[derive(Debug, Deserialize)]
pub struct TaskConfig {
    #[serde(default = "default_progress_file")]
    pub progress_file: PathBuf,        // "PROGRESS.md"
    #[serde(default = "default_system_prompt_file")]
    pub system_prompt_file: PathBuf,   // "SYSTEM_PROMPT.md"
    #[serde(default = "default_current_task_file")]
    pub current_task_file: PathBuf,    // "CURRENT_TASK.md"
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub auto_continue: bool,
    #[serde(default = "default_true")]
    pub adaptive_iterations: bool,
    #[serde(default)]
    pub files: TaskFilesConfig,
}
```

**Notes:** Backward compat — istniejące `.ralph.toml` bez sekcji `[task]` muszą nadal działać (all defaults).

---

### Phase 3: Embedded Templates
**Goal:** Templaty plików boilerplate i promptów jako `include_str!()`.

**Locations:** `src/templates/` (nowy katalog)

**Tasks:**
- [ ] Utworzyć `src/templates/` katalog
- [ ] Utworzyć `src/templates/prd_prompt.md` — prompt do Claude dla generowania PRD (instrukcje: wykryj stack, stwórz PROGRESS.md z atomowymi taskami, stwórz SYSTEM_PROMPT.md z komendami weryfikacyjnymi, CURRENT_TASK.md z pierwszym taskiem, CLAUDE.md z kontekstem projektu)
- [ ] Utworzyć `src/templates/add_prompt.md` — prompt do Claude dla dodawania tasków (instrukcje: przeczytaj PROGRESS.md, kontynuuj numerację, dodaj nowe taski, zaktualizuj CURRENT_TASK.md)
- [ ] Utworzyć `src/templates/changenotes.md` — boilerplate CHANGENOTES.md
- [ ] Utworzyć `src/templates/implementation_issues.md` — boilerplate IMPLEMENTATION_ISSUES.md
- [ ] Utworzyć `src/templates/open_questions.md` — boilerplate OPEN_QUESTIONS.md
- [ ] Utworzyć `src/templates/mod.rs` — moduł z `pub const` stałymi z `include_str!()`

**Patterns:**
```rust
// src/templates/mod.rs
pub const PRD_PROMPT: &str = include_str!("prd_prompt.md");
pub const ADD_PROMPT: &str = include_str!("add_prompt.md");
pub const CHANGENOTES_TEMPLATE: &str = include_str!("changenotes.md");
pub const ISSUES_TEMPLATE: &str = include_str!("implementation_issues.md");
pub const QUESTIONS_TEMPLATE: &str = include_str!("open_questions.md");
```

**Notes:** Prompty powinny zawierać precyzyjne instrukcje formatu PROGRESS.md (żeby Claude generował parsable output). Referuj format z SKILL.md.

---

### Phase 4: Shared Claude Runner
**Goal:** Wydzielić reusable funkcję do jednorazowego wywołania Claude CLI (używaną przez `task prd` i `task add`).

**Locations:** `src/shared/`

**Tasks:**
- [ ] Dodać `claude.rs` do `src/shared/mod.rs`
- [ ] Zaimplementować `run_claude_once(prompt: &str, model: Option<&str>, write_access: bool) -> Result<()>` — wywołuje `claude -p --dangerously-skip-permissions`, streamuje stdout na terminal, czeka na zakończenie
- [ ] Obsługa stdin piping (user prompt z stdin)
- [ ] Obsługa non-zero exit code → `RalphError`

**Patterns:**
```rust
// src/shared/claude.rs
pub struct ClaudeOnceOptions {
    pub prompt: String,
    pub model: Option<String>,
    pub write_access: bool,  // czy Claude ma full FS write
}

/// Wywołaj Claude CLI jednorazowo, streamuj output na terminal
pub async fn run_claude_once(options: ClaudeOnceOptions) -> Result<()> {
    let mut cmd = tokio::process::Command::new("claude");
    cmd.arg("-p").arg("--dangerously-skip-permissions");
    if let Some(model) = &options.model {
        cmd.arg("--model").arg(model);
    }
    cmd.arg(&options.prompt);
    cmd.stdout(Stdio::inherit()); // stream to terminal
    cmd.stderr(Stdio::inherit());
    // ...
}
```

**Notes:** Nie reusujemy `ClaudeRunner` z `commands/run/runner.rs` bo tamten jest zoptymalizowany pod JSON streaming w pętli. Tu potrzebujemy prostego one-shot z inherit stdout.

---

### Phase 5: Error Variants
**Goal:** Dodać nowe typy błędów dla task management.

**Locations:** `src/shared/error.rs`

**Tasks:**
- [ ] Dodać `TaskSetup(String)` — błędy z task prd/add (np. brak pliku input)
- [ ] Dodać `ProgressParse(String)` — błędy parsowania PROGRESS.md (używane tylko w strict mode, ale warto mieć)
- [ ] Dodać `MissingFile(String)` — brak wymaganego pliku (PROGRESS.md, SYSTEM_PROMPT.md)

**Notes:** Minimalne — tolerancyjny parser rzadko generuje błędy, ale `load_progress()` potrzebuje wariantu na IO error.

---

### Phase 6: CLI Structure — `task` Subcommand
**Goal:** Rozszerzenie clap CLI o zagnieżdżone subkomendy `task {prd, continue, add, status}`.

**Locations:** `src/cli.rs`, `src/commands/mod.rs`, `src/commands/task/mod.rs`, `src/commands/task/args.rs`

**Tasks:**
- [ ] Dodać `TaskCommands` enum z wariantami `Prd(PrdArgs)`, `Continue`, `Add(AddArgs)`, `Status`
- [ ] Dodać `PrdArgs` struct: `--file`, `--prompt`, `--output-dir`, `--model`
- [ ] Dodać `AddArgs` struct: `--file`, `--prompt`, `--model`
- [ ] Dodać `Task(TaskCommands)` do `Commands` enum w `cli.rs`
- [ ] Dodać `pub mod task` do `src/commands/mod.rs`
- [ ] Utworzyć `src/commands/task/mod.rs` z re-exportami i routing do subkomend
- [ ] Dodać routing w `src/main.rs`: `Commands::Task(cmd) => commands::task::execute(cmd).await`
- [ ] Unit testy: parsing CLI args dla task prd/continue/add/status

**Patterns:**
```rust
// src/cli.rs
#[derive(Subcommand, Debug)]
pub enum Commands {
    Run(RunArgs),
    Update,
    Task {
        #[command(subcommand)]
        command: TaskCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum TaskCommands {
    /// Generate project files from PRD
    Prd(PrdArgs),
    /// Continue working on tasks
    Continue,
    /// Add new tasks
    Add(AddArgs),
    /// Show task status dashboard
    Status,
}
```

**Notes:** `TaskCommands` i args structs w osobnym pliku `src/commands/task/args.rs`, re-export w `cli.rs`.

---

### Phase 7: `task prd` Implementation
**Goal:** Implementacja komendy generującej pliki projektu z PRD.

**Locations:** `src/commands/task/prd.rs`

**Tasks:**
- [ ] Implementować `execute(args: PrdArgs, file_config: &FileConfig) -> Result<()>`
- [ ] Obsługa input: --file (czytaj plik), --prompt (tekst), stdin (czytaj jeśli brak obu) → błąd jeśli brak wszystkich trzech
- [ ] Określić output_dir (args > file_config > CWD)
- [ ] Stworzyć boilerplate pliki z templateów (CHANGENOTES.md, IMPLEMENTATION_ISSUES.md, OPEN_QUESTIONS.md) — TYLKO jeśli nie istnieją
- [ ] Zbudować prompt dla Claude: embedded prd_prompt.md + user's PRD content + lista plików do wygenerowania + output_dir info + instrukcja aby nie nadpisywać CLAUDE.md jeśli istnieje
- [ ] Wywołać `run_claude_once()` z promptem (stream na terminal)
- [ ] Po zakończeniu: sprawdzić czy PROGRESS.md i SYSTEM_PROMPT.md zostały utworzone
- [ ] Wyświetlić podsumowanie: jakie pliki, ile tasków (parsuj PROGRESS.md)

**Patterns:**
```rust
// src/commands/task/prd.rs
pub async fn execute(args: PrdArgs, file_config: &FileConfig) -> Result<()> {
    let input = resolve_input(&args)?;  // file/prompt/stdin
    let output_dir = resolve_output_dir(&args, file_config);

    // Create boilerplate files (only if missing)
    create_boilerplate_if_missing(&output_dir, file_config)?;

    // Build prompt for Claude
    let prompt = build_prd_prompt(&input, &output_dir);

    // Run Claude (streaming)
    run_claude_once(ClaudeOnceOptions {
        prompt,
        model: args.model,
        write_access: true,
    }).await?;

    // Verify outputs
    verify_outputs(&output_dir, file_config)?;

    // Print summary
    print_prd_summary(&output_dir, file_config)?;
    Ok(())
}
```

**Notes:**
- stdin detection: `!std::io::stdin().is_terminal()` → czytaj stdin
- Claude musi wiedzieć w jakim katalogu tworzyć pliki → podaj output_dir w prompcie
- Prompt powinien informować Claude'a o istniejącym CLAUDE.md (jeśli jest)

---

### Phase 8: `task add` Implementation
**Goal:** Implementacja komendy dodającej nowe zadania.

**Locations:** `src/commands/task/add.rs`

**Tasks:**
- [ ] Implementować `execute(args: AddArgs, file_config: &FileConfig) -> Result<()>`
- [ ] Obsługa input: --file / --prompt / stdin (identycznie jak prd)
- [ ] Sprawdzenie czy PROGRESS.md istnieje → błąd jeśli nie
- [ ] Zbudować prompt z embedded add_prompt.md + user's requirements
- [ ] Wywołać `run_claude_once()` z write_access=true (Claude modyfikuje PROGRESS.md i CURRENT_TASK.md)
- [ ] Po zakończeniu: parsuj PROGRESS.md, przelicz tasks, zaktualizuj state file
- [ ] Wyświetlić podsumowanie: nowy task count, nowe min_iterations

**Patterns:**
```rust
pub async fn execute(args: AddArgs, file_config: &FileConfig) -> Result<()> {
    let progress_path = &file_config.task.progress_file;
    if !progress_path.exists() {
        return Err(RalphError::MissingFile(
            format!("{} not found. Run `ralph-wiggum task prd` first.", progress_path.display())
        ));
    }

    let input = resolve_input(&args)?;
    let prompt = build_add_prompt(&input);

    run_claude_once(ClaudeOnceOptions {
        prompt,
        model: args.model,
        write_access: true,
    }).await?;

    // Re-parse and update state
    let summary = load_progress(progress_path)?;
    update_state_file(file_config, &summary)?;
    print_add_summary(&summary)?;
    Ok(())
}
```

**Notes:** State file update: czytaj `.claude/ralph-loop.local.md`, zaktualizuj min_iterations w YAML frontmatter, zapisz.

---

### Phase 9: `task status` Implementation
**Goal:** TUI dashboard ze stanem projektu.

**Locations:** `src/commands/task/status.rs`

**Tasks:**
- [ ] Implementować `execute(file_config: &FileConfig) -> Result<()>`
- [ ] Parsuj PROGRESS.md → ProgressSummary
- [ ] Parsuj CURRENT_TASK.md → task ID, status, attempts (prosty regex na nagłówki `## Task ID:`, `## Status:`, `## Attempts:`)
- [ ] Opcjonalnie parsuj CHANGENOTES.md → oblicz avg time per task
- [ ] Ratatui Viewport::Inline multiline:
  - Linia 1: Podsumowanie: `Done: N | In Progress: N | Blocked: N | Todo: N`
  - Linia 2: Current task: `▶ ID [component] name | Status: X | Attempts: Y/3`
  - Linia 3: Gauge progress bar `[████████░░░░░░░░] 8/20 (40%)`
  - Linia 4: Avg time/task (jeśli dostępne), total elapsed
- [ ] Obsługa klawiszy: q/Esc/Ctrl+C → wyjście
- [ ] Wyświetlić i wyjść (bez interaktywnego odświeżania — to inline, nie live)

**Patterns:**
```rust
pub fn execute(file_config: &FileConfig) -> Result<()> {
    let summary = load_progress(&file_config.task.progress_file)?;
    let current = current_task(&summary);

    // Render with ratatui Inline viewport
    let mut terminal = ratatui::init();
    // ... draw widgets ...
    ratatui::restore();
    Ok(())
}
```

**Notes:**
- Inline multiline = nie fullscreen, po wyświetleniu wraca do terminala
- Może być synchroniczny (nie async) — brak potrzeby tokio tutaj
- Rozważ `crossterm::terminal::disable_raw_mode()` cleanup

---

### Phase 10: Core Loop Modification — Adaptive min_iterations
**Goal:** Hook w `run::execute()` do dynamicznej adaptacji min_iterations na bazie PROGRESS.md.

**Locations:** `src/commands/run/mod.rs`, `src/commands/run/config.rs`, `src/commands/run/state.rs`

**Tasks:**
- [ ] Dodać `progress_file: Option<PathBuf>` do `Config`
- [ ] Dodać `set_min_iterations(&mut self, n: u32)` do `StateManager`
- [ ] W `run::execute()`, po każdej iteracji (po `run_result`): jeśli `config.progress_file.is_some()` → `load_progress()` → oblicz `remaining = summary.todo + summary.in_progress` → `state_manager.set_min_iterations(current_iteration + remaining)`
- [ ] Zalogować adaptację w output formatter (opcjonalnie)

**Patterns:**
```rust
// W run::execute() po run_result handling:
if let Some(ref progress_file) = config.progress_file {
    if let Ok(summary) = crate::shared::progress::load_progress(progress_file) {
        let remaining = summary.todo + summary.in_progress;
        let new_min = state_manager.iteration() + remaining as u32;
        state_manager.set_min_iterations(new_min);
    }
    // Błąd parsowania = cichy skip (tolerancyjny)
}
```

**Notes:**
- `set_min_iterations` NIE powinien zmniejszać min_iterations poniżej current iteration
- Cichy fallback — jeśli PROGRESS.md nie istnieje lub nie parsuje się, kontynuuj bez adaptacji
- `task continue` ustawia `config.progress_file` automatycznie

---

### Phase 11: Enhanced Status Bar — Task Progress
**Goal:** Rozszerzenie status bar o informacje o postępie tasków (3 linie) gdy `progress_file` jest ustawiony.

**Locations:** `src/commands/run/ui.rs`, `src/commands/run/output.rs`

**Tasks:**
- [ ] Dodać `TaskProgress` struct do `StatusData`: `total`, `done`, `in_progress`, `blocked`, `todo`, `current_task_id`, `current_task_name`, `avg_time_per_task: Option<Duration>`
- [ ] Warunkowy Viewport::Inline(3) gdy `task_progress` jest Some, Inline(1) gdy None
- [ ] Linia 2: renderuj current task info jako `Span` z kolorami (component w kolorze, ID bold)
- [ ] Linia 3: ratatui `Gauge` widget — `ratio = done as f64 / total as f64`, kolor zielony
- [ ] Aktualizacja `StatusTerminal::update()` dla 3-liniowego layout
- [ ] Odświeżanie task progress w output formatter po każdej iteracji
- [ ] Key shortcut `r` w InputThread do wymuszenia re-read PROGRESS.md

**Patterns:**
```rust
#[derive(Debug, Clone, Default)]
pub struct TaskProgress {
    pub total: usize,
    pub done: usize,
    pub in_progress: usize,
    pub blocked: usize,
    pub todo: usize,
    pub current_task_id: Option<String>,
    pub current_task_name: Option<String>,
}

// W StatusData:
pub task_progress: Option<TaskProgress>,
```

**Notes:**
- Viewport zmiana z Inline(1) na Inline(3) musi być obsłużona w `StatusTerminal::new()` — dynamiczny `Viewport::Inline(N)` na bazie config
- Gdy przejście Inline(1)→Inline(3) w trakcie sesji nie jest możliwe → ustaw na starcie na bazie `config.progress_file.is_some()`
- Gauge widget wymaga `ratatui::widgets::Gauge` — sprawdzić czy dostępny w ratatui 0.30

---

### Phase 12: `task continue` Implementation
**Goal:** Implementacja komendy łączącej wszystko — automatyczny wrapper na `run`.

**Locations:** `src/commands/task/continue_cmd.rs`

**Tasks:**
- [ ] Implementować `execute(file_config: &FileConfig) -> Result<()>`
- [ ] Sprawdzić istnienie PROGRESS.md i SYSTEM_PROMPT.md → `RalphError::MissingFile` jeśli brak
- [ ] Czytaj SYSTEM_PROMPT.md → prompt
- [ ] Parsuj PROGRESS.md → oblicz min_iterations = todo + in_progress
- [ ] Zbuduj `RunArgs` programatycznie:
  - `prompt = Some(system_prompt_content)`
  - `min_iterations = remaining_tasks`
  - `max_iterations = remaining_tasks + 5`
  - `continue_session = true`
  - `resume = true` (jeśli state file istnieje, false jeśli nie)
  - `state_file = config.state_file` (domyślny)
  - `config = config_path`
  - `no_nf = false` (z file_config)
- [ ] Zbuduj `Config` z `RunArgs` + ustaw `progress_file` z `file_config.task.progress_file`
- [ ] Wywołaj `commands::run::execute()` — LUB zbuduj Config i wywołaj bezpośrednio
- [ ] Wyświetl banner + informację startową (ile tasków, current task)

**Patterns:**
```rust
pub async fn execute(file_config: &FileConfig) -> Result<()> {
    let progress_path = &file_config.task.progress_file;
    let system_prompt_path = &file_config.task.system_prompt_file;

    if !progress_path.exists() {
        return Err(RalphError::MissingFile(
            format!("{} not found. Run `ralph-wiggum task prd` first.", progress_path.display())
        ));
    }
    if !system_prompt_path.exists() {
        return Err(RalphError::MissingFile(
            format!("{} not found. Run `ralph-wiggum task prd` first.", system_prompt_path.display())
        ));
    }

    let prompt = std::fs::read_to_string(system_prompt_path)?;
    let summary = load_progress(progress_path)?;
    let remaining = (summary.todo + summary.in_progress) as u32;

    let args = RunArgs {
        prompt: Some(prompt),
        min_iterations: remaining.max(1),
        max_iterations: remaining + 5,
        continue_session: true,
        resume: state_file_exists,
        // ...
    };

    commands::run::execute(args).await
}
```

**Notes:**
- `task continue` NIE ma własnych flag — zero override
- Musi ustawić `Config.progress_file` — wymaga modyfikacji `Config::build()` lub post-build setter
- Jeśli resume=true ale state file nie istnieje → resume=false (fresh start)
- Rozważ dodanie `Config::with_progress_file(mut self, path: PathBuf) -> Self` builder

---

### Phase 13: Integration — main.rs Routing
**Goal:** Połączenie wszystkiego — routing w main.rs.

**Locations:** `src/main.rs`

**Tasks:**
- [ ] Dodać `Commands::Task { command }` do match w main
- [ ] Routing: `TaskCommands::Prd(args)` → `commands::task::prd::execute(args)`
- [ ] Routing: `TaskCommands::Continue` → `commands::task::continue_cmd::execute()`
- [ ] Routing: `TaskCommands::Add(args)` → `commands::task::add::execute(args)`
- [ ] Routing: `TaskCommands::Status` → `commands::task::status::execute()`
- [ ] Obsługa nowych error variants w error handling

**Patterns:**
```rust
Some(Commands::Task { command }) => {
    let file_config = FileConfig::load_from_path(&PathBuf::from(".ralph.toml"))?;
    match command {
        TaskCommands::Prd(args) => commands::task::prd::execute(args, &file_config).await,
        TaskCommands::Continue => commands::task::continue_cmd::execute(&file_config).await,
        TaskCommands::Add(args) => commands::task::add::execute(args, &file_config).await,
        TaskCommands::Status => commands::task::status::execute(&file_config),
    }
}
```

---

### Phase 14: Unit Tests
**Goal:** Testy dla nowych modułów.

**Locations:** Inline `#[cfg(test)]` w każdym nowym pliku

**Tasks:**
- [ ] Testy parsera PROGRESS.md:
  - Poprawny format z różnymi statusami
  - Dowolna głębokość ID (1.1, 1.1.1, 1.1.1.1)
  - Housekeeping tasks (H.1, H.2)
  - Tolerancja: puste linie, nagłówki, separatory
  - Edge case: pusty plik, brak tasków
  - current_task(): pierwszy [~] > pierwszy [ ]
- [ ] Testy FileConfig z sekcją [task]:
  - Domyślne wartości
  - Częściowa konfiguracja
  - Pełna konfiguracja
  - Backward compat (brak sekcji [task])
- [ ] Testy templates: verify include_str!() ładuje poprawną treść
- [ ] Testy CLI args: parsing task prd/continue/add/status
- [ ] Testy adaptacji min_iterations: set_min_iterations na StateManager
- [ ] Snapshot testy promptów: verify że build_prd_prompt() i build_add_prompt() generują poprawne prompty

---

### Phase 15: Cleanup & Polish
**Goal:** Finalne szlify.

**Locations:** Cały projekt

**Tasks:**
- [ ] `cargo clippy` — zero ostrzeżeń
- [ ] `cargo test` — wszystkie testy przechodzą
- [ ] Zweryfikować backward compat: `ralph-wiggum --prompt "test"` działa bez zmian
- [ ] Zweryfikować `ralph-wiggum run --prompt "test"` działa bez zmian
- [ ] Zweryfikować `ralph-wiggum task prd --help` wyświetla poprawny help z bannerem
- [ ] Zweryfikować `ralph-wiggum task status` gdy brak PROGRESS.md → przyjazny błąd
- [ ] Zweryfikować `ralph-wiggum task continue` gdy brak plików → przyjazny błąd z sugestią
- [ ] Sprawdzić czy Gauge widget jest w ratatui 0.30 (jeśli nie → fallback na ręczny progress bar)

---

## Final Verification

1. **Build**: `cargo build` — kompilacja bez błędów
2. **Tests**: `cargo test` — wszystkie testy przechodzą
3. **Lint**: `cargo clippy --all-targets -- -D warnings` — zero ostrzeżeń
4. **Backward compat**:
   - `ralph-wiggum --prompt "test"` → działa jak dotychczas
   - `ralph-wiggum run --prompt "test"` → działa jak dotychczas
   - `ralph-wiggum update` → działa jak dotychczas
5. **Nowe komendy**:
   - `ralph-wiggum task prd --prompt "Zbuduj REST API w Rust"` → generuje pliki
   - `ralph-wiggum task prd --file PRD.md` → generuje pliki z dokumentu
   - `echo "Build a CLI tool" | ralph-wiggum task prd` → stdin pipe
   - `ralph-wiggum task status` → wyświetla dashboard
   - `ralph-wiggum task add --prompt "Dodaj autentykację"` → dodaje taski
   - `ralph-wiggum task continue` → uruchamia loop z automatycznymi parametrami
6. **Adaptive iterations**: podczas `task continue` po iteracji w której Claude oznaczy 2+ tasków jako [x], min_iterations powinno się zmniejszyć odpowiednio
7. **Status bar**: przy `task continue` status bar ma 3 linie z Gauge i task info
8. **Error handling**: brak plików → przyjazne komunikaty z sugestią `task prd`
