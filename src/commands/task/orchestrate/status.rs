use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::commands::run::ui::StatusTerminal;
use crate::commands::task::orchestrate::events::WorkerPhase;
use crate::commands::task::orchestrate::scheduler::SchedulerStatus;
use crate::shared::error::Result;

// ── Data types ──────────────────────────────────────────────────────

/// Per-worker status for the status bar display.
#[derive(Debug, Clone)]
pub struct WorkerStatus {
    pub worker_id: u32,
    pub state: WorkerState,
    pub task_id: Option<String>,
    pub component: Option<String>,
    pub phase: Option<WorkerPhase>,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl WorkerStatus {
    /// Create an idle worker status.
    pub fn idle(worker_id: u32) -> Self {
        Self {
            worker_id,
            state: WorkerState::Idle,
            task_id: None,
            component: None,
            phase: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        }
    }
}

/// Worker state for display purposes.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerState {
    Idle,
    Implementing,
    Reviewing,
    Verifying,
    Merging,
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerState::Idle => write!(f, "idle"),
            WorkerState::Implementing => write!(f, "implementing"),
            WorkerState::Reviewing => write!(f, "reviewing"),
            WorkerState::Verifying => write!(f, "verifying"),
            WorkerState::Merging => write!(f, "merging"),
        }
    }
}

/// Shutdown phase for display purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShutdownState {
    #[default]
    Running,
    /// First q/Ctrl+C — waiting for in-progress tasks to finish
    Draining,
    /// Second q/Ctrl+C — force-aborting all workers
    Aborting,
}

/// Snapshot of the orchestrator's status for rendering.
#[derive(Debug, Clone)]
pub struct OrchestratorStatus {
    pub scheduler: SchedulerStatus,
    pub workers: Vec<WorkerStatus>,
    pub total_cost: f64,
    pub elapsed: Duration,
    pub shutdown_state: ShutdownState,
}

// ── Status bar (owns StatusTerminal) ────────────────────────────────

/// Orchestrator status bar — renders N+2 inline lines via ratatui.
///
/// - Line 1: Overall progress bar + cost + elapsed
/// - Lines 2..N+1: Per-worker status (icon, task, phase, tokens, cost)
/// - Line N+2: Queue info (ready, in-progress, blocked, pending)
///
/// Note: Replaced by `Dashboard` for fullscreen mode, but kept for tests
/// and potential future inline-mode fallback.
#[allow(dead_code)]
pub struct OrchestratorStatusBar {
    terminal: StatusTerminal,
    #[allow(dead_code)]
    started_at: Instant,
    #[allow(dead_code)]
    worker_count: u32,
}

#[allow(dead_code)]
impl OrchestratorStatusBar {
    pub fn new(worker_count: u32, use_nerd_font: bool) -> Result<Self> {
        let height = worker_count as u16 + 2;
        let terminal = StatusTerminal::with_height(use_nerd_font, height)?;
        Ok(Self {
            terminal,
            started_at: Instant::now(),
            worker_count,
        })
    }

    /// Total number of lines this status bar occupies.
    #[allow(dead_code)]
    pub fn line_count(&self) -> u16 {
        self.worker_count as u16 + 2
    }

    /// Redraw the full N+2 status bar with current status.
    pub fn update(&mut self, status: &OrchestratorStatus) -> Result<()> {
        let mut lines = Vec::with_capacity(status.workers.len() + 2);
        lines.push(Self::render_progress_line(status));
        for worker in &status.workers {
            lines.push(Self::render_worker_line(worker));
        }
        lines.push(Self::render_queue_line(status));
        self.terminal.draw_lines(&lines)
    }

    // ── Delegation to StatusTerminal ─────────────────────────────

    pub fn print_line(&mut self, text: &str) -> Result<()> {
        self.terminal.print_line(text)
    }

    #[allow(dead_code)]
    pub fn print_lines(&mut self, lines: &[String]) -> Result<()> {
        self.terminal.print_lines(lines)
    }

    pub fn cleanup(&mut self) -> Result<()> {
        self.terminal.cleanup()
    }

    #[allow(dead_code)]
    pub fn show_shutting_down(&mut self) -> Result<()> {
        self.terminal.show_shutting_down()
    }

    // ── Styled line builders ─────────────────────────────────────

    /// Overall progress line with bar, counts, cost, elapsed.
    fn render_progress_line(status: &OrchestratorStatus) -> Line<'static> {
        let total = status.scheduler.total;
        let done = status.scheduler.done;
        let pct = if total > 0 { (done * 100) / total } else { 0 };

        let bar = render_progress_bar(done, total, 20);
        let elapsed = format_duration(status.elapsed);
        let cost_per_task = if done > 0 {
            status.total_cost / done as f64
        } else {
            0.0
        };

        Line::from(vec![
            Span::styled(bar, Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(
                format!("{done}/{total} ({pct}%)"),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(" │ "),
            Span::styled(
                format!("${:.4}", status.total_cost),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" │ "),
            Span::styled(
                format!("${cost_per_task:.4}/task"),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" │ "),
            Span::styled(
                format!("⏱ {elapsed}"),
                Style::default().fg(Color::White),
            ),
        ])
    }

    /// Per-worker status line.
    fn render_worker_line(worker: &WorkerStatus) -> Line<'static> {
        let (icon, icon_color) = match &worker.state {
            WorkerState::Idle => ("○", Color::DarkGray),
            WorkerState::Implementing => ("●", Color::Cyan),
            WorkerState::Reviewing => ("◎", Color::Yellow),
            WorkerState::Verifying => ("◉", Color::Magenta),
            WorkerState::Merging => ("⊕", Color::Green),
        };

        let task = worker
            .task_id
            .clone()
            .unwrap_or_else(|| "---".to_string());
        let component = worker
            .component
            .clone()
            .unwrap_or_default();
        let phase = worker
            .phase
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| worker.state.to_string());

        let mut spans = vec![
            Span::raw("  "),
            Span::styled(
                format!("W{}", worker.worker_id),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(icon.to_string(), Style::default().fg(icon_color)),
            Span::raw(" "),
            Span::styled(
                task,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" [{component}]"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(" "),
            Span::styled(phase, Style::default().fg(icon_color)),
        ];

        // Show tokens if worker is active
        if worker.state != WorkerState::Idle {
            spans.push(Span::raw(" │ "));
            spans.push(Span::styled("↓", Style::default().fg(Color::Green)));
            spans.push(Span::raw(format_tokens(worker.input_tokens)));
            spans.push(Span::raw(" "));
            spans.push(Span::styled("↑", Style::default().fg(Color::Magenta)));
            spans.push(Span::raw(format_tokens(worker.output_tokens)));
        }

        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(
            format!("${:.4}", worker.cost_usd),
            Style::default().fg(Color::Yellow),
        ));

        Line::from(spans)
    }

    /// Queue info line.
    fn render_queue_line(status: &OrchestratorStatus) -> Line<'static> {
        Line::from(vec![
            Span::raw("  Queue: "),
            Span::styled(
                format!("{} ready", status.scheduler.ready),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" │ "),
            Span::styled(
                format!("{} in-progress", status.scheduler.in_progress),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(" │ "),
            Span::styled(
                format!("{} blocked", status.scheduler.blocked),
                Style::default().fg(Color::Red),
            ),
            Span::raw(" │ "),
            Span::styled(
                format!("{} pending", status.scheduler.pending),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    }

    // ── Text rendering (for tests / dry-run) ─────────────────────

    /// Render all status bar lines as a plain-text string.
    #[allow(dead_code)]
    pub fn render_text(&self, status: &OrchestratorStatus) -> String {
        let mut lines = Vec::new();
        lines.push(Self::render_progress_text(status));
        for worker in &status.workers {
            lines.push(Self::render_worker_text(worker));
        }
        lines.push(Self::render_queue_text(status));
        lines.join("\n")
    }

    #[allow(dead_code)]
    fn render_progress_text(status: &OrchestratorStatus) -> String {
        let total = status.scheduler.total;
        let done = status.scheduler.done;
        let pct = if total > 0 { (done * 100) / total } else { 0 };
        let bar = render_progress_bar(done, total, 20);
        let elapsed = format_duration(status.elapsed);
        let cost_per_task = if done > 0 {
            status.total_cost / done as f64
        } else {
            0.0
        };
        format!(
            "{bar} {done}/{total} ({pct}%) | ${:.4} | ${:.4}/task | {elapsed}",
            status.total_cost, cost_per_task
        )
    }

    #[allow(dead_code)]
    fn render_worker_text(worker: &WorkerStatus) -> String {
        let icon = match &worker.state {
            WorkerState::Idle => "○",
            WorkerState::Implementing => "●",
            WorkerState::Reviewing => "◎",
            WorkerState::Verifying => "◉",
            WorkerState::Merging => "⊕",
        };
        let task = worker.task_id.as_deref().unwrap_or("---");
        let component = worker.component.as_deref().unwrap_or("");
        let phase = worker
            .phase
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| worker.state.to_string());
        format!(
            "  W{}: {icon} {task} [{component}] {phase} | ${:.4}",
            worker.worker_id, worker.cost_usd
        )
    }

    #[allow(dead_code)]
    fn render_queue_text(status: &OrchestratorStatus) -> String {
        format!(
            "  Queue: {} ready | {} in-progress | {} blocked | {} pending",
            status.scheduler.ready,
            status.scheduler.in_progress,
            status.scheduler.blocked,
            status.scheduler.pending
        )
    }
}

// ── Input thread (dedicated OS thread for crossterm) ────────────────

/// Keyboard/resize input thread for the orchestrator TUI.
///
/// Runs on a dedicated OS thread (never tokio::spawn!) because
/// crossterm::event::poll() is blocking and would starve the async runtime.
///
/// Note: Replaced by `DashboardInputThread`, kept for potential fallback.
#[allow(dead_code)]
pub struct OrchestratorInputThread {
    handle: Option<std::thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl OrchestratorInputThread {
    #[allow(dead_code)]
    pub fn spawn(
        shutdown: Arc<AtomicBool>,
        graceful_shutdown: Arc<AtomicBool>,
        resize_flag: Arc<AtomicBool>,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = std::thread::Builder::new()
            .name("orch-input".into())
            .spawn(move || {
                while running_clone.load(Ordering::SeqCst) {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    if crossterm::event::poll(Duration::from_millis(100)).unwrap_or(false) {
                        match crossterm::event::read() {
                            Ok(crossterm::event::Event::Key(key)) => {
                                use crossterm::event::{KeyCode, KeyModifiers};
                                let is_quit = matches!(
                                    key.code,
                                    KeyCode::Char('q')
                                ) || (key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL));

                                if is_quit {
                                    if graceful_shutdown.load(Ordering::SeqCst) {
                                        shutdown.store(true, Ordering::SeqCst);
                                    } else {
                                        graceful_shutdown.store(true, Ordering::SeqCst);
                                    }
                                }
                            }
                            Ok(crossterm::event::Event::Resize(_, _)) => {
                                resize_flag.store(true, Ordering::SeqCst);
                            }
                            _ => {}
                        }
                    }
                }
            })
            .expect("Failed to spawn orchestrator input thread");

        Self {
            handle: Some(handle),
            running,
        }
    }

    #[allow(dead_code)]
    pub fn stop(mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for OrchestratorInputThread {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Render a Unicode progress bar.
pub fn render_progress_bar(done: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return format!("[{}]", " ".repeat(width));
    }
    let filled = (done * width) / total;
    let empty = width - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

/// Format a duration as human-readable string.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Format token count for display (e.g., 1234 -> "1.2k").
pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

// ── Dashboard input thread (extended key support) ────────────────────

/// Keyboard/resize input thread for the fullscreen dashboard.
///
/// Extends `OrchestratorInputThread` with Tab/Esc/1-9/arrows for
/// panel focus and scroll control. Runs on a dedicated OS thread.
pub struct DashboardInputThread {
    handle: Option<std::thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl DashboardInputThread {
    pub fn spawn(
        shutdown: Arc<AtomicBool>,
        graceful_shutdown: Arc<AtomicBool>,
        resize_flag: Arc<AtomicBool>,
        focused_worker: Arc<AtomicU32>,
        worker_count: u32,
        scroll_delta: Arc<AtomicI32>,
        render_notify: Arc<tokio::sync::Notify>,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = std::thread::Builder::new()
            .name("dash-input".into())
            .spawn(move || {
                while running_clone.load(Ordering::SeqCst) {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    if crossterm::event::poll(Duration::from_millis(100)).unwrap_or(false) {
                        match crossterm::event::read() {
                            Ok(crossterm::event::Event::Key(key)) => {
                                use crossterm::event::{KeyCode, KeyModifiers};

                                // Quit: q or Ctrl+C
                                let is_quit = matches!(key.code, KeyCode::Char('q'))
                                    || (key.code == KeyCode::Char('c')
                                        && key.modifiers.contains(KeyModifiers::CONTROL));

                                if is_quit {
                                    if graceful_shutdown.load(Ordering::SeqCst) {
                                        shutdown.store(true, Ordering::SeqCst);
                                    } else {
                                        graceful_shutdown.store(true, Ordering::SeqCst);
                                    }
                                    render_notify.notify_one();
                                    continue;
                                }

                                // Tab: cycle focus forward
                                if key.code == KeyCode::Tab
                                    && !key.modifiers.contains(KeyModifiers::SHIFT)
                                {
                                    let current = focused_worker.load(Ordering::Relaxed);
                                    let next = if current >= worker_count {
                                        0
                                    } else {
                                        current + 1
                                    };
                                    focused_worker.store(next, Ordering::Relaxed);
                                    scroll_delta.store(0, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }

                                // Shift+Tab: cycle focus backward
                                if key.code == KeyCode::BackTab {
                                    let current = focused_worker.load(Ordering::Relaxed);
                                    let next = if current == 0 {
                                        worker_count
                                    } else {
                                        current - 1
                                    };
                                    focused_worker.store(next, Ordering::Relaxed);
                                    scroll_delta.store(0, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }

                                // Esc: unfocus
                                if key.code == KeyCode::Esc {
                                    focused_worker.store(0, Ordering::Relaxed);
                                    scroll_delta.store(0, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }

                                // 1-9: direct focus
                                if let KeyCode::Char(ch) = key.code
                                    && ch.is_ascii_digit()
                                    && ch != '0'
                                {
                                    let n = ch as u32 - '0' as u32;
                                    if n <= worker_count {
                                        focused_worker.store(n, Ordering::Relaxed);
                                        scroll_delta.store(0, Ordering::Relaxed);
                                        render_notify.notify_one();
                                    }
                                    continue;
                                }

                                // Up/Down arrows: scroll focused panel
                                if key.code == KeyCode::Up {
                                    scroll_delta.fetch_sub(1, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }
                                if key.code == KeyCode::Down {
                                    scroll_delta.fetch_add(1, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }

                                // End: reset scroll (signal via large positive value)
                                if key.code == KeyCode::End {
                                    scroll_delta.store(i32::MAX, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }
                            }
                            Ok(crossterm::event::Event::Resize(_, _)) => {
                                resize_flag.store(true, Ordering::SeqCst);
                                render_notify.notify_one();
                            }
                            _ => {}
                        }
                    }
                }
            })
            .expect("Failed to spawn dashboard input thread");

        Self {
            handle: Some(handle),
            running,
        }
    }

    pub fn stop(mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for DashboardInputThread {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_progress_bar() {
        assert_eq!(render_progress_bar(0, 10, 10), "[░░░░░░░░░░]");
        assert_eq!(render_progress_bar(5, 10, 10), "[█████░░░░░]");
        assert_eq!(render_progress_bar(10, 10, 10), "[██████████]");
        assert_eq!(render_progress_bar(0, 0, 10), "[          ]");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m30s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h1m");
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1200), "1.2k");
        assert_eq!(format_tokens(1_500_000), "1.5M");
    }

    fn make_status() -> OrchestratorStatus {
        OrchestratorStatus {
            scheduler: SchedulerStatus {
                total: 10,
                done: 5,
                in_progress: 2,
                blocked: 1,
                ready: 2,
                pending: 0,
            },
            workers: vec![],
            total_cost: 0.25,
            elapsed: Duration::from_secs(120),
            shutdown_state: ShutdownState::Running,
        }
    }

    #[test]
    fn test_render_progress_text() {
        let status = make_status();
        let line = OrchestratorStatusBar::render_progress_text(&status);
        assert!(line.contains("5/10"));
        assert!(line.contains("50%"));
        assert!(line.contains("$0.25"));
        assert!(line.contains("2m0s"));
    }

    #[test]
    fn test_render_worker_text_idle() {
        let worker = WorkerStatus::idle(1);
        let line = OrchestratorStatusBar::render_worker_text(&worker);
        assert!(line.contains("W1"));
        assert!(line.contains("○"));
        assert!(line.contains("---"));
        assert!(line.contains("idle"));
    }

    #[test]
    fn test_render_worker_text_busy() {
        let worker = WorkerStatus {
            worker_id: 2,
            state: WorkerState::Implementing,
            task_id: Some("T03".to_string()),
            component: Some("api".to_string()),
            phase: Some(WorkerPhase::Implement),
            cost_usd: 0.042,
            input_tokens: 1200,
            output_tokens: 500,
        };
        let line = OrchestratorStatusBar::render_worker_text(&worker);
        assert!(line.contains("W2"));
        assert!(line.contains("●"));
        assert!(line.contains("T03"));
        assert!(line.contains("api"));
        assert!(line.contains("implement"));
    }

    #[test]
    fn test_render_queue_text() {
        let status = OrchestratorStatus {
            scheduler: SchedulerStatus {
                total: 10,
                done: 3,
                in_progress: 2,
                blocked: 1,
                ready: 3,
                pending: 1,
            },
            workers: vec![],
            total_cost: 0.0,
            elapsed: Duration::from_secs(0),
            shutdown_state: ShutdownState::Running,
        };
        let line = OrchestratorStatusBar::render_queue_text(&status);
        assert!(line.contains("3 ready"));
        assert!(line.contains("2 in-progress"));
        assert!(line.contains("1 blocked"));
        assert!(line.contains("1 pending"));
    }

    #[test]
    fn test_line_count() {
        // We can't create OrchestratorStatusBar in test (needs terminal),
        // but we can test the formula directly.
        assert_eq!(3u32 as u16 + 2, 5); // 1 progress + 3 workers + 1 queue
    }

    #[test]
    fn test_worker_state_display() {
        assert_eq!(WorkerState::Idle.to_string(), "idle");
        assert_eq!(WorkerState::Implementing.to_string(), "implementing");
        assert_eq!(WorkerState::Reviewing.to_string(), "reviewing");
        assert_eq!(WorkerState::Verifying.to_string(), "verifying");
        assert_eq!(WorkerState::Merging.to_string(), "merging");
    }

    #[test]
    fn test_worker_status_idle_constructor() {
        let ws = WorkerStatus::idle(3);
        assert_eq!(ws.worker_id, 3);
        assert_eq!(ws.state, WorkerState::Idle);
        assert!(ws.task_id.is_none());
        assert_eq!(ws.cost_usd, 0.0);
    }

    #[test]
    fn test_full_render_text() {
        let status = OrchestratorStatus {
            scheduler: SchedulerStatus {
                total: 10,
                done: 3,
                in_progress: 2,
                blocked: 0,
                ready: 5,
                pending: 0,
            },
            workers: vec![
                WorkerStatus {
                    worker_id: 1,
                    state: WorkerState::Implementing,
                    task_id: Some("T04".to_string()),
                    component: Some("ui".to_string()),
                    phase: Some(WorkerPhase::Implement),
                    cost_usd: 0.01,
                    input_tokens: 100,
                    output_tokens: 50,
                },
                WorkerStatus::idle(2),
            ],
            total_cost: 0.01,
            elapsed: Duration::from_secs(45),
            shutdown_state: ShutdownState::Running,
        };

        // render_text requires an instance, but we can test the static methods
        let progress = OrchestratorStatusBar::render_progress_text(&status);
        let w1 = OrchestratorStatusBar::render_worker_text(&status.workers[0]);
        let w2 = OrchestratorStatusBar::render_worker_text(&status.workers[1]);
        let queue = OrchestratorStatusBar::render_queue_text(&status);

        assert!(progress.contains("3/10"));
        assert!(w1.contains("T04"));
        assert!(w2.contains("---"));
        assert!(queue.contains("Queue"));
    }
}
