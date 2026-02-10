# SPEC: Task Orchestration — Parallel Ralph Workers

## Requirements

### Overview
System orkiestracji wielu instancji Ralph pracujących równolegle nad zadaniami z PROGRESS.md. Orkiestrator (centralny proces) zarządza DAG-iem zależności, przydziela taski do workerów (tokio tasks), każdy worker działa w izolowanym git worktree, a po zakończeniu wynik jest squash-mergowany do main.

### Architektura wysokopoziomowa
```
                     ┌─────────────────────────┐
                     │     Orchestrator         │
                     │  (main tokio runtime)    │
                     │                          │
                     │  ┌─────────────────┐     │
                     │  │  DAG Scheduler  │     │
                     │  │  (FIFO queue)   │     │
                     │  └────────┬────────┘     │
                     │           │              │
                     │  ┌────────┴────────┐     │
                     │  │  mpsc channel   │     │
                     │  └──┬──────┬───┬───┘     │
                     └─────┼──────┼───┼─────────┘
                           │      │   │
              ┌────────────┘      │   └────────────┐
              ▼                   ▼                 ▼
    ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
    │   Worker W1     │ │   Worker W2     │ │   Worker W3     │
    │  (tokio task)   │ │  (tokio task)   │ │  (tokio task)   │
    │                 │ │                 │ │                 │
    │  Worktree:      │ │  Worktree:      │ │  Worktree:      │
    │  ../proj-rw-w1/ │ │  ../proj-rw-w2/ │ │  ../proj-rw-w3/ │
    │                 │ │                 │ │                 │
    │  Branch:        │ │  Branch:        │ │  Branch:        │
    │  ralph/w1/T03   │ │  ralph/w2/T05   │ │  ralph/w3/T07   │
    │                 │ │                 │ │                 │
    │  Phases:        │ │  Phases:        │ │  Phases:        │
    │  1. implement   │ │  1. implement   │ │  1. implement   │
    │  2. review+fix  │ │  2. review+fix  │ │  2. review+fix  │
    │  3. verify      │ │  3. verify      │ │  3. verify      │
    └─────────────────┘ └─────────────────┘ └─────────────────┘
```

### Komendy CLI
- `ralph-wiggum task orchestrate` — główna komenda orkiestracji
- `ralph-wiggum task clean` — interaktywne czyszczenie orphaned worktrees/branches/state

### Flagi `task orchestrate`
| Flaga | Typ | Default | Opis |
|-------|-----|---------|------|
| `--workers N` | u32 | z config, fallback 2 | Liczba równoległych workerów |
| `--model` | String | - | Domyślny model Claude (nadpisywany per-task) |
| `--max-retries` | u32 | 3 | Max retry per task przed [!] Blocked |
| `--verbose` | bool | false | Pełny event log (JSONL) |
| `--resume` | bool | false | Wznów sesję z `.ralph/orchestrate.yaml` |
| `--dry-run` | bool | false | Generuj DAG + pokaż plan, nie uruchamiaj |
| `--worktree-prefix` | String | `{project}-ralph-w` | Prefix nazw worktree |
| `--no-merge` | bool | false | Nie merguj do main (tylko worktrees) |
| `--max-cost` | f64 | - | Max koszt sesji w USD |
| `--timeout` | String | - | Max czas sesji (np. "2h", "30m") |
| `--tasks` | String | - | Filtr tasków: `T01,T03,T07` |

### PROGRESS.md — rozszerzony format YAML frontmatter
```yaml
---
deps:
  T02: [T01]
  T03: [T01, T02]
  T04: []
  T05: [T03]
models:
  T01: claude-opus-4-6
  T07: claude-haiku-4-5-20251001
default_model: claude-sonnet-4-5-20250929
---

# PROGRESS
- [ ] T01 [api] Setup JWT authentication
- [ ] T02 [api] Create user endpoints
- [ ] T03 [ui] Build login form
- [ ] T04 [test] Setup test framework
- [ ] T05 [ui] Dashboard page
```

### Worker lifecycle (3 fazy per task)
1. **Implement**: Claude CLI z task-specific promptem — implementuje zadanie
2. **Review + Fix**: Claude CLI review — przegląda swoją pracę, naprawia problemy
3. **Verify**: Worker uruchamia komendy weryfikacyjne z SYSTEM_PROMPT.md

Jeśli verify fail → retry full cycle (max `--max-retries`), potem [!] Blocked.

### Worker prompt
Worker NIE dostaje PROGRESS.md. Dostaje:
- SYSTEM_PROMPT.md (generyczny, kontekst projektu)
- Wstrzyknięty task description: ID, component, name, full context

### Git strategy
- Worktree z najnowszego HEAD (po ostatnim merge)
- Branch: `ralph/w{N}/{task_id}`
- Claude commituje w trakcie pracy + orkiestrator robi final commit
- Squash merge do main: `task(T01): Setup JWT authentication`
- Worktree zachowany do successful merge (dla conflict resolution)
- Po merge: worktree usunięty

### Komunikacja worker→orkiestrator
- **Primary**: tokio mpsc channel — real-time eventy
- **Persistence**: JSONL log file per worker (`.ralph/logs/w{N}.jsonl`)
- **Combined log**: `.ralph/orchestrate.log` (all workers)

### Orkiestrator AI-assisted
Claude wywoływany w dwóch scenariuszach:
1. **Planning**: Generacja deps DAG z PROGRESS.md (jeśli brak YAML frontmatter)
2. **Conflict resolution**: Diff + kontekst gdy merge conflict

### Scheduling
- Push model: orkiestrator przydziela
- FIFO queue sortowana po ID (najniższy ID najpierw)
- First-free worker assignment
- Two-phase progress: `[ ]→[~]` na start, `[~]→[x]` po merge

### State & Resume
- Session state: `.ralph/orchestrate.yaml`
- Lockfile: `.ralph/orchestrate.lock` z PID + heartbeat (5s refresh)
- Full resume z YAML state
- Exclusive lock — blokuje `task continue` i inne `orchestrate`

### UI
- Multiplexed output: `[W1|T03] text...` z hash-based kolorami
- Dynamic N+2 status bar (1 linia per worker + progress + cost)
- Full cost dashboard (real-time, per-worker, trend $/task)
- Per-task summary table na koniec sesji
- Per-worker + combined logs do plików

### Shutdown
- First Ctrl+C: graceful drain (nie przydziela nowych, czeka na bieżące)
- Second Ctrl+C: force kill wszystkich workerów
- State zapisany w obu przypadkach

### Conflict resolution
- Orkiestrator próbuje squash merge (Rust git commands)
- Jeśli conflict: tworzy diff (our vs theirs) + kontekst taska
- Claude dostaje prompt z diff i rozwiązuje conflict
- Merge w workerowym worktree (zachowanym do merge)

### Hot reload
- Orkiestrator monitoruje PROGRESS.md (mtime, 15s interval)
- Nowe taski automatycznie dodawane do kolejki
- DAG dynamicznie aktualizowany

### `task clean` — interaktywne czyszczenie
- Wyszukuje orphaned worktrees, branches `ralph/w*/T*`, state files, lock files, logs
- Wyświetla co znaleziono
- User potwierdza co usunąć

### Integracja z istniejącymi komendami
- `task prd` generuje YAML deps frontmatter od razu
- `task add` auto-aktualizuje deps dla nowych tasków

---

## Project Conventions

### Wzorce kodu (z analizy codebase)
- **Modularyzacja**: `src/commands/<name>/mod.rs` + args.rs, config.rs, etc.
- **Error handling**: `thiserror::Error` enum `RalphError`, `type Result<T>`
- **Config**: `FileConfig` z `serde::Deserialize` na `.ralph.toml`, sekcje zagnieżdżone
- **Clap**: derive macros `#[derive(Parser/Args/Subcommand)]`
- **Shared utilities**: `src/shared/` (error, icons, markdown, banner, file_config, progress)
- **Async**: tokio runtime, `async fn execute()` w modułach komend
- **State sharing**: `Arc<Mutex<T>>` z konsolidacją locków, `Arc<AtomicBool>` dla flag
- **TUI**: ratatui `Viewport::Inline(N)`, crossterm, `insert_before` dla contentu
- **Templates**: `include_str!()` w `src/templates/`
- **Tests**: `#[cfg(test)] mod tests` inline w plikach źródłowych
- **Edition**: Rust 2024 (1.85+), `LazyLock` dostępny
- **Input thread**: Dedykowany OS thread (`std::thread::spawn`) dla crossterm — nigdy tokio::spawn
- **Lock contention**: Max 2 lock/unlock per event cycle
- **Git commits**: Angular convention: `type(scope): description`

---

## Complexity Assessment
- **Scope**: Very Large
- **Areas affected**:
  - `src/commands/task/` — nowy moduł `orchestrate/` + `clean.rs`
  - `src/commands/task/args.rs` — nowe subkomendy Orchestrate + Clean
  - `src/commands/task/prd.rs` — rozszerzenie o generację deps
  - `src/commands/task/add.rs` — rozszerzenie o update deps
  - `src/shared/progress.rs` — YAML frontmatter parser (deps, models)
  - `src/shared/error.rs` — nowe warianty błędów
  - `src/shared/file_config.rs` — sekcja `[task.orchestrate]`
  - `src/commands/run/runner.rs` — adapted ClaudeRunner for workers
  - `src/cli.rs` — nowe subkomendy
  - Nowe moduły: `dag.rs`, `worktree.rs`, `worker.rs`, `merge.rs`, `scheduler.rs`

---

## Implementation Plan

### Phase 1: YAML Frontmatter Parser
**Goal:** Rozszerzenie parsera PROGRESS.md o odczyt YAML frontmatter (deps, models, default_model).

**Locations:** `src/shared/progress.rs`

**Tasks:**
- [ ] Dodać `ProgressFrontmatter` struct: `deps: HashMap<String, Vec<String>>`, `models: HashMap<String, String>`, `default_model: Option<String>`
- [ ] Dodać `frontmatter: Option<ProgressFrontmatter>` do `ProgressSummary`
- [ ] Implementować `parse_frontmatter(content: &str) -> (Option<ProgressFrontmatter>, &str)` — wyodrębnia YAML między `---` markerami, reszta to body
- [ ] Zmodyfikować `parse_progress()` aby najpierw parsować frontmatter, potem body
- [ ] Dodać `write_frontmatter(fm: &ProgressFrontmatter) -> String` — serializacja frontmatter z powrotem do YAML
- [ ] Dodać `update_progress_frontmatter(path: &Path, fm: &ProgressFrontmatter) -> Result<()>` — atomowy update frontmatter zachowując body
- [ ] Unit testy: frontmatter z deps, z models, z oboma, bez frontmatter (backward compat), malformed YAML

**Patterns:**
```rust
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProgressFrontmatter {
    #[serde(default)]
    pub deps: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub models: HashMap<String, String>,
    #[serde(default)]
    pub default_model: Option<String>,
}
```

**Notes:**
- Backward compat: brak frontmatter = `ProgressFrontmatter::default()` (empty deps, no models)
- YAML parsing przez istniejący `serde_yaml` crate
- Tolerancyjny: malformed YAML → log warning, treat as no frontmatter

---

### Phase 2: DAG Data Structures & Algorithms
**Goal:** Moduł DAG z topological sort, cycle detection, ready task computation.

**Locations:** `src/shared/dag.rs` (nowy plik)

**Tasks:**
- [ ] Dodać `pub mod dag` do `src/shared/mod.rs`
- [ ] Zdefiniować `TaskDag` struct z adjacency list (HashMap<String, Vec<String>> — deps)
- [ ] Implementować `TaskDag::from_frontmatter(fm: &ProgressFrontmatter) -> Self`
- [ ] Implementować `detect_cycles(&self) -> Option<Vec<String>>` — DFS cycle detection, zwraca cycle path lub None
- [ ] Implementować `topological_sort(&self) -> Result<Vec<String>>` — Kahn's algorithm, error jeśli cykl
- [ ] Implementować `ready_tasks(&self, done: &HashSet<String>, in_progress: &HashSet<String>) -> Vec<String>` — taski z all deps done, nie in-progress, nie done
- [ ] Implementować `dependents(&self, task_id: &str) -> Vec<String>` — kto zależy od tego taska
- [ ] Implementować `critical_path(&self) -> Vec<String>` — najdłuższa ścieżka w DAG (opcjonalne, dla przyszłego priority scheduling)
- [ ] Unit testy: liniowy DAG, diamond DAG, cycle detection, empty DAG, disconnected components, ready_tasks z partial completion

**Patterns:**
```rust
pub struct TaskDag {
    /// task_id → list of tasks it depends on (predecessors)
    deps: HashMap<String, Vec<String>>,
    /// task_id → list of tasks that depend on it (successors)
    reverse: HashMap<String, Vec<String>>,
}
```

**Notes:**
- `ready_tasks` sortuje po ID (FIFO, najniższy ID najpierw)
- Taski bez wpisu w deps traktowane jako niezależne (zero deps)

---

### Phase 3: Worker Event Protocol
**Goal:** Definicja eventów między workerami a orkiestratorem (mpsc channel + JSONL persistence).

**Locations:** `src/commands/task/orchestrate/events.rs` (nowy)

**Tasks:**
- [ ] Zdefiniować `WorkerEvent` enum:
  - `TaskStarted { worker_id, task_id, started_at }`
  - `PhaseStarted { worker_id, task_id, phase: WorkerPhase }`
  - `PhaseCompleted { worker_id, task_id, phase: WorkerPhase, success: bool }`
  - `ClaudeEvent { worker_id, task_id, event: ClaudeEventData }` — forwarded z ClaudeRunner
  - `TaskCompleted { worker_id, task_id, success, cost, tokens, files_changed, commit_hash }`
  - `TaskFailed { worker_id, task_id, error, retries_left }`
  - `CostUpdate { worker_id, cost_usd, input_tokens, output_tokens }`
  - `MergeStarted { worker_id, task_id }`
  - `MergeCompleted { worker_id, task_id, success, commit_hash }`
  - `MergeConflict { worker_id, task_id, conflicting_files }`
- [ ] Zdefiniować `WorkerPhase` enum: `Implement`, `ReviewFix`, `Verify`
- [ ] Zdefiniować `OrchestratorCommand` enum (orkiestrator → worker):
  - `AssignTask { task_id, task_description, model, worktree_path, branch }`
  - `Shutdown`
  - `Abort` (force stop current task)
- [ ] Implementować serializację JSONL dla `WorkerEvent` (serde_json + timestamp)
- [ ] Implementować `EventLogger` struct: zapis do per-worker JSONL + combined log
- [ ] Unit testy: serializacja/deserializacja wszystkich event types

**Patterns:**
```rust
#[derive(Debug, Clone, Serialize)]
pub struct WorkerEvent {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    #[serde(flatten)]
    pub kind: WorkerEventKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event")]
pub enum WorkerEventKind {
    TaskStarted { worker_id: u32, task_id: String },
    // ...
}
```

**Notes:**
- JSONL format: jedna linia JSON per event, append-only
- Configurable verbosity: standard vs verbose (z `--verbose` flag)
- Standard: task/phase/cost events. Verbose: + forwarded Claude events

---

### Phase 4: Adapted ClaudeRunner for Workers
**Goal:** Nowa wersja ClaudeRunner przystosowana do pracy jako worker — eventy idą do mpsc channel zamiast na terminal.

**Locations:** `src/commands/task/orchestrate/worker_runner.rs` (nowy)

**Tasks:**
- [ ] Zdefiniować `WorkerRunner` struct — wrapper nad `ClaudeRunner` z `mpsc::Sender<WorkerEvent>`
- [ ] Implementować `run_phase(phase: WorkerPhase, prompt: &str, model: Option<&str>, cwd: &Path) -> Result<String>` — uruchamia Claude CLI w worktree, forwarduje eventy przez channel
- [ ] Implementować `run_implement(task_desc: &str, system_prompt: &str) -> Result<String>`
- [ ] Implementować `run_review(implementation_output: &str, task_desc: &str) -> Result<String>`
- [ ] Implementować `run_verify(system_prompt: &str, cwd: &Path) -> Result<bool>` — uruchamia Claude z instrukcją weryfikacji
- [ ] Obsługa `current_dir(worktree_path)` — Claude pracuje w worktree
- [ ] Obsługa `model` per task (z YAML frontmatter)
- [ ] Prompt injection: task description + minimal awareness ("pracujesz w izolowanym worktree, skup się tylko na swoim tasku")

**Patterns:**
```rust
pub struct WorkerRunner {
    worker_id: u32,
    task_id: String,
    event_tx: mpsc::Sender<WorkerEvent>,
    shutdown: Arc<AtomicBool>,
}

impl WorkerRunner {
    pub async fn run_phase(
        &self,
        phase: WorkerPhase,
        prompt: &str,
        model: Option<&str>,
        cwd: &Path,
    ) -> Result<String> {
        self.send_event(WorkerEventKind::PhaseStarted { ... }).await;
        let runner = ClaudeRunner::oneshot(prompt, model, Some(cwd.to_path_buf()));
        // Adapt on_event to forward through channel
        let result = runner.run(self.shutdown.clone(), |event| { ... }, || {}).await;
        self.send_event(WorkerEventKind::PhaseCompleted { ... }).await;
        result
    }
}
```

**Notes:**
- ClaudeRunner.output_dir = worktree path
- Bez `--continue` — fresh per iteration (per phase)
- Bez promise detection — 1 iteracja per phase

---

### Phase 5: Git Worktree Manager
**Goal:** Moduł zarządzania git worktrees — tworzenie, usuwanie, branch management.

**Locations:** `src/commands/task/orchestrate/worktree.rs` (nowy)

**Tasks:**
- [ ] Zdefiniować `WorktreeManager` struct z root project path i prefix
- [ ] Implementować `create_worktree(worker_id: u32, task_id: &str) -> Result<WorktreeInfo>` — `git worktree add` z nowym branchem
- [ ] Implementować `remove_worktree(path: &Path) -> Result<()>` — `git worktree remove` + cleanup
- [ ] Implementować `remove_branch(branch: &str) -> Result<()>` — `git branch -D`
- [ ] Implementować `worktree_path(worker_id: u32) -> PathBuf` — `../{project}-ralph-w{N}/`
- [ ] Implementować `branch_name(worker_id: u32, task_id: &str) -> String` — `ralph/w{N}/{task_id}`
- [ ] Implementować `list_orphaned() -> Result<Vec<OrphanedWorktree>>` — find worktrees bez aktywnej sesji
- [ ] Implementować `prune() -> Result<()>` — `git worktree prune`
- [ ] Worktree tworzone z `HEAD` aktualnego main (najnowszy stan po ostatnim merge)
- [ ] Unit testy: path generation, branch naming

**Patterns:**
```rust
pub struct WorktreeManager {
    project_root: PathBuf,
    prefix: String,  // default: "{project_name}-ralph-w"
}

pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub worker_id: u32,
    pub task_id: String,
}

impl WorktreeManager {
    pub async fn create_worktree(&self, worker_id: u32, task_id: &str) -> Result<WorktreeInfo> {
        let path = self.worktree_path(worker_id);
        let branch = self.branch_name(worker_id, task_id);
        // git worktree add -b {branch} {path} HEAD
        let output = Command::new("git")
            .args(["worktree", "add", "-b", &branch])
            .arg(&path)
            .arg("HEAD")
            .current_dir(&self.project_root)
            .output().await?;
        // ...
    }
}
```

**Notes:**
- Sibling dirs: worktree obok projektu, nie wewnątrz
- `git worktree add -b ralph/w1/T03 ../project-ralph-w1 HEAD`
- Ephemeral: tworzony per task, usuwany po successful merge
- Zachowany jeśli merge fail (do conflict resolution)

---

### Phase 6: Worker Task Executor (3-phase lifecycle)
**Goal:** Pełny executor workera — 3 fazy + commit + retry logic.

**Locations:** `src/commands/task/orchestrate/worker.rs` (nowy)

**Tasks:**
- [ ] Zdefiniować `Worker` struct: `id: u32`, `event_tx`, `shutdown`, `worktree_manager`, `system_prompt`
- [ ] Implementować `execute_task(task_id: &str, task_desc: &str, model: Option<&str>) -> Result<TaskResult>`
- [ ] Phase 1 — Implement: uruchom `WorkerRunner::run_implement()` w worktree
- [ ] Phase 2 — Review + Fix: uruchom `WorkerRunner::run_review()` z outputem phase 1
- [ ] Phase 3 — Verify: uruchom `WorkerRunner::run_verify()` — Claude uruchamia komendy weryfikacyjne
- [ ] Po każdej fazie: `git add -A && git commit` w worktree (final commit)
- [ ] Retry logic: jeśli verify fail → retry full cycle (3 fazy od nowa), max N razy
- [ ] Raportowanie eventów przez channel: TaskStarted, PhaseStarted/Completed, CostUpdate, TaskCompleted/Failed
- [ ] Cleanup: worker NIE usuwa worktree (orkiestrator decyduje po merge)

**Patterns:**
```rust
pub struct TaskResult {
    pub task_id: String,
    pub success: bool,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub commit_hash: Option<String>,
    pub retries: u32,
    pub files_changed: Vec<String>,
}
```

**Notes:**
- Worker jest "minimally aware": prompt zawiera info o izolacji worktree
- Worker NIE dostaje PROGRESS.md — tylko task description
- Worker NIE modyfikuje PROGRESS.md
- `git add -A && git commit -m "wip: {task_id} phase {N}"` po każdej fazie

---

### Phase 7: Task Queue & Scheduler
**Goal:** Kolejka zadań z FIFO + DAG awareness.

**Locations:** `src/commands/task/orchestrate/scheduler.rs` (nowy)

**Tasks:**
- [ ] Zdefiniować `TaskScheduler` struct: `dag: TaskDag`, `done: HashSet<String>`, `in_progress: HashSet<String>`, `blocked: HashSet<String>`, `queue: VecDeque<String>`
- [ ] Implementować `new(dag: TaskDag, progress: &ProgressSummary) -> Self` — inicjalizacja z aktualnym stanem PROGRESS.md
- [ ] Implementować `next_ready_task() -> Option<String>` — pierwszy task z kolejki gotowych (deps spełnione, nie in-progress/done/blocked)
- [ ] Implementować `mark_started(task_id: &str)` — przenieś do in_progress
- [ ] Implementować `mark_done(task_id: &str)` — przenieś do done, refresh ready queue
- [ ] Implementować `mark_blocked(task_id: &str)` — przenieś do blocked
- [ ] Implementować `mark_failed(task_id: &str)` — z powrotem do queue (retry) lub blocked (max retries)
- [ ] Implementować `is_complete() -> bool` — all tasks done or blocked
- [ ] Implementować `refresh_ready_queue()` — re-compute ready tasks z DAG
- [ ] Implementować `add_tasks(tasks: Vec<String>, deps: HashMap<String, Vec<String>>)` — hot reload support
- [ ] Unit testy: scheduling order, dependency respect, blocked propagation, hot reload

**Patterns:**
```rust
pub struct TaskScheduler {
    dag: TaskDag,
    done: HashSet<String>,
    in_progress: HashSet<String>,
    blocked: HashSet<String>,
    failed_retries: HashMap<String, u32>,  // task_id → retry count
    max_retries: u32,
    ready_queue: VecDeque<String>,  // sorted by ID
}
```

**Notes:**
- `ready_queue` re-computed after each `mark_done` — DAG unlock nowych tasków
- ID order: `ready_tasks` sortowane po ID (T01 < T02 < T10 — numerycznie jeśli możliwe, leksykograficznie fallback)
- `--tasks T01,T03` filter: scheduler ignoruje taski spoza filtra, ale zaciąga ich deps

---

### Phase 8: Orchestrator Core Loop
**Goal:** Główna pętla orkiestratora — spawning workerów, obsługa eventów, merge, assignment.

**Locations:** `src/commands/task/orchestrate/mod.rs` (nowy)

**Tasks:**
- [ ] Zdefiniować `Orchestrator` struct: scheduler, worktree_manager, workers, event channels, state, config
- [ ] Implementować `execute(args: OrchestrateArgs, file_config: &FileConfig) -> Result<()>` — entry point
- [ ] Main loop z `tokio::select!`:
  - mpsc event receiver → handle worker events
  - Ctrl+C signal → graceful/force shutdown
  - Timer (1s) → heartbeat, hot reload check, stale worker detection
- [ ] Worker spawning: gdy free worker + ready task → spawn tokio task z `Worker::execute_task()`
- [ ] Event handling:
  - `TaskCompleted(success=true)` → attempt squash merge
  - `TaskCompleted(success=false)` → retry lub mark blocked
  - `TaskFailed` → mark blocked po max retries
  - `CostUpdate` → aggregate costs
  - `ClaudeEvent` → forward do multiplexed output
- [ ] Merge flow: po TaskCompleted → squash merge → jeśli ok: mark done, remove worktree → jeśli conflict: AI resolve
- [ ] Graceful shutdown: stop assigning, wait for in-progress, save state
- [ ] Force shutdown: kill all workers, save state
- [ ] Session end: summary table, cleanup

**Patterns:**
```rust
pub async fn execute(args: OrchestrateArgs, file_config: &FileConfig) -> Result<()> {
    // 1. Load PROGRESS.md + parse frontmatter
    // 2. Validate/generate deps DAG
    // 3. Acquire lockfile
    // 4. Initialize scheduler, worktree manager, state
    // 5. Resume check
    // 6. Initialize UI (status terminal, multiplexed output)
    // 7. Spawn input thread
    // 8. Main orchestration loop
    //    - Assign tasks to free workers
    //    - Process worker events
    //    - Handle merges
    //    - Check completion/shutdown
    // 9. Summary & cleanup
}
```

**Notes:**
- Orkiestrator NIGDY nie blokuje na jednym workerze — `tokio::select!` na wszystkich
- First-free assignment: `HashMap<u32, WorkerState>` — worker_id → Idle/Busy(task_id)
- Hot reload PROGRESS.md: mtime check co 15s, nowe taski dodane do scheduler

---

### Phase 9: AI Planning (Dep Generation + Conflict Resolution)
**Goal:** Moduł AI — generacja deps DAG z PROGRESS.md + rozwiązywanie merge conflicts.

**Locations:** `src/commands/task/orchestrate/ai.rs` (nowy)

**Tasks:**
- [ ] Implementować `generate_deps(progress_content: &str, model: Option<&str>) -> Result<ProgressFrontmatter>`:
  - Buduje prompt z PROGRESS.md content
  - Instrukcja: "Przeanalizuj taski i zwróć JSON z zależnościami"
  - Claude zwraca `{"deps": {"T02": ["T01"], ...}}`
  - Parse JSON response → `ProgressFrontmatter`
- [ ] Implementować cycle detection z retry: jeśli cykl w deps → re-prompt Claude (max 3 próby)
- [ ] Implementować `resolve_conflict(our_diff: &str, their_diff: &str, task_desc: &str, conflicting_files: &[String], model: Option<&str>) -> Result<String>`:
  - Buduje prompt z diff context + task description
  - Claude zwraca resolved content
- [ ] Template prompt dla dep generation (embedded `include_str!`)
- [ ] Template prompt dla conflict resolution (embedded `include_str!`)
- [ ] Unit testy: JSON parsing odpowiedzi Claude, cycle detection retry

**Patterns:**
```rust
/// Generate dependency DAG using Claude AI
pub async fn generate_deps(
    progress_content: &str,
    model: Option<&str>,
) -> Result<ProgressFrontmatter> {
    let prompt = format!(
        "{}\n\n---\nPROGRESS.md content:\n{}\n\nReturn ONLY valid JSON.",
        DEPS_GENERATION_PROMPT, progress_content
    );
    // Use ClaudeRunner::oneshot with JSON parsing
    // Retry on cycle detection (max 3 times)
}
```

**Notes:**
- Deps generation: `--dry-run` generuje ale NIE zapisuje (chyba że brak w PROGRESS.md i pełny run)
- Conflict resolution: Claude w main worktree z conflict markers
- Używa `ClaudeRunner::oneshot` — bez streaming, czeka na kompletny output

---

### Phase 10: Squash Merge Engine
**Goal:** Moduł merge — squash merge z worktree do main, conflict detection, commit formatting.

**Locations:** `src/commands/task/orchestrate/merge.rs` (nowy)

**Tasks:**
- [ ] Implementować `squash_merge(worktree: &WorktreeInfo, task: &ProgressTask) -> Result<MergeResult>`
- [ ] Flow: `git merge --squash {branch}` w main worktree
- [ ] Commit message: `task({task_id}): {task_name}`
- [ ] Conflict detection: parse `git merge --squash` output, check exit code
- [ ] Zdefiniować `MergeResult` enum: `Success { commit_hash }`, `Conflict { files: Vec<String> }`
- [ ] Implementować `resolve_and_retry(worktree: &WorktreeInfo, conflict_files: &[String], task: &ProgressTask) -> Result<MergeResult>`:
  - Pobierz diff (our vs theirs)
  - Wywołaj `ai::resolve_conflict()`
  - Aplikuj resolved content
  - Retry merge
- [ ] Implementować `abort_merge() -> Result<()>` — `git merge --abort` na fallback
- [ ] Obsługa `--no-merge` flag — skip merge, zostawia worktrees
- [ ] Unit testy: commit message formatting

**Patterns:**
```rust
pub enum MergeResult {
    Success { commit_hash: String },
    Conflict { files: Vec<String> },
    Failed { error: String },
}

pub async fn squash_merge(
    project_root: &Path,
    worktree: &WorktreeInfo,
    task: &ProgressTask,
) -> Result<MergeResult> {
    // git merge --squash {branch}
    let output = Command::new("git")
        .args(["merge", "--squash", &worktree.branch])
        .current_dir(project_root)
        .output().await?;

    if output.status.success() {
        // git commit -m "task(T01): Setup JWT"
        let commit_msg = format!("task({}): {}", task.id, task.name);
        // ...
        Ok(MergeResult::Success { commit_hash })
    } else {
        // Parse conflict files from output
        Ok(MergeResult::Conflict { files })
    }
}
```

**Notes:**
- Merge w main worktree (project root), nie w workerowym worktree
- Sequential: tylko jeden merge na raz (mutex na git operations w main)
- Po successful merge: worktree + branch usuwane
- Po conflict: worktree zachowany do resolution

---

### Phase 11: Multiplexed Output Renderer
**Goal:** System renderowania outputu z wielu workerów z kolorowymi prefixami.

**Locations:** `src/commands/task/orchestrate/output.rs` (nowy)

**Tasks:**
- [ ] Zdefiniować `MultiplexedOutput` struct: per-worker color map, output buffer
- [ ] Implementować `worker_color(worker_id: u32) -> Color` — hash-based color z palety ANSI
- [ ] Implementować `format_worker_line(worker_id: u32, task_id: &str, text: &str) -> String` — `[W{N}|{task_id}] {text}`
- [ ] Implementować `format_orchestrator_line(text: &str) -> String` — `[ORCH] {text}` (biały/bold)
- [ ] Integracja z istniejącym `OutputFormatter` — reuse markdown rendering, tool formatting
- [ ] Per-worker `OutputFormatter` instances (dla token tracking, cost per worker)
- [ ] Implementować `format_claude_event(worker_id: u32, task_id: &str, event: &ClaudeEvent) -> Vec<String>` — reuse logiki z `output.rs`
- [ ] Logging: zapis do per-worker log + combined log + terminal

**Patterns:**
```rust
const WORKER_COLORS: &[Color] = &[
    Color::Red, Color::Green, Color::Yellow, Color::Blue,
    Color::Magenta, Color::Cyan, Color::White, Color::DarkGray,
];

pub struct MultiplexedOutput {
    worker_formatters: HashMap<u32, OutputFormatter>,
    combined_log: Option<File>,
    use_nerd_font: bool,
}

impl MultiplexedOutput {
    pub fn worker_color(worker_id: u32) -> Color {
        // Hash-based: deterministic per worker_id
        let hash = worker_id.wrapping_mul(2654435761); // Knuth multiplicative hash
        WORKER_COLORS[(hash as usize) % WORKER_COLORS.len()]
    }
}
```

**Notes:**
- Hash-based colors: deterministyczne, worker_id=1 zawsze ten sam kolor
- Prefix format: `[W1|T03]` — bold, kolorowy
- Combined log: timestamp + worker_id + task_id + content
- Terminal output: show all (no filtering)

---

### Phase 12: Orchestrator Status Bar (Dynamic N+2)
**Goal:** Status bar orkiestratora z dynamiczną ilością linii.

**Locations:** `src/commands/task/orchestrate/status.rs` (nowy)

**Tasks:**
- [ ] Zdefiniować `OrchestratorStatus` struct: overall progress, per-worker status, total cost, cost trend
- [ ] Implementować `OrchestratorStatusBar` — ratatui `Viewport::Inline(N+2)` gdzie N = workers count
- [ ] Linia 1: Overall progress bar + cost summary
  - `[████████░░░░░░] 8/20 (40%) │ $0.42 │ $0.053/task │ ⏱ 12m`
- [ ] Linie 2..N+1: Per-worker status
  - `W1 ▶ T03 [api] implement │ ↓1.2k ↑500 │ $0.012`
  - `W2 ✓ T05 [ui] done │ merging...`
  - `W3 ⏳ idle │ waiting for deps`
- [ ] Linia N+2: Queue info
  - `Queue: 5 ready │ 3 blocked │ 2 in-progress │ ETA ~25m`
- [ ] Aktualizacja przez `update(status: &OrchestratorStatus)` — lock-consolidated
- [ ] Obsługa terminal resize
- [ ] Integracja z istniejącą `StatusTerminal` architekturą (reuse `insert_before`, `strip_trailing_spaces`)

**Patterns:**
```rust
pub struct OrchestratorStatus {
    pub total_tasks: usize,
    pub done_tasks: usize,
    pub blocked_tasks: usize,
    pub total_cost: f64,
    pub cost_per_task: f64,
    pub elapsed: Duration,
    pub workers: Vec<WorkerStatus>,
    pub queue_ready: usize,
    pub queue_blocked: usize,
}

pub struct WorkerStatus {
    pub id: u32,
    pub state: WorkerState,  // Idle, Implementing(task), Reviewing(task), Verifying(task), Merging(task)
    pub current_task: Option<String>,
    pub cost: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
}
```

**Notes:**
- Dynamiczna wysokość: `Viewport::Inline(workers.len() + 2)` — ustawiane na starcie
- Hot-add workers nie zmienia viewport height (fixed na starcie)
- Reuse `StatusTerminal::print_line` / `insert_before` dla multiplexed output

---

### Phase 13: Session State, Resume & Lockfile
**Goal:** Persistencja stanu sesji orkiestracji + lockfile z heartbeat.

**Locations:** `src/commands/task/orchestrate/state.rs` (nowy)

**Tasks:**
- [ ] Zdefiniować `OrchestrateState` struct (serde YAML):
  ```yaml
  session_id: "uuid"
  started_at: "2026-02-10T12:34:56Z"
  workers_count: 3
  tasks:
    T01: { status: done, worker: 1, retries: 0, cost: 0.042 }
    T03: { status: in_progress, worker: 2, retries: 1, cost: 0.018 }
  dag:
    T02: [T01]
    T03: [T01, T02]
  ```
- [ ] Implementować `save(path: &Path) -> Result<()>` — atomic write (write to .tmp, rename)
- [ ] Implementować `load(path: &Path) -> Result<OrchestrateState>`
- [ ] Implementować `resume(state: &OrchestrateState) -> Scheduler` — odbudowa scheduler z saved state
- [ ] Zdefiniować `Lockfile` struct z PID + heartbeat
- [ ] Implementować `Lockfile::acquire(path: &Path) -> Result<Self>` — atomic create, sprawdź stale lock (>10s no heartbeat)
- [ ] Implementować `Lockfile::heartbeat(&self)` — update timestamp co 5s (tokio::spawn)
- [ ] Implementować `Lockfile::release(self)` — delete lockfile
- [ ] Implementować `Lockfile::is_stale(path: &Path) -> bool` — check PID alive + heartbeat freshness
- [ ] Unit testy: state serialize/deserialize, stale lock detection

**Patterns:**
```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct OrchestrateState {
    pub session_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub workers_count: u32,
    pub tasks: HashMap<String, TaskState>,
    pub dag: HashMap<String, Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskState {
    pub status: String,  // "pending", "in_progress", "done", "blocked"
    pub worker: Option<u32>,
    pub retries: u32,
    pub cost: f64,
}
```

**Notes:**
- `.ralph/orchestrate.yaml` — session state
- `.ralph/orchestrate.lock` — PID + timestamp, heartbeat 5s
- `.ralph/logs/w{N}.jsonl` — per-worker event log
- `.ralph/orchestrate.log` — combined log
- Stale lock: PID nie żyje LUB heartbeat >10s old → auto-break

---

### Phase 14: Config & CLI Integration
**Goal:** Rozszerzenie CLI o `task orchestrate` + `task clean`, sekcja `[task.orchestrate]` w `.ralph.toml`.

**Locations:** `src/cli.rs`, `src/commands/task/args.rs`, `src/shared/file_config.rs`

**Tasks:**
- [ ] Dodać `OrchestrateArgs` struct z flagami (workers, model, max-retries, verbose, resume, dry-run, worktree-prefix, no-merge, max-cost, timeout, tasks)
- [ ] Dodać `Orchestrate(OrchestrateArgs)` do `TaskCommands` enum
- [ ] Dodać `Clean` do `TaskCommands` enum
- [ ] Dodać `OrchestrateConfig` struct do `file_config.rs`:
  ```rust
  #[derive(Debug, Deserialize)]
  pub struct OrchestrateConfig {
      pub workers: Option<u32>,           // default: 2
      pub max_retries: Option<u32>,       // default: 3
      pub worktree_prefix: Option<String>,
      pub default_model: Option<String>,
  }
  ```
- [ ] Dodać `orchestrate: OrchestrateConfig` do `TaskConfig` z `#[serde(default)]`
- [ ] Merge logic: CLI flags > `.ralph.toml` > defaults
- [ ] Routing w `src/commands/task/mod.rs`: `Orchestrate(args) → orchestrate::execute(args, &file_config)`
- [ ] Routing w `src/commands/task/mod.rs`: `Clean → clean::execute(&file_config)`
- [ ] Unit testy: CLI arg parsing, config merge

**Patterns:**
```toml
# .ralph.toml
[task.orchestrate]
workers = 3
max_retries = 3
worktree_prefix = "my-project-ralph-w"
default_model = "claude-sonnet-4-5-20250929"
```

**Notes:**
- `--workers` default z config, fallback 2
- `--tasks T01,T03` parser: split by comma, trim whitespace
- Backward compat: brak `[task.orchestrate]` = all defaults

---

### Phase 15: `task clean` Command
**Goal:** Interaktywne czyszczenie orphaned zasobów orkiestracji.

**Locations:** `src/commands/task/clean.rs` (nowy)

**Tasks:**
- [ ] Implementować `execute(file_config: &FileConfig) -> Result<()>`
- [ ] Scan: orphaned worktrees (`git worktree list` + filter `ralph-w`)
- [ ] Scan: orphaned branches (`git branch --list 'ralph/w*/*'`)
- [ ] Scan: stale state files (`.ralph/orchestrate.yaml`, `.ralph/orchestrate.lock`)
- [ ] Scan: log files (`.ralph/logs/`, `.ralph/orchestrate.log`)
- [ ] Wyświetl znalezione zasoby z rozmiarami
- [ ] Interaktywne potwierdzenie: "Usunąć N worktrees, M branches, K plików? [y/N]"
- [ ] Wykonanie cleanup: `git worktree remove`, `git branch -D`, `rm` plików
- [ ] Podsumowanie: "Wyczyszczono: X worktrees, Y branches, Z plików, freed N MB"

**Patterns:**
```rust
pub async fn execute(file_config: &FileConfig) -> Result<()> {
    let findings = scan_orphaned_resources(file_config)?;
    if findings.is_empty() {
        println!("No orphaned resources found.");
        return Ok(());
    }
    display_findings(&findings);
    if confirm_cleanup()? {
        cleanup(&findings)?;
        display_summary(&findings);
    }
    Ok(())
}
```

**Notes:**
- `confirm_cleanup()` — czyta stdin, `y` lub `Y` = proceed
- Bezpieczne: nie czyści jeśli lock file ma aktywny PID (sesja w toku)
- `git worktree prune` na koniec

---

### Phase 16: `task prd` & `task add` Extensions (Deps Generation)
**Goal:** Rozszerzenie istniejących komend o generację/aktualizację YAML deps.

**Locations:** `src/commands/task/prd.rs`, `src/commands/task/add.rs`, `src/templates/prd_prompt.md`, `src/templates/add_prompt.md`

**Tasks:**
- [ ] **prd.rs**: Po wygenerowaniu PROGRESS.md przez Claude, wywołaj AI deps generation
  - Parsuj PROGRESS.md → sprawdź czy ma frontmatter deps → jeśli nie: `ai::generate_deps()`
  - Zapisz deps do PROGRESS.md frontmatter
  - Wyświetl wygenerowane deps w podsumowaniu
- [ ] **prd_prompt.md**: Zaktualizuj prompt aby Claude generował PROGRESS.md z YAML frontmatter deps od razu
  - Instrukcja: "Na górze PROGRESS.md dodaj YAML frontmatter z sekcją `deps:` definiującą zależności między taskami"
- [ ] **add.rs**: Po dodaniu tasków, sprawdź czy istnieją deps → jeśli tak: `ai::generate_deps()` dla nowych tasków
  - Merge nowych deps z istniejącymi
  - Update frontmatter
- [ ] **add_prompt.md**: Zaktualizuj prompt aby Claude wiedział o deps
- [ ] Unit testy: weryfikacja że deps są generowane/aktualizowane

**Patterns:**
```rust
// W prd.rs, po Claude finishes:
let summary = load_progress(progress_path)?;
if summary.frontmatter.as_ref().map_or(true, |fm| fm.deps.is_empty()) {
    let content = std::fs::read_to_string(progress_path)?;
    let fm = ai::generate_deps(&content, model.as_deref()).await?;
    update_progress_frontmatter(progress_path, &fm)?;
}
```

**Notes:**
- `task prd` — preferuje aby Claude generował deps od razu (prompt update)
- Fallback: jeśli Claude nie wygenerował deps → AI post-processing
- `task add` — merge logic: istniejące deps zachowane, nowe dodane

---

### Phase 17: Dry-Run & DAG Visualization
**Goal:** `--dry-run` mode z topological order DAG visualization.

**Locations:** `src/commands/task/orchestrate/dry_run.rs` (nowy)

**Tasks:**
- [ ] Implementować `execute_dry_run(progress: &ProgressSummary, dag: &TaskDag, workers: u32) -> Result<()>`
- [ ] Jeśli brak deps: wygeneruj z AI (ale NIE zapisuj do pliku)
- [ ] Topological order visualization:
  ```
  DAG Execution Plan (3 workers):

  Layer 0 (parallel):
    T01 [api] Setup JWT authentication
    T04 [test] Setup test framework

  Layer 1 (after T01):
    T02 [api] Create user endpoints

  Layer 2 (after T01, T02):
    T03 [ui] Build login form

  Layer 3 (after T03):
    T05 [ui] Dashboard page

  Summary: 5 tasks, 4 layers, estimated ~4 serial rounds with 3 workers
  ```
- [ ] Dependency arrows (text-based):
  ```
  T01 ──┬──→ T02 ──→ T03 ──→ T05
        └──→ T04
  ```
- [ ] Worker assignment preview: kto co dostanie w jakiej kolejności
- [ ] Estimacja: layers * avg_time_per_task = estimated time

**Notes:**
- Layered visualization: Group tasks by "distance from root" in DAG
- Dry-run NIE tworzy worktrees, NIE spawninguje workerów
- Dry-run MOŻE wywołać Claude (dla deps generation) — info o koszcie

---

### Phase 18: Summary & Reporting
**Goal:** Podsumowanie sesji orkiestracji — per-task tabela.

**Locations:** `src/commands/task/orchestrate/summary.rs` (nowy)

**Tasks:**
- [ ] Implementować `format_summary(state: &OrchestrateState, elapsed: Duration) -> Vec<String>`
- [ ] Per-task tabela:
  ```
  ┌────────┬───────────┬────────┬─────────┬────────┬─────────┐
  │ Task   │ Status    │ Cost   │ Time    │ Iters  │ Retries │
  ├────────┼───────────┼────────┼─────────┼────────┼─────────┤
  │ T01    │ ✓ Done    │ $0.042 │ 2m 15s  │ 3      │ 0       │
  │ T02    │ ✓ Done    │ $0.031 │ 1m 48s  │ 3      │ 0       │
  │ T03    │ ✓ Done    │ $0.055 │ 3m 02s  │ 6      │ 1       │
  │ T04    │ ! Blocked │ $0.089 │ 5m 10s  │ 9      │ 3       │
  │ T05    │ ✓ Done    │ $0.028 │ 1m 22s  │ 3      │ 0       │
  ├────────┼───────────┼────────┼─────────┼────────┼─────────┤
  │ TOTAL  │ 4/5 done  │ $0.245 │ 8m 42s  │ 24     │ 4       │
  └────────┴───────────┴────────┴─────────┴────────┴─────────┘

  Workers: 3 │ Avg/task: $0.049 │ Parallelism: 1.6x speedup
  ```
- [ ] Kolorowanie: Done=green, Blocked=red, per-worker breakdown
- [ ] Zapis do `.ralph/orchestrate-summary.txt` (opcjonalnie)

**Notes:**
- Tabela rysowana ratatui Paragraphs z hardcoded box chars
- Iters = łączna liczba faz (3 per successful attempt + 3*retries)
- Parallelism metric: (sum of task times) / (wall clock time)

---

### Phase 19: Error Handling Extensions
**Goal:** Nowe warianty błędów dla orkiestracji.

**Locations:** `src/shared/error.rs`

**Tasks:**
- [ ] Dodać warianty:
  - `Orchestrate(String)` — ogólne błędy orkiestracji
  - `WorktreeError(String)` — git worktree operations
  - `MergeConflict(String)` — unresolvable merge conflict
  - `DagCycle(Vec<String>)` — cycle w DAG
  - `LockfileHeld(String)` — lockfile z aktywnym PID
  - `SessionResume(String)` — błędy resume
- [ ] Obsługa nowych exit codes:
  - `2` — partial completion (some tasks blocked)
  - `3` — lockfile held
  - `0` — full success

**Notes:**
- Error propagation: worker errors → WorkerEvent::TaskFailed → orkiestrator loguje i decyduje
- Orkiestrator nie crashuje na single worker failure

---

### Phase 20: Templates — Orchestration Prompts
**Goal:** Embedded prompts dla AI-assisted orkiestracji.

**Locations:** `src/templates/` (nowe pliki)

**Tasks:**
- [ ] Utworzyć `src/templates/deps_generation_prompt.md`:
  - Instrukcja: "Analyze tasks and return JSON with dependency map"
  - Format output: `{"deps": {"T02": ["T01"], ...}}`
  - Uwagi: respect Epic ordering, component isolation hints
- [ ] Utworzyć `src/templates/conflict_resolution_prompt.md`:
  - Instrukcja: "Resolve merge conflict between these changes"
  - Format: diff our vs theirs + context
- [ ] Utworzyć `src/templates/worker_implement_prompt.md`:
  - Task description injection point
  - Minimal awareness: "You're working in an isolated worktree"
  - Instrukcja: "Focus only on this task"
- [ ] Utworzyć `src/templates/worker_review_prompt.md`:
  - Review + fix instructions
  - Input: implementation output from phase 1
- [ ] Utworzyć `src/templates/worker_verify_prompt.md`:
  - Verify instructions: run verification commands from SYSTEM_PROMPT
  - Input: SYSTEM_PROMPT.md commands section
- [ ] Dodać `include_str!` stałe do `src/templates/mod.rs`
- [ ] Zaktualizować `src/templates/prd_prompt.md` — dodać instrukcje o YAML deps frontmatter

**Notes:**
- Worker prompts: jasne instrukcje izolacji — "nie modyfikuj plików spoza swojego taska"
- Deps prompt: wymaga JSON output — Claude musi zwrócić parsable JSON
- Conflict prompt: podaje oba diffy + kontekst taska

---

### Phase 21: PROGRESS.md Two-Phase Update
**Goal:** Orkiestrator aktualizuje PROGRESS.md: `[ ]→[~]` na start, `[~]→[x]` po merge.

**Locations:** `src/shared/progress.rs` (rozszerzenie)

**Tasks:**
- [ ] Implementować `update_task_status(path: &Path, task_id: &str, new_status: TaskStatus) -> Result<()>`
  - Czytaj plik, znajdź linię z task_id, zamień status marker, zapisz
- [ ] Implementować `batch_update_statuses(path: &Path, updates: &[(String, TaskStatus)]) -> Result<()>`
  - Atomowy update wielu tasków na raz (na resume)
- [ ] Status marker mapping: `Todo → "[ ]"`, `InProgress → "[~]"`, `Done → "[x]"`, `Blocked → "[!]"`
- [ ] Tolerancyjny matching: znajdź `- [S] {task_id}` niezależnie od formatowania
- [ ] Unit testy: update single task, batch update, missing task_id handling

**Patterns:**
```rust
pub fn update_task_status(path: &Path, task_id: &str, new_status: TaskStatus) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let marker = match new_status {
        TaskStatus::Todo => "[ ]",
        TaskStatus::Done => "[x]",
        TaskStatus::InProgress => "[~]",
        TaskStatus::Blocked => "[!]",
    };
    // Find line with task_id, replace status marker
    let updated = content.lines().map(|line| {
        if let Some(task) = parse_task_line(line) && task.id == task_id {
            line.replacen(&format!("[{}]", old_marker(line)), marker, 1)
        } else {
            line.to_string()
        }
    }).collect::<Vec<_>>().join("\n");
    std::fs::write(path, updated)?;
    Ok(())
}
```

**Notes:**
- Atomowy: write to .tmp → rename (prevent partial writes)
- Orkiestrator jest jedynym pisarzem PROGRESS.md — brak race conditions
- Workerzy NIE modyfikują PROGRESS.md

---

### Phase 22: Integration & End-to-End Testing
**Goal:** Testy integracyjne pełnego flow orkiestracji.

**Locations:** `src/commands/task/orchestrate/` (testy inline), ewentualnie `tests/`

**Tasks:**
- [ ] **Unit testy** (inline w każdym module):
  - DAG: topological sort, cycle detection, ready tasks
  - Scheduler: FIFO order, dependency respect, retry logic
  - Worktree: path generation, branch naming
  - Events: serialization, JSONL format
  - State: save/load, resume reconstruction
  - Progress update: status change, batch update
  - Config: merge CLI + TOML, defaults
- [ ] **Integration testy**:
  - `cargo build` — kompilacja bez błędów
  - `cargo test` — wszystkie testy przechodzą
  - `cargo clippy --all-targets -- -D warnings` — zero warnings
  - Backward compat: `ralph-wiggum --prompt "test"` działa bez zmian
  - `ralph-wiggum task orchestrate --help` — poprawny help
  - `ralph-wiggum task orchestrate --dry-run` — DAG visualization
  - `ralph-wiggum task clean` — interaktywne czyszczenie
- [ ] **Manual verification**:
  - Orchestrate z 2 workerami na prostym PROGRESS.md (3 niezależne taski)
  - Verify worktree creation, merge, cleanup
  - Verify Ctrl+C graceful shutdown
  - Verify resume po przerwaniu

---

## Final Verification

1. **Build**: `cargo build` — kompilacja bez błędów
2. **Tests**: `cargo test` — wszystkie testy przechodzą
3. **Lint**: `cargo clippy --all-targets -- -D warnings` — zero ostrzeżeń
4. **Backward compat**:
   - `ralph-wiggum --prompt "test"` → działa jak dotychczas
   - `ralph-wiggum run --prompt "test"` → działa jak dotychczas
   - `ralph-wiggum task continue` → działa jak dotychczas
   - `ralph-wiggum task status` → działa jak dotychczas
   - Istniejące `.ralph.toml` bez `[task.orchestrate]` → działa bez zmian
5. **Nowe komendy**:
   - `ralph-wiggum task orchestrate --workers 3` → uruchamia orkiestrację
   - `ralph-wiggum task orchestrate --dry-run` → generuje DAG + visualizacja
   - `ralph-wiggum task orchestrate --resume` → wznawia sesję
   - `ralph-wiggum task orchestrate --tasks T01,T03` → filtrowane taski
   - `ralph-wiggum task clean` → interaktywne czyszczenie
6. **Orchestration flow**:
   - Worktrees tworzone jako sibling dirs
   - Workers uruchamiają 3 fazy per task
   - Squash merge po każdym tasku
   - DAG dependencies respected
   - Conflict resolution przez Claude
   - Two-phase progress update ([ ]→[~]→[x])
   - Hot reload PROGRESS.md
7. **UI**:
   - Multiplexed output z `[W1|T03]` prefix
   - Dynamic N+2 status bar
   - Per-task summary table na koniec
8. **Resilience**:
   - Ctrl+C graceful → force shutdown (two-phase)
   - Worker crash → auto-retry (max 3)
   - Merge conflict → Claude resolves
   - Resume po przerwaniu
   - Orphan cleanup via `task clean`
9. **AI-assisted**:
   - Deps generation z Claude (jeśli brak w PROGRESS.md)
   - Conflict resolution z Claude (diff + context)
   - Worker self-review (phase 2)
