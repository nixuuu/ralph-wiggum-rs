//! OrchestrateApp — state implementujący AppState dla orchestrator dashboard.
//!
//! Migracja logiki z `dashboard.rs` do nowego TUI framework (`App`, `EventDispatcher`, `AppState`).
//! Zachowuje: worker panels, scheduler status, shutdown flow, focus navigation,
//! overlays (task preview, text input), scroll, restart confirmation.
//!
//! Key handling: `app_keys.rs`
//! Render functions: `app_render.rs`

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ansi_to_tui::IntoText;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;

use crate::commands::task::orchestrate::app_render::{
    WorkerGridConfig, compute_active_worker_rects, render_global_bar, render_overlay,
    render_worker_grid,
};
use crate::commands::task::orchestrate::scheduler::SchedulerStatus;
use crate::commands::task::orchestrate::shutdown_types::{OrchestratorStatus, ShutdownState};
use crate::commands::task::orchestrate::worker_status::{WorkerState, WorkerStatus};
use crate::shared::tasks::TasksFile;
use crate::tui::app::AppState;
use crate::tui::events::{AppEvent, EventResult};
use crate::tui::keybindings::{KeyAction, KeybindingResolver};
use crate::tui::ring_buffer::OutputRingBuffer;
use crate::tui::widgets::{
    CommandPaletteState, CommandPaletteWidget, TaskSidebar, TaskSidebarState, TextInputOverlay,
};

// ── WorkerPanel ──────────────────────────────────────────────────────

/// State of a single worker panel in the dashboard.
pub struct WorkerPanel {
    pub worker_id: u32,
    pub status: WorkerStatus,
    pub output: OutputRingBuffer,
    pub scroll_offset: usize, // 0 = auto-scroll (follow tail)
    /// Timestamp when the worker became idle (used for grace period visibility).
    pub idle_since: Option<Instant>,
}

impl WorkerPanel {
    pub fn new(worker_id: u32) -> Self {
        Self {
            worker_id,
            status: WorkerStatus::idle(worker_id),
            output: OutputRingBuffer::with_capacity(200),
            scroll_offset: 0,
            idle_since: None,
        }
    }
}

// ── Quit confirmation flow ──────────────────────────────────────────

/// State maszyny quit confirmation (replaces Arc<AtomicU8>).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuitState {
    #[default]
    Normal,
    /// First 'q' pressed — waiting for confirmation
    Pending,
}

// ── Restart confirmation flow ───────────────────────────────────────

/// State restartu workera (replaces Arc<AtomicU32> + Arc<AtomicBool>).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RestartState {
    #[default]
    None,
    /// Restart pending — czeka na y/n
    Pending { worker_id: u32 },
    /// Restart confirmed — orchestrator powinien go wykonać
    Confirmed { worker_id: u32 },
}

// ── OrchestrateApp ──────────────────────────────────────────────────

/// Główny state orchestrator dashboard, implementujący `AppState`.
///
/// Przenosi odpowiedzialność z `Dashboard` struct:
/// - Worker panels z output ring buffers
/// - Focus navigation (Tab/Shift+Tab/1-9)
/// - Quit confirmation flow (q → q/Enter → graceful shutdown)
/// - Restart confirmation flow (R → y/n)
/// - Task preview overlay (p)
/// - Text input overlay (i)
/// - Scroll management (Up/Down/Left/Right)
/// - Scheduler/orchestrator status snapshot
#[allow(dead_code)] // Public API — zostanie podłączony w task 6.9
pub struct OrchestrateApp {
    // ── Worker panels ──
    pub(crate) panels: HashMap<u32, WorkerPanel>,
    pub(crate) worker_count: u32,

    // ── Focus ──
    pub(crate) focused_worker: Option<u32>,
    /// Sorted list of currently active (non-idle) worker IDs.
    /// Updated each tick for focus navigation.
    pub(crate) active_worker_ids: Vec<u32>,

    // ── Overlays ──
    pub(crate) show_task_preview: bool,
    pub(crate) preview_scroll_offset: usize,
    /// Shared overlay instance for text input (synchronized with external systems).
    pub(crate) shared_overlay: Arc<Mutex<Option<TextInputOverlay>>>,

    // ── Task sidebar (compact mode) ──
    pub(crate) sidebar_state: TaskSidebarState,
    pub(crate) sidebar_focused: bool,

    // ── Status ──
    orchestrator_status: OrchestratorStatus,
    tasks_file: Option<TasksFile>,

    // ── Shutdown/quit/restart flow ──
    pub(crate) quit_state: QuitState,
    pub(crate) restart_state: RestartState,
    /// Graceful shutdown requested (first Ctrl+C / confirmed quit)
    pub(crate) graceful_shutdown: bool,

    // ── Reload ──
    /// Flaga reloadu tasks.yml (klawisz 'r').
    pub(crate) reload_requested: bool,

    // ── Idle visibility ──
    /// When true, idle workers are shown even after grace period expires.
    pub(crate) show_idle: bool,

    // ── User messages from overlay ──
    /// Bufor wiadomości użytkownika z overlay (worker_id, message).
    /// Orchestrator loop konsumuje je przez `take_pending_messages()`.
    pub(crate) pending_user_messages: VecDeque<(u32, String)>,

    // ── Log lines ──
    log_lines: VecDeque<Line<'static>>,

    // ── Command palette ──
    /// Stan command palette. `Some` gdy palette jest otwarta (Ctrl+P).
    pub(crate) command_palette: Option<CommandPaletteState>,

    // ── Keybinding resolver ──
    /// Resolver keybindingów — umożliwia konfigurowalny klawisz Ctrl+P.
    /// Domyślnie inicjalizowany z defaults; użyj `with_resolver()` dla custom bindingów.
    pub(crate) resolver: KeybindingResolver,

    // ── Cached hit-test rects (updated each draw()) ──
    /// Mapowanie worker_id → Rect, aktualizowane w każdym draw().
    /// Używane do hit-testingu myszki w handle_event().
    pub(crate) grid_rects: Vec<(u32, Rect)>,
    /// Rect sidebara (gdy widoczny), aktualizowany w każdym draw().
    pub(crate) sidebar_rect: Option<Rect>,
    /// Rect overlaya (gdy aktywny), aktualizowany w każdym draw().
    pub(crate) overlay_rect: Option<Rect>,

    // ── Scroll ──
    /// Liczba linii przewijana przy zdarzeniu scroll myszy (z TuiConfig)
    pub(crate) scroll_step: usize,
}

#[allow(dead_code)] // Public API — zostanie podłączony w task 6.9
impl OrchestrateApp {
    /// Create a new OrchestrateApp with given worker count.
    pub fn new(worker_count: u32, shared_overlay: Arc<Mutex<Option<TextInputOverlay>>>) -> Self {
        let mut panels = HashMap::new();
        for i in 1..=worker_count {
            panels.insert(i, WorkerPanel::new(i));
        }

        let mut sidebar_state = TaskSidebarState::new();
        sidebar_state.visible = false; // Sidebar hidden by default — user toggles with 't'

        Self {
            panels,
            worker_count,
            focused_worker: None,
            active_worker_ids: (1..=worker_count).collect(),
            show_task_preview: false,
            preview_scroll_offset: 0,
            shared_overlay,
            sidebar_state,
            sidebar_focused: false,
            orchestrator_status: default_orchestrator_status(),
            tasks_file: None,
            quit_state: QuitState::Normal,
            restart_state: RestartState::None,
            graceful_shutdown: false,
            reload_requested: false,
            show_idle: false,
            pending_user_messages: VecDeque::new(),
            log_lines: VecDeque::with_capacity(50),
            command_palette: None,
            resolver: KeybindingResolver::with_defaults(),
            grid_rects: Vec::new(),
            sidebar_rect: None,
            overlay_rect: None,
            scroll_step: 3,
        }
    }

    /// Builder: ustaw custom `KeybindingResolver` (np. z `.ralph.toml` user config).
    ///
    /// Pozwala na konfigurowalny klawisz command palette (domyślnie Ctrl+P).
    pub fn with_resolver(mut self, resolver: KeybindingResolver) -> Self {
        self.resolver = resolver;
        self
    }

    /// Ustaw konfigurowalny scroll step (z TuiConfig). Builder pattern.
    pub fn with_scroll_step(mut self, step: u16) -> Self {
        self.scroll_step = step.max(1) as usize;
        self
    }

    // ── Public API (called by orchestrator loop) ────────────────────

    /// Update orchestrator status snapshot for rendering.
    pub fn update_status(&mut self, status: OrchestratorStatus) {
        self.orchestrator_status = status;
    }

    /// Update tasks file for preview overlay and sidebar.
    pub fn update_tasks_file(&mut self, tasks_file: Option<TasksFile>) {
        self.tasks_file = tasks_file;
        // Refresh sidebar with new task tree
        if let Some(ref tf) = self.tasks_file {
            self.sidebar_state.refresh(tf);
        }
    }

    pub fn update_worker_status(&mut self, worker_id: u32, status: WorkerStatus) {
        if let Some(panel) = self.panels.get_mut(&worker_id) {
            let previous_state = &panel.status.state;
            let new_state = &status.state;

            // Track transition to/from Idle for grace period
            if *previous_state != WorkerState::Idle && *new_state == WorkerState::Idle {
                panel.idle_since = Some(Instant::now());
            } else if *new_state != WorkerState::Idle {
                panel.idle_since = None;
            }

            panel.status = status;
        }
        self.refresh_active_worker_ids();
    }

    /// Update only cost/token fields for a worker.
    pub fn update_worker_cost(
        &mut self,
        worker_id: u32,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        if let Some(panel) = self.panels.get_mut(&worker_id) {
            panel.status.cost_usd = cost_usd;
            panel.status.input_tokens = input_tokens;
            panel.status.output_tokens = output_tokens;
        }
    }

    /// Update verify profile statuses for a worker.
    pub fn update_verify_profiles(
        &mut self,
        worker_id: u32,
        profiles: Vec<(String, Option<bool>)>,
    ) {
        if let Some(panel) = self.panels.get_mut(&worker_id) {
            panel.status.verify_profiles = profiles;
        }
    }

    pub fn push_worker_output(&mut self, worker_id: u32, lines: &[String]) {
        if let Some(panel) = self.panels.get_mut(&worker_id) {
            for line in lines {
                panel.output.push_str(line);
            }
        }
    }

    /// Clear the output buffer for a worker and reset scroll.
    pub fn clear_worker_output(&mut self, worker_id: u32) {
        if let Some(panel) = self.panels.get_mut(&worker_id) {
            panel.output.clear();
            panel.scroll_offset = 0;
        }
    }

    pub fn push_log_line(&mut self, text: &str) {
        if text.is_empty() {
            if self.log_lines.len() >= 50 {
                self.log_lines.pop_front();
            }
            self.log_lines.push_back(Line::raw(""));
            return;
        }

        let lines_to_push: Vec<Line<'static>> = match text.into_text() {
            Ok(parsed) => parsed.lines,
            Err(_) => text.lines().map(|l| Line::raw(l.to_string())).collect(),
        };

        for line in lines_to_push {
            if self.log_lines.len() >= 50 {
                self.log_lines.pop_front();
            }
            self.log_lines.push_back(line);
        }
    }

    /// Whether graceful shutdown was requested.
    pub fn is_graceful_shutdown(&self) -> bool {
        self.graceful_shutdown
    }

    /// Get current restart state for orchestrator to consume.
    pub fn restart_state(&self) -> &RestartState {
        &self.restart_state
    }

    /// Reset restart state after orchestrator consumed it.
    pub fn clear_restart(&mut self) {
        self.restart_state = RestartState::None;
    }

    /// Whether reload was requested (klawisz 'r').
    /// Resets the flag after reading.
    pub fn take_reload_requested(&mut self) -> bool {
        std::mem::replace(&mut self.reload_requested, false)
    }

    /// Take all pending user messages (from overlay Send action).
    /// Returns messages as (worker_id, message) tuples, draining the buffer.
    pub fn take_pending_messages(&mut self) -> Vec<(u32, String)> {
        self.pending_user_messages.drain(..).collect()
    }

    /// Read-only access to panels.
    pub fn panels(&self) -> &HashMap<u32, WorkerPanel> {
        &self.panels
    }

    /// Read-only access to focused worker.
    pub fn focused_worker(&self) -> Option<u32> {
        self.focused_worker
    }

    /// Read-only access to tasks_file (used by command registry).
    pub fn tasks_file(&self) -> Option<&TasksFile> {
        self.tasks_file.as_ref()
    }

    /// Otwórz command palette z dynamicznie zbudowanymi elementami.
    ///
    /// Jeśli palette jest już otwarta — odświeża jej listę elementów.
    pub fn open_command_palette(&mut self) {
        use crate::commands::task::orchestrate::command_registry::build_orchestrate_items;
        let items = build_orchestrate_items(self);
        self.command_palette = Some(CommandPaletteState::new(items));
    }

    /// Zamknij command palette.
    pub fn close_command_palette(&mut self) {
        self.command_palette = None;
    }

    /// Czy command palette jest aktualnie otwarta.
    pub fn is_palette_open(&self) -> bool {
        self.command_palette.is_some()
    }

    /// Set focus to a specific worker.
    pub fn set_focus(&mut self, worker_id: Option<u32>) {
        if self.focused_worker != worker_id {
            self.focused_worker = worker_id;
            if let Some(id) = worker_id
                && let Some(panel) = self.panels.get_mut(&id)
            {
                panel.scroll_offset = 0;
            }
        }
    }

    /// Auto-shift focus to first active worker if focused worker became idle.
    pub fn auto_focus_active(&mut self) -> Option<u32> {
        let focused = self.focused_worker?;

        let is_idle = self
            .panels
            .get(&focused)
            .map(|p| p.status.state == WorkerState::Idle)
            .unwrap_or(false);

        if !is_idle {
            return None;
        }

        let first_active = (1..=self.worker_count).find(|&id| {
            self.panels
                .get(&id)
                .map(|p| p.status.state != WorkerState::Idle)
                .unwrap_or(false)
        });

        if let Some(new_focus) = first_active {
            self.set_focus(Some(new_focus));
            Some(new_focus)
        } else {
            self.set_focus(None);
            Some(0)
        }
    }

    // ── Internal helpers ────────────────────────────────────────────

    /// Refresh sorted list of active (non-idle) worker IDs.
    pub(crate) fn refresh_active_worker_ids(&mut self) {
        self.active_worker_ids.clear();
        for id in 1..=self.worker_count {
            if let Some(panel) = self.panels.get(&id)
                && !panel.status.is_idle()
            {
                self.active_worker_ids.push(id);
            }
        }
    }

    /// Apply scroll delta to focused panel or preview overlay.
    ///
    /// Worker panel scroll: offset 0 = follow tail (newest), offset > 0 = scroll up (older).
    /// Preview scroll: offset 0 = top of document, offset > 0 = scroll down.
    pub(crate) fn apply_scroll(&mut self, delta: i32) {
        if self.sidebar_focused && self.sidebar_state.visible {
            match delta {
                d if d < 0 => self.sidebar_state.select_prev(),
                d if d > 0 => self.sidebar_state.select_next(),
                _ => {} // Left/Right (i32::MIN/MAX) — ignoruj w sidebar
            }
            return;
        }
        if self.show_task_preview {
            // Scroll the task preview overlay (top-to-bottom: offset 0 = top)
            match delta {
                i32::MIN => self.preview_scroll_offset = 0,
                i32::MAX => self.preview_scroll_offset = usize::MAX,
                d if d < 0 => {
                    let up = (-d) as usize;
                    self.preview_scroll_offset = self.preview_scroll_offset.saturating_sub(up);
                }
                d if d > 0 => {
                    let down = d as usize;
                    self.preview_scroll_offset = self.preview_scroll_offset.saturating_add(down);
                }
                _ => {}
            }
        } else {
            // Scroll focused worker panel (bottom-up: offset 0 = follow tail)
            let Some(wid) = self.focused_worker else {
                return;
            };
            let Some(panel) = self.panels.get_mut(&wid) else {
                return;
            };

            match delta {
                // Left = jump to top (oldest) = max offset
                i32::MIN => panel.scroll_offset = usize::MAX,
                // Right = jump to bottom (newest/tail) = offset 0
                i32::MAX => panel.scroll_offset = 0,
                // Up = scroll toward older content = increase offset
                d if d < 0 => {
                    let up = (-d) as usize;
                    if panel.scroll_offset == 0 {
                        panel.scroll_offset = up;
                    } else {
                        panel.scroll_offset = panel.scroll_offset.saturating_add(up);
                    }
                }
                // Down = scroll toward newer content = decrease offset
                d if d > 0 => {
                    let down = d as usize;
                    if panel.scroll_offset > 0 {
                        panel.scroll_offset = panel.scroll_offset.saturating_sub(down);
                    }
                }
                _ => {}
            }
        }
    }

    /// Cancel quit pending if active.
    pub(crate) fn cancel_quit_pending(&mut self) -> bool {
        if self.quit_state == QuitState::Pending {
            self.quit_state = QuitState::Normal;
            true
        } else {
            false
        }
    }

    /// Cancel restart pending if active.
    pub(crate) fn cancel_restart_pending(&mut self) -> bool {
        if matches!(self.restart_state, RestartState::Pending { .. }) {
            self.restart_state = RestartState::None;
            true
        } else {
            false
        }
    }

    /// Toggle task sidebar: focused → hide, visible → focus, hidden → show + focus.
    pub(crate) fn toggle_sidebar(&mut self) {
        if self.sidebar_state.visible && self.sidebar_focused {
            // focused → hide
            self.sidebar_state.toggle_visible();
            self.sidebar_focused = false;
        } else if self.sidebar_state.visible {
            // visible but not focused → focus
            self.sidebar_focused = true;
        } else {
            // hidden → show + focus
            self.sidebar_state.toggle_visible();
            self.sidebar_focused = true;
        }
    }

    /// Remove focus from sidebar (back to worker grid).
    pub(crate) fn unfocus_sidebar(&mut self) {
        self.sidebar_focused = false;
    }
}

// ── AppState implementation ─────────────────────────────────────────

impl AppState for OrchestrateApp {
    fn draw(&mut self, frame: &mut Frame, area: Rect) {
        // Refresh active worker IDs before drawing
        self.refresh_active_worker_ids();

        // TODO(task 6.10): Re-implement compact mode after panels.rs removal
        // Small terminal: compact mode
        // if area.width < 60 || area.height < 12 {
        //     render_compact(...);
        //     return;
        // }

        // Split screen: content (fill) + 3-line status bar
        let vertical = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(area);

        // Split content area: optional sidebar + worker grid
        let content_area = vertical[0];
        let is_small = content_area.width < 80;

        let grid_area = if self.sidebar_state.visible && !is_small {
            // Inline sidebar layout (normal width)
            let horizontal = Layout::horizontal([
                Constraint::Length(self.sidebar_state.width()),
                Constraint::Min(1),
            ])
            .split(content_area);

            // Cache sidebar rect dla hit-testingu
            self.sidebar_rect = Some(horizontal[0]);

            // Render sidebar
            TaskSidebar::new(&mut self.sidebar_state, self.sidebar_focused)
                .render(horizontal[0], frame.buffer_mut());

            horizontal[1]
        } else {
            self.sidebar_rect = None;
            content_area
        };

        // Render worker grid or task preview
        render_worker_grid(
            frame,
            grid_area,
            &WorkerGridConfig {
                worker_count: self.worker_count,
                focused: self.focused_worker,
                panels: &self.panels,
                show_preview: self.show_task_preview,
                preview_scroll: self.preview_scroll_offset,
                tasks_file: self.tasks_file.as_ref(),
                show_idle: self.show_idle,
            },
        );

        // Cache worker grid rects dla hit-testingu (puste gdy aktywny preview).
        // Obliczamy po render_worker_grid, stosując tę samą logikę filtrowania.
        // Reużywamy bufora (clear + extend) zamiast nowej alokacji w każdym frame.
        self.grid_rects.clear();
        if !self.show_task_preview {
            self.grid_rects.extend(compute_active_worker_rects(
                grid_area,
                self.worker_count,
                &self.panels,
                self.show_idle,
            ));
        }

        // Overlay sidebar for narrow terminals (on top of grid)
        if self.sidebar_state.visible && is_small {
            // W trybie overlay sidebar przykrywa cały content_area
            self.sidebar_rect = Some(content_area);
            crate::tui::widgets::render_sidebar_overlay(
                &mut self.sidebar_state,
                self.sidebar_focused,
                content_area,
                frame.buffer_mut(),
            );
        }

        // Render global status bar — użyj resolvera do wyświetlenia konfigurowalnego keybindingu
        let palette_key = self
            .resolver
            .key_for_action(KeyAction::CommandPalette)
            .map(|combo| KeybindingResolver::format_key(&combo))
            .unwrap_or_else(|| "Ctrl+p".to_string());
        let status_bar = render_global_bar(
            &self.orchestrator_status,
            self.focused_worker,
            self.show_task_preview,
            self.sidebar_focused && self.sidebar_state.visible,
            self.show_idle,
            &palette_key,
        );
        frame.render_widget(status_bar, vertical[1]);

        // Render text input overlay on top (if active)
        render_overlay(frame, area, &self.shared_overlay);

        // Cache overlay rect dla hit-testingu — area całego terminala gdy overlay aktywny
        self.overlay_rect = self
            .shared_overlay
            .lock()
            .ok()
            .and_then(|guard| guard.is_some().then_some(area));

        // Render command palette on very top — najwyższy z-order
        if let Some(ref mut palette) = self.command_palette {
            frame.render_stateful_widget(CommandPaletteWidget, area, palette);
        }
    }

    // TODO(11.4): zamienić hardcoded KeyCode checks w handle_key() na resolver.resolve()
    fn handle_event(
        &mut self,
        event: AppEvent,
        _resolver: &crate::tui::KeybindingResolver,
    ) -> EventResult {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Resize(_, _) => EventResult::Consumed,
            AppEvent::Mouse(_) => EventResult::Ignored,
            AppEvent::Tick => {
                // Refresh active workers on tick
                self.refresh_active_worker_ids();
                EventResult::Consumed
            }
        }
    }
}

/// Default OrchestratorStatus for initialization.
fn default_orchestrator_status() -> OrchestratorStatus {
    OrchestratorStatus {
        scheduler: SchedulerStatus {
            total: 0,
            done: 0,
            in_progress: 0,
            blocked: 0,
            ready: 0,
            pending: 0,
        },
        total_cost: 0.0,
        elapsed: Duration::ZERO,
        shutdown_state: ShutdownState::Running,
        shutdown_remaining: None,
        quit_pending: false,
        completed: false,
        restart_pending: None,
        active_workers: 0,
        idle_workers: 0,
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app(worker_count: u32) -> OrchestrateApp {
        let mut app = OrchestrateApp::new(worker_count, Arc::new(Mutex::new(None)));
        // Disable sidebar focus for most tests — sidebar-specific tests enable it explicitly
        app.sidebar_focused = false;
        app
    }

    // ── Construction tests ──────────────────────────────────────────

    #[test]
    fn new_creates_panels_for_all_workers() {
        let app = make_app(3);
        assert_eq!(app.panels.len(), 3);
        assert!(app.panels.contains_key(&1));
        assert!(app.panels.contains_key(&2));
        assert!(app.panels.contains_key(&3));
    }

    #[test]
    fn new_defaults_no_focus() {
        let app = make_app(3);
        assert_eq!(app.focused_worker, None);
    }

    #[test]
    fn new_defaults_quit_normal() {
        let app = make_app(1);
        assert_eq!(app.quit_state, QuitState::Normal);
    }

    #[test]
    fn new_defaults_no_restart() {
        let app = make_app(1);
        assert_eq!(app.restart_state, RestartState::None);
    }

    #[test]
    fn new_defaults_sidebar_hidden() {
        // Use OrchestrateApp::new directly to test real defaults (make_app disables sidebar focus)
        let app = OrchestrateApp::new(1, Arc::new(Mutex::new(None)));
        assert!(!app.sidebar_state.visible);
        assert!(!app.sidebar_focused);
    }

    // ── Worker status update tests ──────────────────────────────────

    #[test]
    fn update_worker_status_tracks_idle_transition() {
        let mut app = make_app(1);
        let mut status = WorkerStatus::idle(1);
        status.state = WorkerState::Implementing;
        app.update_worker_status(1, status);
        assert!(app.panels[&1].idle_since.is_none());

        let idle_status = WorkerStatus::idle(1);
        app.update_worker_status(1, idle_status);
        assert!(app.panels[&1].idle_since.is_some());
    }

    #[test]
    fn auto_focus_active_shifts_from_idle() {
        let mut app = make_app(3);

        // Worker 1: active, Worker 2: idle, Worker 3: active
        let mut status1 = WorkerStatus::idle(1);
        status1.state = WorkerState::Implementing;
        app.update_worker_status(1, status1);

        let mut status3 = WorkerStatus::idle(3);
        status3.state = WorkerState::Implementing;
        app.update_worker_status(3, status3);

        // Focus idle worker 2
        app.set_focus(Some(2));
        let new_focus = app.auto_focus_active();

        // Should shift to first active (1)
        assert_eq!(new_focus, Some(1));
        assert_eq!(app.focused_worker, Some(1));
    }

    // ── Push log line tests ─────────────────────────────────────────

    #[test]
    fn push_log_line_appends_to_log() {
        let mut app = make_app(1);
        app.push_log_line("hello");
        assert_eq!(app.log_lines.len(), 1);
    }

    #[test]
    fn push_log_line_respects_capacity() {
        let mut app = make_app(1);
        for i in 0..60 {
            app.push_log_line(&format!("line {i}"));
        }
        // Max capacity is 50
        assert!(app.log_lines.len() <= 50);
    }

    // ── Clear worker output tests ───────────────────────────────────

    #[test]
    fn clear_worker_output_resets_buffer_and_scroll() {
        let mut app = make_app(1);
        app.push_worker_output(1, &["line1".to_string(), "line2".to_string()]);
        app.panels.get_mut(&1).unwrap().scroll_offset = 5;

        app.clear_worker_output(1);
        assert_eq!(app.panels[&1].scroll_offset, 0);
    }

    #[test]
    fn clear_worker_output_ignores_unknown_worker() {
        let mut app = make_app(1);
        // Should not panic for unknown worker ID
        app.clear_worker_output(99);
    }

    // ── Update cost tests ───────────────────────────────────────────

    #[test]
    fn update_worker_cost_updates_fields() {
        let mut app = make_app(1);
        app.update_worker_cost(1, 1.5, 1000, 2000);
        let panel = &app.panels[&1];
        assert!((panel.status.cost_usd - 1.5).abs() < f64::EPSILON);
        assert_eq!(panel.status.input_tokens, 1000);
        assert_eq!(panel.status.output_tokens, 2000);
    }

    // ── Preview scroll tests ────────────────────────────────────────

    #[test]
    fn scroll_preview_down_increases_offset() {
        let mut app = make_app(1);
        app.show_task_preview = true;
        app.preview_scroll_offset = 0;

        app.apply_scroll(3);
        assert_eq!(app.preview_scroll_offset, 3);
    }

    #[test]
    fn scroll_preview_up_decreases_offset() {
        let mut app = make_app(1);
        app.show_task_preview = true;
        app.preview_scroll_offset = 5;

        app.apply_scroll(-2);
        assert_eq!(app.preview_scroll_offset, 3);
    }

    #[test]
    fn scroll_preview_up_clamps_to_zero() {
        let mut app = make_app(1);
        app.show_task_preview = true;
        app.preview_scroll_offset = 1;

        app.apply_scroll(-5);
        assert_eq!(app.preview_scroll_offset, 0);
    }

    #[test]
    fn scroll_preview_left_jumps_to_top() {
        let mut app = make_app(1);
        app.show_task_preview = true;
        app.preview_scroll_offset = 100;

        app.apply_scroll(i32::MIN);
        assert_eq!(app.preview_scroll_offset, 0);
    }

    // ── Worker panel scroll tests ───────────────────────────────────

    #[test]
    fn scroll_panel_up_from_tail() {
        let mut app = make_app(1);
        app.focused_worker = Some(1);
        // offset 0 = following tail
        app.panels.get_mut(&1).unwrap().scroll_offset = 0;

        // Up (delta=-1) should move away from tail
        app.apply_scroll(-1);
        assert_eq!(app.panels[&1].scroll_offset, 1);
    }

    #[test]
    fn scroll_panel_down_toward_tail() {
        let mut app = make_app(1);
        app.focused_worker = Some(1);
        app.panels.get_mut(&1).unwrap().scroll_offset = 5;

        // Down (delta=+1) should move toward tail
        app.apply_scroll(1);
        assert_eq!(app.panels[&1].scroll_offset, 4);
    }

    #[test]
    fn scroll_panel_down_clamps_to_zero() {
        let mut app = make_app(1);
        app.focused_worker = Some(1);
        app.panels.get_mut(&1).unwrap().scroll_offset = 1;

        app.apply_scroll(5); // More than offset
        assert_eq!(app.panels[&1].scroll_offset, 0);
    }

    #[test]
    fn scroll_no_focus_is_noop() {
        let mut app = make_app(1);
        app.focused_worker = None;
        // Should not panic
        app.apply_scroll(-1);
        app.apply_scroll(1);
    }

    // ── Reload requested tests ─────────────────────────────────────

    #[test]
    fn new_defaults_reload_not_requested() {
        let app = make_app(1);
        assert!(!app.reload_requested);
    }

    #[test]
    fn take_reload_requested_returns_and_resets() {
        let mut app = make_app(1);
        app.reload_requested = true;
        assert!(app.take_reload_requested());
        assert!(!app.reload_requested);
    }

    // ── Cached hit-test rects tests ────────────────────────────────

    #[test]
    fn new_defaults_grid_rects_empty() {
        let app = make_app(3);
        assert!(app.grid_rects.is_empty());
    }

    #[test]
    fn new_defaults_sidebar_rect_none() {
        let app = make_app(3);
        assert!(app.sidebar_rect.is_none());
    }

    #[test]
    fn new_defaults_overlay_rect_none() {
        let app = make_app(3);
        assert!(app.overlay_rect.is_none());
    }

    // ── Pending user messages tests ────────────────────────────────

    #[test]
    fn new_defaults_no_pending_messages() {
        let app = make_app(1);
        assert!(app.pending_user_messages.is_empty());
    }

    #[test]
    fn take_pending_messages_drains_buffer() {
        let mut app = make_app(1);
        app.pending_user_messages.push_back((1, "hello".into()));
        app.pending_user_messages.push_back((2, "world".into()));

        let msgs = app.take_pending_messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], (1, "hello".to_string()));
        assert_eq!(msgs[1], (2, "world".to_string()));

        // Buffer should be empty
        assert!(app.take_pending_messages().is_empty());
    }
}
