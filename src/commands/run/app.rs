//! RunApp — per-command state dla run mode (fullscreen TUI).
//!
//! Agreguje OutputFormatter, ring buffer, sidebar, header/status data
//! i implementuje AppState trait do użycia z centralnym App<S>.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::tui::app::AppState;
use crate::tui::events::{AppEvent, EventResult};
use crate::tui::responsive::Breakpoint;
use crate::tui::responsive::LayoutAreas;
use crate::tui::ring_buffer::OutputRingBuffer;
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::widgets::{
    Header, HeaderData, OutputView, OutputViewState, ProgressData, SIDEBAR_PADDING_TOP,
    SplashScreen, StatusBar, StatusBarData, TaskSidebar, TaskSidebarState,
};

use super::output::OutputFormatter;
use super::runner::ClaudeEvent;

// ── Enum: application phase ────────────────────────────────────────

/// Faza aplikacji — splash screen vs main layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    /// Wyświetlanie splash screen (1.5 sekundy)
    Splash,
    /// Główny layout (output + sidebar + status bar)
    Running,
}

// ── Enum: focus area ────────────────────────────────────────────────

/// Który panel ma aktualnie focus (wpływa na routing klawiszy ↑↓).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusArea {
    Output,
    Sidebar,
}

// ── RunApp struct ───────────────────────────────────────────────────

/// Per-command state dla run mode — fullscreen TUI.
///
/// Zawiera:
/// - `ring_buffer` — bufor wyjścia (Vec<Line>) do OutputView
/// - `sidebar` — stan task tree sidebar
/// - `output_formatter` — formatowanie eventów Claude → Lines
/// - `output_view_state` — scroll state (auto-follow, offset)
/// - `header_data` — dane nagłówka (command, model, iteration, elapsed)
/// - `status_data` — dane status bar (tokens, cost, elapsed, progress)
/// - `focus` — który panel ma focus
/// - `start_time` — czas startu sesji
/// - `shutdown` — flaga shutdown (współdzielona z App)
/// - `use_nerd_font` — Nerd Font icons vs ASCII fallback
/// - `tasks_file_path` — ścieżka do tasks.yml (opcjonalna)
/// - `last_tasks_mtime` — ostatni znany mtime tasks.yml (auto-refresh sidebara)
/// - `last_tasks_check` — ostatni czas sprawdzenia tasks.yml (throttle)
/// - `current_breakpoint` — aktualny breakpoint (Large/Medium/Small)
pub struct RunApp {
    pub ring_buffer: OutputRingBuffer,
    pub sidebar: TaskSidebarState,
    pub output_formatter: OutputFormatter,
    pub output_view_state: OutputViewState,
    pub header_data: HeaderData,
    pub status_data: StatusBarData,
    pub focus: FocusArea,
    pub start_time: Instant,
    pub shutdown: Arc<AtomicBool>,
    pub use_nerd_font: bool,
    /// Ostatni znany rozmiar output area — cache'owany w draw() do scroll_home.
    last_output_area: Rect,
    /// Aktualna faza aplikacji — splash screen lub running layout
    pub phase: RunPhase,
    /// Czas wejścia do fazy Splash — służy do odmierzenia 1.5s timera
    pub(super) splash_start_time: Option<Instant>,
    /// Quit confirmation state: pierwszy 'q' ustawia true, drugi 'q' lub Enter potwierdza quit
    pub(super) quit_pending: bool,
    /// Ścieżka do tasks.yml — jeśli Some, sidebar auto-refresh jest aktywny
    tasks_file_path: Option<PathBuf>,
    /// Ostatni znany mtime tasks.yml (do wykrywania zmian pliku)
    last_tasks_mtime: Option<SystemTime>,
    /// Ostatni czas sprawdzenia mtime (throttle: sprawdzaj co 2s)
    last_tasks_check: Instant,
    /// Aktualny responsywny breakpoint (Large/Medium/Small).
    current_breakpoint: Breakpoint,
    /// Liczba linii przewijana przy zdarzeniu scroll myszy (z TuiConfig)
    pub scroll_step: usize,
    /// Cache'owany rect sidebara — aktualizowany w każdym draw() (do hit-testingu).
    pub(crate) sidebar_rect: Option<Rect>,
    /// Cache'owany rect panelu output — aktualizowany w każdym draw() (do hit-testingu).
    pub(crate) output_rect: Option<Rect>,
}

impl RunApp {
    /// Tworzy nowy RunApp z domyślnymi wartościami.
    ///
    /// # Argumenty
    /// - `command_name` — nazwa komendy (np. "run")
    /// - `model` — identyfikator modelu (np. "claude-sonnet-4-5")
    /// - `use_nerd_font` — Nerd Font icons
    /// - `buffer_capacity` — rozmiar ring buffera (domyślnie 5000)
    /// - `show_splash` — czy wyświetlić splash screen na starcie
    pub fn new(
        command_name: impl Into<String>,
        model: impl Into<String>,
        use_nerd_font: bool,
        buffer_capacity: usize,
        show_splash: bool,
    ) -> Self {
        let initial_phase = if show_splash {
            RunPhase::Splash
        } else {
            RunPhase::Running
        };

        Self {
            ring_buffer: OutputRingBuffer::with_capacity(buffer_capacity),
            sidebar: TaskSidebarState::default(),
            output_formatter: OutputFormatter::new(use_nerd_font),
            output_view_state: OutputViewState::default(),
            header_data: HeaderData {
                command_name: command_name.into(),
                model: model.into(),
                iteration: None,
                max_iterations: None,
                elapsed: std::time::Duration::ZERO,
                is_running: false,
            },
            status_data: StatusBarData::default(),
            focus: FocusArea::Output,
            start_time: Instant::now(),
            shutdown: Arc::new(AtomicBool::new(false)),
            use_nerd_font,
            last_output_area: Rect::default(),
            phase: initial_phase,
            splash_start_time: if show_splash {
                Some(Instant::now())
            } else {
                None
            },
            quit_pending: false,
            tasks_file_path: None,
            last_tasks_mtime: None,
            last_tasks_check: Instant::now(),
            current_breakpoint: Breakpoint::Large, // domyślnie Large
            scroll_step: 3,
            sidebar_rect: None,
            output_rect: None,
        }
    }

    /// Ustaw konfigurowalny scroll step (z TuiConfig). Builder pattern.
    pub fn with_scroll_step(mut self, step: u16) -> Self {
        self.scroll_step = step.max(1) as usize;
        self
    }

    /// Ustaw ścieżkę do tasks.yml i załaduj początkowy stan (opcjonalnie).
    ///
    /// Po ustawieniu, sidebar będzie automatycznie odświeżany co 2s jeśli plik się zmieni.
    /// Jeśli plik istnieje, natychmiast ładuje task tree do sidebara.
    #[allow(dead_code)] // Public API - reserved for future use
    pub fn set_tasks_file(&mut self, path: PathBuf) {
        // Spróbuj załadować początkowy stan (borrow &path przed move)
        if let Ok(tf) = crate::shared::tasks::TasksFile::load(&path) {
            self.sidebar.refresh(&tf);
            // Zapisz mtime początkowy
            if let Ok(meta) = std::fs::metadata(&path)
                && let Ok(mtime) = meta.modified()
            {
                self.last_tasks_mtime = Some(mtime);
            }
        }
        self.tasks_file_path = Some(path);
    }

    /// Zwraca aktualny breakpoint (Large/Medium/Small).
    ///
    /// Używane w testach do weryfikacji responsywności.
    #[allow(dead_code)]
    pub fn current_breakpoint(&self) -> Breakpoint {
        self.current_breakpoint
    }

    /// Formatuj ClaudeEvent → Vec<Line> i wrzuć do ring buffera.
    pub fn push_event(&mut self, event: &ClaudeEvent) {
        let lines = self.output_formatter.format_event(event);
        self.push_lines(lines);
    }

    /// Wrzuć wstępnie sformatowane linie do ring buffera.
    pub fn push_lines(&mut self, lines: Vec<Line<'static>>) {
        for line in lines {
            self.ring_buffer.push(line);
        }
    }

    /// Odśwież StatusBarData z aktualnych danych OutputFormatter.
    pub fn update_status(&mut self) {
        let status = self.output_formatter.get_status();
        self.status_data = StatusBarData {
            input_tokens: status.input_tokens,
            output_tokens: status.output_tokens,
            cost_usd: status.cost_usd,
            elapsed_secs: self.start_time.elapsed().as_secs_f64(),
            progress: status.task_progress.as_ref().map(|tp| ProgressData {
                done: tp.done,
                total: tp.total,
                eta_text: status.eta_text.clone(),
            }),
            hints: self.keybinding_hints(),
        };

        // Aktualizuj header elapsed + iteration
        self.header_data.elapsed = self.start_time.elapsed();

        // Aktualizuj current task w sidebarze (dla highlighting)
        let current_task_id = status
            .task_progress
            .as_ref()
            .and_then(|tp| tp.current_task_id.clone());
        self.sidebar.set_current_task(current_task_id);
    }

    /// Keybinding hints dla status bar — zależne od focus i quit_pending.
    fn keybinding_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.quit_pending {
            // W trybie quit_pending pokazujemy specjalne podpowiedzi
            vec![("q/Enter", "Confirm"), ("Esc", "Cancel")]
        } else {
            // Normalne podpowiedzi
            let mut h = vec![("q", "Quit"), ("t", "Sidebar")];
            match self.focus {
                FocusArea::Output => h.push(("↑↓", "Scroll")),
                FocusArea::Sidebar => h.push(("↑↓", "Navigate")),
            }
            h.push(("Tab", "Focus"));
            h
        }
    }

    /// Obsługa klawiszy sidebar (gdy focus=Sidebar).
    fn handle_sidebar_key(&mut self, key: &KeyEvent) -> EventResult {
        match key.code {
            KeyCode::Up => {
                self.cancel_quit_pending();
                self.sidebar.select_prev();
                EventResult::Consumed
            }
            KeyCode::Down => {
                self.cancel_quit_pending();
                self.sidebar.select_next();
                EventResult::Consumed
            }
            KeyCode::Enter if !self.quit_pending => {
                // Enter w sidebar tylko gdy nie jest quit_pending (wtedy Enter = confirm quit)
                self.sidebar.toggle_expand();
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }

    /// Obsługa klawiszy output (gdy focus=Output).
    fn handle_output_key(&mut self, key: &KeyEvent) -> EventResult {
        match key.code {
            KeyCode::Up => {
                self.cancel_quit_pending();
                self.output_view_state.scroll_up(self.scroll_step);
                EventResult::Consumed
            }
            KeyCode::Down => {
                self.cancel_quit_pending();
                self.output_view_state.scroll_down(self.scroll_step);
                EventResult::Consumed
            }
            KeyCode::PageUp => {
                self.cancel_quit_pending();
                self.output_view_state.scroll_up(self.scroll_step * 3);
                EventResult::Consumed
            }
            KeyCode::PageDown => {
                self.cancel_quit_pending();
                self.output_view_state.scroll_down(self.scroll_step * 3);
                EventResult::Consumed
            }
            KeyCode::Home => {
                self.cancel_quit_pending();
                // Użyj cache'owanego rozmiaru output area z ostatniego draw()
                let width = self.last_output_area.width;
                let height = self.last_output_area.height as usize;
                let total = self.ring_buffer.total_visual_rows(width);
                self.output_view_state.scroll_home(total, height);
                EventResult::Consumed
            }
            KeyCode::End => {
                self.cancel_quit_pending();
                self.output_view_state.scroll_end();
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }

    /// Sprawdź czy tasks.yml się zmienił (co 2s) i odśwież sidebar jeśli potrzeba.
    ///
    /// Throttle: sprawdza mtime tylko jeśli minęły 2s od ostatniego check.
    /// Jeśli mtime się zmienił, reload tasks.yml i refresh sidebar.
    fn check_tasks_reload(&mut self) {
        let path = match &self.tasks_file_path {
            Some(p) => p,
            None => return, // Brak tasks_file_path — sidebar nie jest w trybie auto-refresh
        };

        // Throttle: sprawdź tylko jeśli minęły 2s od ostatniego check
        let now = Instant::now();
        if now.duration_since(self.last_tasks_check).as_secs() < 2 {
            return;
        }
        self.last_tasks_check = now;

        // Sprawdź mtime
        let mtime = match std::fs::metadata(path).ok().and_then(|m| m.modified().ok()) {
            Some(m) => m,
            None => return, // Plik nie istnieje lub brak dostępu — ignore
        };

        // Porównaj z ostatnim znanym mtime
        if self.last_tasks_mtime.as_ref() == Some(&mtime) {
            return; // Bez zmian
        }

        // Mtime się zmienił — reload tasks.yml
        // Aktualizuj mtime TYLKO po udanym load (retry przy następnym check jeśli load failuje)
        if let Ok(tf) = crate::shared::tasks::TasksFile::load(path) {
            self.last_tasks_mtime = Some(mtime);
            self.sidebar.refresh(&tf);
        }
    }

    /// Obsługa klawiszy globalnych (niezależnych od focus).
    fn handle_global_key(&mut self, key: &KeyEvent) -> EventResult {
        match key.code {
            // 'q' — quit confirmation flow (pierwszy q → pending, drugi q → Quit)
            KeyCode::Char('q') => {
                if self.quit_pending {
                    // Drugi 'q' — potwierdź quit
                    EventResult::Quit
                } else {
                    // Pierwszy 'q' — wejdź w tryb quit_pending
                    self.quit_pending = true;
                    EventResult::Consumed
                }
            }
            // Enter — potwierdź quit gdy quit_pending
            KeyCode::Enter if self.quit_pending => EventResult::Quit,
            // Esc — anuluj quit_pending
            KeyCode::Esc => {
                if self.quit_pending {
                    self.quit_pending = false;
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            // 't' — toggle sidebar visibility
            KeyCode::Char('t') => {
                self.cancel_quit_pending();
                self.sidebar.toggle_visible();
                EventResult::Consumed
            }
            // '[' / '-' — shrink sidebar
            KeyCode::Char('[') | KeyCode::Char('-') => {
                self.cancel_quit_pending();
                self.sidebar.shrink();
                EventResult::Consumed
            }
            // ']' / '+' / '=' — grow sidebar
            KeyCode::Char(']') | KeyCode::Char('+') | KeyCode::Char('=') => {
                self.cancel_quit_pending();
                self.sidebar.grow();
                EventResult::Consumed
            }
            // Tab — switch focus
            KeyCode::Tab => {
                self.cancel_quit_pending();
                self.focus = match self.focus {
                    FocusArea::Output => FocusArea::Sidebar,
                    FocusArea::Sidebar => FocusArea::Output,
                };
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }

    /// Obsługa zdarzeń myszy w run mode.
    ///
    /// Routing:
    /// - Lewy klik w `sidebar_rect` → focus Sidebar + zaznaczenie tasku pod kursorem
    /// - Lewy klik w `output_rect` → focus Output
    /// - ScrollUp/ScrollDown nad sidebar → nawigacja po task tree (krok=1)
    /// - ScrollUp/ScrollDown nad output → scroll output buffera (krok=`scroll_step` z config)
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> EventResult {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_mouse_left_click(mouse.column, mouse.row)
            }
            MouseEventKind::ScrollUp => self.handle_mouse_scroll(mouse.column, mouse.row, true),
            MouseEventKind::ScrollDown => self.handle_mouse_scroll(mouse.column, mouse.row, false),
            _ => EventResult::Ignored,
        }
    }

    /// Obsługa scroll wheel myszy — kontekstowy routing.
    ///
    /// Hit-test decyduje o akcji:
    /// - Kursor nad sidebar → nawigacja po task tree (`select_prev`/`select_next`, krok=1)
    /// - Kursor nad output → scroll output buffera (`scroll_up`/`scroll_down`, krok=`self.scroll_step`)
    /// - Kursor poza oboma → Ignored
    fn handle_mouse_scroll(&mut self, col: u16, row: u16, scroll_up: bool) -> EventResult {
        let pos = Position::new(col, row);

        // Hit-test na sidebar (priorytet nad output, może nakładać się w Small mode overlay)
        if let Some(sidebar_rect) = self.sidebar_rect
            && sidebar_rect.contains(pos)
        {
            self.cancel_quit_pending();
            if scroll_up {
                self.sidebar.select_prev();
            } else {
                self.sidebar.select_next();
            }
            return EventResult::Consumed;
        }

        // Hit-test na output area
        if let Some(output_rect) = self.output_rect
            && output_rect.contains(pos)
        {
            self.cancel_quit_pending();
            if scroll_up {
                self.output_view_state.scroll_up(self.scroll_step);
            } else {
                self.output_view_state.scroll_down(self.scroll_step);
            }
            return EventResult::Consumed;
        }

        EventResult::Ignored
    }

    /// Obsługa lewego kliknięcia myszy.
    ///
    /// - Klik w `output_rect` → focus Output
    /// - Klik w `sidebar_rect` → focus Sidebar + zaznaczenie tasku pod kursorem
    fn handle_mouse_left_click(&mut self, col: u16, row: u16) -> EventResult {
        let pos = Position::new(col, row);

        // Klik w output_rect → focus Output
        if let Some(output_rect) = self.output_rect
            && output_rect.contains(pos)
        {
            self.cancel_quit_pending();
            self.focus = FocusArea::Output;
            return EventResult::Consumed;
        }

        // Klik w sidebar_rect → focus Sidebar + zaznaczenie tasku pod kursorem
        let Some(sidebar_rect) = self.sidebar_rect else {
            return EventResult::Ignored;
        };

        if !sidebar_rect.contains(pos) {
            return EventResult::Ignored;
        }

        self.focus = FocusArea::Sidebar;

        // Oblicz task index z row pozycji.
        // inner_y = sidebar_rect.y + SIDEBAR_PADDING_TOP (z task_sidebar.rs).
        // Task i jest renderowany w wierszu inner_y + i (uwzględniając scroll_offset).
        let inner_y = sidebar_rect.y.saturating_add(SIDEBAR_PADDING_TOP);
        if row >= inner_y {
            let row_within_inner = (row - inner_y) as usize;
            let task_index = self.sidebar.scroll_offset + row_within_inner;
            self.sidebar.select_index(task_index);
        }

        EventResult::Consumed
    }

    /// Anuluj stan quit_pending (wywołuj przy innych akcjach).
    fn cancel_quit_pending(&mut self) {
        self.quit_pending = false;
    }
}

// ── AppState impl ───────────────────────────────────────────────────

impl AppState for RunApp {
    fn draw(&mut self, frame: &mut Frame, area: Rect) {
        // Jeśli jesteśmy w fazie Splash, renderuj tylko splash screen
        if self.phase == RunPhase::Splash {
            let splash = SplashScreen;
            frame.render_widget(splash, area);
            return;
        }

        // Sync breakpoint z aktualnym rozmiarem area (fallback jeśli Resize event
        // nie dotarł, np. pierwsze renderowanie).
        let bp = Breakpoint::detect(area.width);
        self.current_breakpoint = bp;
        let layout = LayoutAreas::for_breakpoint(bp, area);

        // Header (tylko Large breakpoint)
        if let Some(header_area) = layout.header {
            let header = Header::new(&self.header_data, &DEFAULT_THEME);
            frame.render_widget(header, header_area);
        }

        // Sidebar (Large / Medium breakpoint)
        if let Some(sidebar_area) = layout.sidebar {
            if self.sidebar.visible {
                // Ograniczenie szerokości sidebar do sidebar_area.width
                let actual_width = sidebar_area.width.min(self.sidebar.width());
                let sidebar_rect = Rect {
                    width: actual_width,
                    ..sidebar_area
                };
                let sidebar_focused = self.focus == FocusArea::Sidebar;
                let sidebar_widget = TaskSidebar::new(&mut self.sidebar, sidebar_focused);
                sidebar_widget.render(sidebar_rect, frame.buffer_mut());

                // Cache sidebar rect do hit-testingu
                self.sidebar_rect = Some(sidebar_rect);

                // Output wypełnia resztę
                let output_x = sidebar_area.x + actual_width;
                let output_width =
                    (sidebar_area.width + layout.content.width).saturating_sub(actual_width);
                let output_area = Rect {
                    x: output_x,
                    y: layout.content.y,
                    width: output_width,
                    height: layout.content.height,
                };
                self.last_output_area = output_area;
                self.output_rect = Some(output_area);
                let output_view = OutputView::new(&self.ring_buffer);
                // Rezerwujemy 1 kolumnę po prawej dla scrollbara
                let output_content_area = Rect {
                    width: output_area.width.saturating_sub(1),
                    ..output_area
                };
                frame.render_stateful_widget(
                    output_view,
                    output_content_area,
                    &mut self.output_view_state,
                );
            } else {
                // Sidebar ukryty — output zajmuje pełną content width
                let full_content = Rect {
                    x: sidebar_area.x,
                    width: sidebar_area.width + layout.content.width,
                    ..layout.content
                };
                self.last_output_area = full_content;
                self.sidebar_rect = None;
                self.output_rect = Some(full_content);
                let output_view = OutputView::new(&self.ring_buffer);
                // Rezerwujemy 1 kolumnę po prawej dla scrollbara
                let output_content_area = Rect {
                    width: full_content.width.saturating_sub(1),
                    ..full_content
                };
                frame.render_stateful_widget(
                    output_view,
                    output_content_area,
                    &mut self.output_view_state,
                );
            }
        } else {
            // Small breakpoint — output na pełną szerokość, sidebar jako overlay
            self.last_output_area = layout.content;
            self.sidebar_rect = None;
            self.output_rect = Some(layout.content);
            let output_view = OutputView::new(&self.ring_buffer);
            // Rezerwujemy 1 kolumnę po prawej dla scrollbara
            let output_content_area = Rect {
                width: layout.content.width.saturating_sub(1),
                ..layout.content
            };
            frame.render_stateful_widget(
                output_view,
                output_content_area,
                &mut self.output_view_state,
            );

            // Overlay sidebar na wierzchu (jeśli visible)
            if self.sidebar.visible {
                let sidebar_focused = self.focus == FocusArea::Sidebar;
                crate::tui::widgets::render_sidebar_overlay(
                    &mut self.sidebar,
                    sidebar_focused,
                    layout.content,
                    frame.buffer_mut(),
                );
            }
        }

        // ── Output scrollbar (VerticalRight, widoczny tylko gdy content > viewport) ──
        {
            let output_area = self.last_output_area;
            // Używamy zmniejszonej szerokości (content area bez kolumny scrollbara)
            let content_width = output_area.width.saturating_sub(1);
            let total_visual = self.ring_buffer.total_visual_rows(content_width);
            let viewport_h = output_area.height as usize;
            if total_visual > viewport_h {
                let max_scroll = total_visual - viewport_h;
                let pos = max_scroll.saturating_sub(self.output_view_state.scroll_offset);
                let mut sb = ScrollbarState::default()
                    .content_length(max_scroll + 1)
                    .viewport_content_length(viewport_h)
                    .position(pos);
                frame.render_stateful_widget(
                    Scrollbar::new(ScrollbarOrientation::VerticalRight)
                        .thumb_symbol("▐")
                        .track_symbol(Some("▐"))
                        .thumb_style(Style::default().fg(DEFAULT_THEME.secondary))
                        .track_style(Style::default().fg(DEFAULT_THEME.border_normal))
                        .begin_symbol(None)
                        .end_symbol(None),
                    output_area,
                    &mut sb,
                );
            }
        }

        // Status bar
        let status_bar =
            StatusBar::new(self.status_data.clone(), &DEFAULT_THEME, self.use_nerd_font);
        frame.render_widget(status_bar, layout.status_bar);
    }

    // TODO(11.4): zamienić hardcoded KeyCode checks poniżej na resolver.resolve()
    fn handle_event(
        &mut self,
        event: AppEvent,
        _resolver: &crate::tui::KeybindingResolver,
    ) -> EventResult {
        match event {
            AppEvent::Key(key) => {
                // Jeśli jest splash screen — każdy klawisz go pomija, przejdź do Running
                if self.phase == RunPhase::Splash {
                    self.phase = RunPhase::Running;
                    self.splash_start_time = None;
                    return EventResult::Consumed;
                }

                // Ctrl+C → shutdown (obsługiwane globalnie w App, tu safety fallback)
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    self.shutdown.store(true, Ordering::SeqCst);
                    return EventResult::Shutdown;
                }

                // Globalne klawisze (t, [, ], Tab) — niezależne od focus
                let global_result = self.handle_global_key(&key);
                if global_result != EventResult::Ignored {
                    return global_result;
                }

                // Routing do focus area
                match self.focus {
                    FocusArea::Output => self.handle_output_key(&key),
                    FocusArea::Sidebar => self.handle_sidebar_key(&key),
                }
            }
            AppEvent::Resize(w, _h) => {
                // Recalculate breakpoint — layout przeliczany w draw()
                let new_breakpoint = Breakpoint::detect(w);
                self.current_breakpoint = new_breakpoint;
                // Sidebar jest teraz dostępny jako overlay w Small mode — focus zachowany
                EventResult::Consumed
            }
            AppEvent::Mouse(mouse) => self.handle_mouse(mouse),
            AppEvent::Tick => {
                // Jeśli splash screen jest aktywny — sprawdź timer (1.5s)
                if self.phase == RunPhase::Splash {
                    if let Some(start) = self.splash_start_time
                        && start.elapsed().as_millis() >= 1500
                    {
                        self.phase = RunPhase::Running;
                        self.splash_start_time = None;
                    }
                    return EventResult::Consumed;
                }

                // W głównym layout — aktualizuj elapsed time
                self.update_status();
                // Sprawdź czy tasks.yml się zmienił (co 2s) i odśwież sidebar
                self.check_tasks_reload();
                EventResult::Consumed
            }
        }
    }
}

// ── Testy ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::KeybindingResolver;
    use crate::tui::events::AppEvent;
    use crate::tui::widgets::SIDEBAR_PADDING_TOP;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ratatui::backend::TestBackend;

    /// Helper: tworzy KeyEvent dla testów.
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn key_with_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn default_run_app() -> RunApp {
        // Domyślnie bez splash screen w testach dla szybszości
        RunApp::new("run", "claude-sonnet-4-5", false, 100, false)
    }

    // ── Konstrukcja ─────────────────────────────────────────────

    #[test]
    fn new_creates_default_state() {
        let app = default_run_app();

        assert_eq!(app.header_data.command_name, "run");
        assert_eq!(app.header_data.model, "claude-sonnet-4-5");
        assert_eq!(app.header_data.iteration, None);
        assert!(!app.header_data.is_running);
        assert_eq!(app.focus, FocusArea::Output);
        assert!(app.sidebar.visible);
        assert!(app.output_view_state.auto_follow);
        assert!(!app.shutdown.load(Ordering::SeqCst));
        assert_eq!(app.last_output_area, Rect::default());
        assert!(app.sidebar_rect.is_none());
        assert!(app.output_rect.is_none());
        assert_eq!(app.phase, RunPhase::Running);
        assert_eq!(app.splash_start_time, None);
    }

    #[test]
    fn new_with_splash_screen() {
        let app = RunApp::new("run", "claude-sonnet-4-5", false, 100, true);
        assert_eq!(app.phase, RunPhase::Splash);
        assert!(app.splash_start_time.is_some());
    }

    #[test]
    fn new_without_splash_screen() {
        let app = RunApp::new("run", "claude-sonnet-4-5", false, 100, false);
        assert_eq!(app.phase, RunPhase::Running);
        assert_eq!(app.splash_start_time, None);
    }

    // ── push_lines / push_event ─────────────────────────────────

    #[test]
    fn push_lines_adds_to_ring_buffer() {
        let mut app = default_run_app();
        let lines = vec![Line::raw("Hello"), Line::raw("World")];
        app.push_lines(lines);

        let visible = app.ring_buffer.tail_visual(10, 80);
        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn push_lines_respects_capacity() {
        let mut app = RunApp::new("run", "model", false, 3, false);
        for i in 0..10 {
            app.push_lines(vec![Line::raw(format!("Line {i}"))]);
        }
        let visible = app.ring_buffer.tail_visual(10, 80);
        assert_eq!(visible.len(), 3);
    }

    // ── update_status ───────────────────────────────────────────

    #[test]
    fn update_status_refreshes_elapsed() {
        let mut app = default_run_app();
        // Odczekaj minimalnie (test jest niemal natychmiastowy)
        app.update_status();

        assert!(app.status_data.elapsed_secs >= 0.0);
        assert!(app.header_data.elapsed.as_secs_f64() >= 0.0);
    }

    #[test]
    fn update_status_includes_hints() {
        let mut app = default_run_app();
        app.update_status();

        assert!(!app.status_data.hints.is_empty());
        // Powinny zawierać "Quit"
        let has_quit = app.status_data.hints.iter().any(|(_, d)| *d == "Quit");
        assert!(has_quit);
    }

    // ── handle_event: global keys ───────────────────────────────

    #[test]
    fn tab_toggles_focus() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(app.focus, FocusArea::Output);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Tab)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focus, FocusArea::Sidebar);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Tab)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focus, FocusArea::Output);
    }

    #[test]
    fn t_toggles_sidebar_visibility() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        assert!(app.sidebar.visible);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Char('t'))), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert!(!app.sidebar.visible);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Char('t'))), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert!(app.sidebar.visible);
    }

    #[test]
    fn bracket_keys_resize_sidebar() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        let initial_width = app.sidebar.width();

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Char(']'))), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.sidebar.width(), initial_width + 1);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Char('['))), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.sidebar.width(), initial_width);
    }

    // ── handle_event: output focus keys ─────────────────────────

    #[test]
    fn arrow_up_scrolls_output() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        // Dodaj trochę contentu
        for i in 0..20 {
            app.push_lines(vec![Line::raw(format!("Line {i}"))]);
        }
        assert!(app.output_view_state.auto_follow);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Up)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert!(!app.output_view_state.auto_follow);
        // scroll_step domyślnie = 3
        assert_eq!(app.output_view_state.scroll_offset, 3);
    }

    #[test]
    fn arrow_down_scrolls_output_back() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        for i in 0..20 {
            app.push_lines(vec![Line::raw(format!("Line {i}"))]);
        }
        // scroll_up(6) → offset=6, potem Down (scroll_step=3) → offset=3
        app.output_view_state.scroll_up(6);
        assert_eq!(app.output_view_state.scroll_offset, 6);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Down)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.output_view_state.scroll_offset, 3);
    }

    #[test]
    fn end_key_re_enables_auto_follow() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        app.output_view_state.scroll_up(5);
        assert!(!app.output_view_state.auto_follow);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::End)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert!(app.output_view_state.auto_follow);
        assert_eq!(app.output_view_state.scroll_offset, 0);
    }

    #[test]
    fn page_up_scrolls_by_10() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        for i in 0..50 {
            app.push_lines(vec![Line::raw(format!("Line {i}"))]);
        }

        let result = app.handle_event(AppEvent::Key(key(KeyCode::PageUp)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        // PageUp = scroll_step * 3 = 3 * 3 = 9
        assert_eq!(app.output_view_state.scroll_offset, 9);
    }

    // ── handle_event: sidebar focus keys ────────────────────────

    #[test]
    fn sidebar_up_down_navigates() {
        let mut app = default_run_app();
        app.focus = FocusArea::Sidebar;

        // Załaduj taski do sidebar
        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"
tasks:
  - id: "1"
    name: "Task A"
    status: todo
  - id: "2"
    name: "Task B"
    status: done
"#,
        )
        .unwrap();
        app.sidebar.refresh(&tf);

        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(app.sidebar.selected_index, 0);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Down)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.sidebar.selected_index, 1);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Up)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.sidebar.selected_index, 0);
    }

    #[test]
    fn sidebar_enter_toggles_expand() {
        let mut app = default_run_app();
        app.focus = FocusArea::Sidebar;

        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"
tasks:
  - id: "1"
    name: "Epic"
    subtasks:
      - id: "1.1"
        name: "Sub"
        status: todo
"#,
        )
        .unwrap();
        app.sidebar.refresh(&tf);

        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(AppEvent::Key(key(KeyCode::Enter)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        // Po expand powinno być 2 widoczne wiersze
    }

    // ── handle_event: tick ──────────────────────────────────────

    #[test]
    fn tick_updates_status() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(AppEvent::Tick, &resolver);
        assert_eq!(result, EventResult::Consumed);
        // elapsed_secs powinien być >= 0
        assert!(app.status_data.elapsed_secs >= 0.0);
    }

    // ── handle_event: resize ────────────────────────────────────

    #[test]
    fn resize_is_consumed() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(AppEvent::Resize(120, 40), &resolver);
        assert_eq!(result, EventResult::Consumed);
    }

    #[test]
    fn resize_updates_breakpoint_large() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        app.current_breakpoint = Breakpoint::Small;
        let result = app.handle_event(AppEvent::Resize(120, 40), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.current_breakpoint, Breakpoint::Large);
    }

    #[test]
    fn resize_updates_breakpoint_medium() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        app.current_breakpoint = Breakpoint::Large;
        let result = app.handle_event(AppEvent::Resize(100, 30), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.current_breakpoint, Breakpoint::Medium);
    }

    #[test]
    fn resize_updates_breakpoint_small() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        app.current_breakpoint = Breakpoint::Large;
        let result = app.handle_event(AppEvent::Resize(70, 20), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.current_breakpoint, Breakpoint::Small);
    }

    #[test]
    fn resize_preserves_breakpoint_if_same_range() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        app.current_breakpoint = Breakpoint::Large;
        let result = app.handle_event(AppEvent::Resize(150, 40), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.current_breakpoint, Breakpoint::Large);
    }

    #[test]
    fn resize_to_small_preserves_sidebar_focus() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        app.focus = FocusArea::Sidebar;
        app.handle_event(AppEvent::Resize(70, 20), &resolver);
        // Small breakpoint — sidebar jest overlay, focus zachowany
        assert_eq!(app.focus, FocusArea::Sidebar);
        assert_eq!(app.current_breakpoint, Breakpoint::Small);
    }

    #[test]
    fn resize_to_medium_preserves_sidebar_focus() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        app.focus = FocusArea::Sidebar;
        app.handle_event(AppEvent::Resize(100, 30), &resolver);
        // Medium nadal ma sidebar (collapsed) — focus zachowany
        assert_eq!(app.focus, FocusArea::Sidebar);
        assert_eq!(app.current_breakpoint, Breakpoint::Medium);
    }

    // ── handle_event: ctrl+c shutdown ───────────────────────────

    #[test]
    fn ctrl_c_triggers_shutdown() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(
            AppEvent::Key(key_with_mod(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            &resolver,
        );
        assert_eq!(result, EventResult::Shutdown);
        assert!(app.shutdown.load(Ordering::SeqCst));
    }

    // ── handle_event: quit confirmation flow ────────────────────

    #[test]
    fn first_q_enters_quit_pending() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        assert!(!app.quit_pending);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert!(app.quit_pending);
    }

    #[test]
    fn second_q_confirms_quit() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        // Pierwszy 'q'
        app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert!(app.quit_pending);

        // Drugi 'q'
        let result = app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert_eq!(result, EventResult::Quit);
    }

    #[test]
    fn enter_confirms_quit_when_pending() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        // Pierwszy 'q'
        app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert!(app.quit_pending);

        // Enter
        let result = app.handle_event(AppEvent::Key(key(KeyCode::Enter)), &resolver);
        assert_eq!(result, EventResult::Quit);
    }

    #[test]
    fn esc_cancels_quit_pending() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        // Pierwszy 'q'
        app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert!(app.quit_pending);

        // Esc anuluje
        let result = app.handle_event(AppEvent::Key(key(KeyCode::Esc)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert!(!app.quit_pending);
    }

    #[test]
    fn esc_ignored_when_not_quit_pending() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        assert!(!app.quit_pending);

        let result = app.handle_event(AppEvent::Key(key(KeyCode::Esc)), &resolver);
        assert_eq!(result, EventResult::Ignored);
    }

    #[test]
    fn navigation_cancels_quit_pending() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        // Wejdź w quit_pending
        app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert!(app.quit_pending);

        // Strzałka w górę anuluje quit_pending
        app.handle_event(AppEvent::Key(key(KeyCode::Up)), &resolver);
        assert!(!app.quit_pending);
    }

    #[test]
    fn sidebar_toggle_cancels_quit_pending() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert!(app.quit_pending);

        app.handle_event(AppEvent::Key(key(KeyCode::Char('t'))), &resolver);
        assert!(!app.quit_pending);
    }

    #[test]
    fn tab_cancels_quit_pending() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert!(app.quit_pending);

        app.handle_event(AppEvent::Key(key(KeyCode::Tab)), &resolver);
        assert!(!app.quit_pending);
    }

    #[test]
    fn sidebar_enter_ignored_when_quit_pending() {
        let mut app = default_run_app();
        app.focus = FocusArea::Sidebar;

        // Załaduj taski
        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"
tasks:
  - id: "1"
    name: "Epic"
    subtasks:
      - id: "1.1"
        name: "Sub"
        status: todo
"#,
        )
        .unwrap();
        app.sidebar.refresh(&tf);

        let resolver = KeybindingResolver::with_defaults();
        // Wejdź w quit_pending
        app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert!(app.quit_pending);

        // Enter powinien potwierdzić quit, nie expand sidebar
        let result = app.handle_event(AppEvent::Key(key(KeyCode::Enter)), &resolver);
        assert_eq!(result, EventResult::Quit);
    }

    #[test]
    fn keybinding_hints_change_when_quit_pending() {
        let mut app = default_run_app();

        // Normalne hinty
        let hints_normal = app.keybinding_hints();
        let has_quit = hints_normal.iter().any(|(k, _)| *k == "q");
        assert!(has_quit);

        // Wejdź w quit_pending
        app.quit_pending = true;
        let hints_pending = app.keybinding_hints();

        // Powinny zawierać "Confirm" i "Cancel"
        let has_confirm = hints_pending.iter().any(|(_, d)| *d == "Confirm");
        let has_cancel = hints_pending.iter().any(|(_, d)| *d == "Cancel");
        assert!(has_confirm);
        assert!(has_cancel);
    }

    // ── handle_event: unknown key ignored ───────────────────────

    #[test]
    fn unknown_key_is_ignored() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(AppEvent::Key(key(KeyCode::F(12))), &resolver);
        assert_eq!(result, EventResult::Ignored);
    }

    #[test]
    fn unknown_key_preserves_quit_pending() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        // Wejdź w quit_pending
        app.handle_event(AppEvent::Key(key(KeyCode::Char('q'))), &resolver);
        assert!(app.quit_pending);

        // Nieznany klawisz — ignorowany, ale quit_pending NIE jest anulowane
        let result = app.handle_event(AppEvent::Key(key(KeyCode::F(12))), &resolver);
        assert_eq!(result, EventResult::Ignored);
        assert!(app.quit_pending);
    }

    // ── keybinding_hints ────────────────────────────────────────

    #[test]
    fn hints_differ_by_focus() {
        let mut app = default_run_app();

        app.focus = FocusArea::Output;
        let hints_output = app.keybinding_hints();

        app.focus = FocusArea::Sidebar;
        let hints_sidebar = app.keybinding_hints();

        // Output focus shows "Scroll", sidebar shows "Navigate"
        let output_has_scroll = hints_output.iter().any(|(_, d)| *d == "Scroll");
        let sidebar_has_navigate = hints_sidebar.iter().any(|(_, d)| *d == "Navigate");
        assert!(output_has_scroll);
        assert!(sidebar_has_navigate);
    }

    // ── Snapshot: draw renders without panic ────────────────────

    #[test]
    fn draw_renders_large_layout() {
        let mut app = default_run_app();
        for i in 0..5 {
            app.push_lines(vec![Line::raw(format!("Output line {i}"))]);
        }
        app.update_status();

        let backend = TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");
    }

    #[test]
    fn draw_renders_medium_layout() {
        let mut app = default_run_app();
        for i in 0..5 {
            app.push_lines(vec![Line::raw(format!("Output line {i}"))]);
        }

        let backend = TestBackend::new(100, 20);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");
    }

    #[test]
    fn draw_renders_small_layout() {
        let mut app = default_run_app();
        for i in 0..5 {
            app.push_lines(vec![Line::raw(format!("Output line {i}"))]);
        }

        let backend = TestBackend::new(60, 15);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");
    }

    // ── Layout integration tests ─────────────────────────────────
    // Breakpoint/LayoutAreas unit tests → src/tui/responsive.rs
    // Tu testujemy integrację RunApp.draw() z systemem responsywnym

    /// Hidden sidebar w Large breakpoint — output zajmuje pełną szerokość
    #[test]
    fn layout_large_sidebar_hidden() {
        let mut app = default_run_app();
        app.sidebar.toggle_visible();
        app.push_lines(vec![Line::raw("Test output")]);

        let area = Rect::new(0, 0, 120, 30);
        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        // Sidebar hidden → output = sidebar_area(24) + content(96) = 120
        assert_eq!(app.last_output_area.width, 120);
        assert_eq!(app.last_output_area.height, 27); // 30 - 1 (header) - 2 (status)
    }

    /// Resize Large → Small przelicza layout (last_output_area się zmienia)
    #[test]
    fn layout_responds_to_resize() {
        let mut app = default_run_app();
        app.push_lines(vec![Line::raw("Test")]);

        let large_area = Rect::new(0, 0, 120, 30);
        let small_area = Rect::new(0, 0, 60, 15);

        let backend_large = TestBackend::new(120, 30);
        let mut terminal_large = ratatui::Terminal::new(backend_large).unwrap();
        terminal_large
            .draw(|frame| {
                app.draw(frame, large_area);
            })
            .unwrap();
        let large_cached = app.last_output_area;

        let backend_small = TestBackend::new(60, 15);
        let mut terminal_small = ratatui::Terminal::new(backend_small).unwrap();
        terminal_small
            .draw(|frame| {
                app.draw(frame, small_area);
            })
            .unwrap();
        let small_cached = app.last_output_area;

        assert!(large_cached.width > small_cached.width);
        // Small: pełna szerokość (60), Large: content po odjęciu sidebar
        assert_eq!(small_cached.width, 60);
    }

    #[test]
    fn draw_with_sidebar_hidden() {
        let mut app = default_run_app();
        app.sidebar.toggle_visible();
        for i in 0..5 {
            app.push_lines(vec![Line::raw(format!("Line {i}"))]);
        }

        let backend = TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw with hidden sidebar should not panic");
    }

    #[test]
    fn draw_empty_buffer() {
        let mut app = default_run_app();

        let backend = TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw empty buffer should not panic");
    }

    #[test]
    fn draw_caches_last_output_area() {
        let mut app = default_run_app();
        assert_eq!(app.last_output_area, Rect::default());

        let backend = TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        // Po draw(), last_output_area powinien mieć niezerowe wymiary
        assert!(app.last_output_area.width > 0);
        assert!(app.last_output_area.height > 0);
    }

    #[test]
    fn draw_caches_output_rect() {
        let mut app = default_run_app();
        assert!(app.output_rect.is_none());

        let backend = TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        // Po draw(), output_rect powinien być ustawiony z niezerowymi wymiarami
        let output_rect = app
            .output_rect
            .expect("output_rect should be Some after draw");
        assert!(output_rect.width > 0);
        assert!(output_rect.height > 0);
    }

    #[test]
    fn draw_caches_sidebar_rect_when_sidebar_visible() {
        let mut app = default_run_app();
        // Sidebar domyślnie widoczny
        assert!(app.sidebar.visible);
        assert!(app.sidebar_rect.is_none());

        // Large breakpoint (120+ kolumn) — sidebar powinien być widoczny
        let backend = TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        // Po draw() z widocznym sidebarem sidebar_rect powinien być Some
        assert!(app.sidebar_rect.is_some());
    }

    #[test]
    fn draw_sidebar_rect_none_when_sidebar_hidden() {
        let mut app = default_run_app();
        app.sidebar.visible = false;

        let backend = TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        // Sidebar ukryty — sidebar_rect powinien być None
        assert!(app.sidebar_rect.is_none());
        // Ale output_rect powinien być ustawiony
        assert!(app.output_rect.is_some());
    }

    #[test]
    fn draw_sidebar_rect_none_on_small_breakpoint() {
        let mut app = default_run_app();

        // Small breakpoint (poniżej 80 kolumn) — sidebar jako overlay, sidebar_rect = None
        let backend = TestBackend::new(60, 20);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        assert!(app.sidebar_rect.is_none());
        assert!(app.output_rect.is_some());
    }

    #[test]
    fn home_key_uses_cached_output_area() {
        let mut app = default_run_app();
        for i in 0..50 {
            app.push_lines(vec![Line::raw(format!("Line {i}"))]);
        }

        // Symuluj draw() aby wypełnić last_output_area
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        let cached_area = app.last_output_area;
        assert!(cached_area.width > 0);

        let resolver = KeybindingResolver::with_defaults();
        // Home powinien ustawić scroll_offset na max (top)
        let result = app.handle_event(AppEvent::Key(key(KeyCode::Home)), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert!(!app.output_view_state.auto_follow);
        assert!(app.output_view_state.scroll_offset > 0);
    }

    #[test]
    fn draw_zero_area() {
        let mut app = default_run_app();

        let backend = TestBackend::new(1, 1);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw zero area should not panic");
    }

    // ── Mouse handling tests (task 13.4.3) ──────────────────────────

    /// Helper: tworzy MouseEvent dla testów.
    fn make_mouse_click(col: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    /// Pomocnik tworzący MouseEvent innego rodzaju (nie Left click).
    fn make_mouse_scroll_up(col: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::ScrollUp,
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    /// Left click w output_rect → focus zmienia się na Output.
    #[test]
    fn mouse_left_click_in_output_rect_sets_focus_to_output() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        // Ustaw focus na Sidebar i skonfiguruj output_rect
        app.focus = FocusArea::Sidebar;
        app.output_rect = Some(Rect::new(0, 2, 60, 20));

        // Click wewnątrz output_rect (5, 5)
        let mouse = make_mouse_click(5, 5);
        let result = app.handle_event(AppEvent::Mouse(mouse), &resolver);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focus, FocusArea::Output);
    }

    /// Left click poza output_rect → focus bez zmian.
    #[test]
    fn mouse_left_click_outside_output_rect_ignores_focus() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        // Ustaw focus na Sidebar i skonfiguruj output_rect
        app.focus = FocusArea::Sidebar;
        app.output_rect = Some(Rect::new(0, 2, 60, 20));

        // Click poza output_rect (70, 5) — poza zakresem szerokości
        let mouse = make_mouse_click(70, 5);
        let result = app.handle_event(AppEvent::Mouse(mouse), &resolver);

        assert_eq!(result, EventResult::Ignored);
        assert_eq!(
            app.focus,
            FocusArea::Sidebar,
            "Focus nie powinien zmienić się przy kliku poza output_rect"
        );
    }

    /// Gdy output_rect jest None → mouse click nie zmienia focusu.
    #[test]
    fn mouse_left_click_without_output_rect_is_ignored() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        // output_rect jest None (np. przed pierwszym draw)
        app.focus = FocusArea::Sidebar;
        assert!(app.output_rect.is_none());

        let mouse = make_mouse_click(10, 10);
        let result = app.handle_event(AppEvent::Mouse(mouse), &resolver);

        assert_eq!(result, EventResult::Ignored);
        assert_eq!(app.focus, FocusArea::Sidebar);
    }

    /// ScrollUp nad output_rect → obsługuje scroll output buffera (Consumed).
    ///
    /// handle_mouse obsługuje ScrollUp/ScrollDown od task 13.4.4 —
    /// ten test weryfikuje że kursor nad output_rect zwraca Consumed.
    #[test]
    fn mouse_scroll_up_in_output_rect_is_consumed() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        app.focus = FocusArea::Sidebar;
        app.output_rect = Some(Rect::new(0, 2, 60, 20));

        // Scroll w obszarze output_rect — obsługiwany (scroll output buffera)
        let mouse = make_mouse_scroll_up(5, 5);
        let result = app.handle_event(AppEvent::Mouse(mouse), &resolver);

        assert_eq!(result, EventResult::Consumed);
        // Focus nie zmienia się przy scroll (tylko klik zmienia focus)
        assert_eq!(app.focus, FocusArea::Sidebar);
    }

    /// Left click w output_rect gdy quit_pending → anuluje quit_pending i ustawia focus Output.
    #[test]
    fn mouse_left_click_in_output_rect_cancels_quit_pending() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        app.focus = FocusArea::Sidebar;
        app.quit_pending = true;
        app.output_rect = Some(Rect::new(0, 2, 60, 20));

        let mouse = make_mouse_click(5, 5);
        let result = app.handle_event(AppEvent::Mouse(mouse), &resolver);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focus, FocusArea::Output);
        assert!(
            !app.quit_pending,
            "Mouse click powinien anulować quit_pending"
        );
    }

    /// Left click gdy focus już Output → focus pozostaje Output, wynik Consumed.
    #[test]
    fn mouse_left_click_in_output_rect_when_already_focused_stays_output() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        // Focus już Output, output_rect ustawiony
        assert_eq!(app.focus, FocusArea::Output);
        app.output_rect = Some(Rect::new(0, 2, 60, 20));

        let mouse = make_mouse_click(5, 5);
        let result = app.handle_event(AppEvent::Mouse(mouse), &resolver);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focus, FocusArea::Output);
    }

    // ── Snapshot tests: run layout w różnych breakpointach ──────────

    /// Snapshot test: Large breakpoint (120x40)
    ///
    /// Weryfikuje pełny layout: header + sidebar + output + status bar.
    /// Large breakpoint powinien mieć wszystkie elementy widoczne.
    #[test]
    fn snapshot_run_layout_large() {
        let mut app = default_run_app();

        // Setup: dodaj przykładowe dane
        app.header_data.iteration = Some(2);
        app.header_data.max_iterations = Some(10);
        app.header_data.is_running = true;

        for i in 0..10 {
            app.push_lines(vec![Line::raw(format!("Output line {i}"))]);
        }

        app.update_status();

        // Renderuj w Large breakpoint (120x40)
        let backend = TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        // Snapshot buffer jako string (bez testowania wymiarów - już przetestowane unit)
        let buffer = terminal.backend().buffer();
        let snapshot = render_buffer_snapshot(buffer);
        insta::assert_snapshot!(snapshot);
    }

    /// Snapshot test: Medium breakpoint (100x30)
    ///
    /// Weryfikuje layout bez header, z collapsed sidebar.
    #[test]
    fn snapshot_run_layout_medium() {
        let mut app = default_run_app();

        // Setup: dodaj przykładowe dane
        app.header_data.iteration = Some(3);
        app.header_data.max_iterations = Some(10);
        app.header_data.is_running = true;

        for i in 0..8 {
            app.push_lines(vec![Line::raw(format!("Medium output {i}"))]);
        }

        app.update_status();

        // Renderuj w Medium breakpoint (100x30)
        let backend = TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        let buffer = terminal.backend().buffer();
        let snapshot = render_buffer_snapshot(buffer);
        insta::assert_snapshot!(snapshot);
    }

    /// Snapshot test: Small breakpoint (60x24)
    ///
    /// Weryfikuje minimal layout: tylko output + status bar (bez header i sidebar).
    #[test]
    fn snapshot_run_layout_small() {
        let mut app = default_run_app();

        // Setup: dodaj przykładowe dane
        app.header_data.iteration = Some(1);
        app.header_data.is_running = true;

        for i in 0..5 {
            app.push_lines(vec![Line::raw(format!("Small {i}"))]);
        }

        app.update_status();

        // Renderuj w Small breakpoint (60x24)
        let backend = TestBackend::new(60, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw should not panic");

        let buffer = terminal.backend().buffer();
        let snapshot = render_buffer_snapshot(buffer);
        insta::assert_snapshot!(snapshot);
    }

    /// Helper: konwertuje Buffer do string snapshot (redukuje whitespace dla lepszej czytelności)
    fn render_buffer_snapshot(buffer: &ratatui::buffer::Buffer) -> String {
        (0..buffer.area().height)
            .map(|y| {
                let mut line = String::new();
                for x in 0..buffer.area().width {
                    if let Some(cell) = buffer.cell((x, y)) {
                        line.push_str(cell.symbol());
                    }
                }
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ── Splash screen tests ─────────────────────────────────────────

    #[test]
    fn splash_screen_keyboard_skip() {
        let mut app = RunApp::new("run", "claude-sonnet-4-5", false, 100, true);
        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(app.phase, RunPhase::Splash);

        // Każdy klawisz powinien pominąć splash
        let result = app.handle_event(AppEvent::Key(key(KeyCode::Char('a'))), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.phase, RunPhase::Running);
        assert_eq!(app.splash_start_time, None);
    }

    #[test]
    fn splash_screen_timer_transition() {
        let mut app = RunApp::new("run", "claude-sonnet-4-5", false, 100, true);
        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(app.phase, RunPhase::Splash);

        // Simulate tick events (splash trwa 1500ms)
        for _ in 0..20 {
            let result = app.handle_event(AppEvent::Tick, &resolver);
            assert_eq!(result, EventResult::Consumed);

            if app.phase == RunPhase::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // Po ~1500ms splash powinien się skończyć
        assert_eq!(app.phase, RunPhase::Running);
        assert_eq!(app.splash_start_time, None);
    }

    #[test]
    fn splash_screen_no_status_update_during_splash() {
        let mut app = RunApp::new("run", "claude-sonnet-4-5", false, 100, true);
        let initial_elapsed = app.status_data.elapsed_secs;

        let resolver = KeybindingResolver::with_defaults();
        // Tick podczas splash — status nie powinien być aktualizowany
        app.handle_event(AppEvent::Tick, &resolver);

        // elapsed_secs powinien być taki sam
        assert_eq!(app.status_data.elapsed_secs, initial_elapsed);
    }

    #[test]
    fn draw_splash_screen() {
        let mut app = RunApp::new("run", "claude-sonnet-4-5", false, 100, true);
        assert_eq!(app.phase, RunPhase::Splash);

        let backend = TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw splash should not panic");
    }

    #[test]
    fn draw_splash_then_running() {
        let mut app = RunApp::new("run", "claude-sonnet-4-5", false, 100, true);

        // Renderuj splash
        let backend1 = TestBackend::new(80, 24);
        let mut terminal1 = ratatui::Terminal::new(backend1).expect("test terminal");
        terminal1
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw splash should not panic");

        // Przejdź do Running
        app.phase = RunPhase::Running;
        app.push_lines(vec![Line::raw("Output after splash")]);

        // Renderuj main layout
        let backend2 = TestBackend::new(80, 24);
        let mut terminal2 = ratatui::Terminal::new(backend2).expect("test terminal");
        terminal2
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .expect("draw running should not panic");
    }

    // ── Auto-refresh sidebar tests (Task 3.6) ───────────────────

    /// Helper: tworzy tasks.yml w temp dir
    fn create_test_tasks_yml(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("tasks.yml");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn set_tasks_file_loads_initial_state() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = r#"
tasks:
  - id: "1"
    name: "Test task"
    status: todo
"#;
        let path = create_test_tasks_yml(tmp.path(), yaml);

        let mut app = default_run_app();
        app.set_tasks_file(path.clone());

        // last_tasks_mtime powinien być ustawiony (plik istnieje)
        assert!(app.last_tasks_mtime.is_some());
        // tasks_file_path powinien być ustawiony
        assert_eq!(app.tasks_file_path, Some(path));
    }

    #[test]
    fn set_tasks_file_nonexistent_path_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.yml");

        let mut app = default_run_app();
        app.set_tasks_file(path.clone());

        // last_tasks_mtime nie powinien być ustawiony (plik nie istnieje)
        assert!(app.last_tasks_mtime.is_none());
        // tasks_file_path ustawiony mimo braku pliku (dla przyszłego reload)
        assert_eq!(app.tasks_file_path, Some(path));
    }

    #[test]
    fn check_tasks_reload_does_nothing_when_no_file_path() {
        let mut app = default_run_app();
        // Brak tasks_file_path — check_tasks_reload powinien być noop
        app.check_tasks_reload();
        assert!(app.last_tasks_mtime.is_none());
    }

    #[test]
    fn check_tasks_reload_throttle_2s() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = r#"
tasks:
  - id: "1"
    name: "Initial"
    status: todo
"#;
        let path = create_test_tasks_yml(tmp.path(), yaml);

        let mut app = default_run_app();
        app.set_tasks_file(path.clone());

        // Zapisz początkowy mtime
        let initial_mtime = app.last_tasks_mtime;

        // Wywołaj check_tasks_reload natychmiast (< 2s) — powinien być throttle
        app.check_tasks_reload();

        // Mtime powinien pozostać bez zmian (throttle)
        assert_eq!(app.last_tasks_mtime, initial_mtime);
    }

    #[test]
    fn check_tasks_reload_detects_file_change() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml1 = r#"
tasks:
  - id: "1"
    name: "Version 1"
    status: todo
"#;
        let path = create_test_tasks_yml(tmp.path(), yaml1);

        let mut app = default_run_app();
        app.set_tasks_file(path.clone());

        let initial_mtime = app.last_tasks_mtime;

        // Wymuś wstecz last_tasks_check o 3s (żeby throttle przepuścił)
        app.last_tasks_check = Instant::now() - std::time::Duration::from_secs(3);

        // Zmień plik (sleep krótko żeby mtime się zmienił)
        std::thread::sleep(std::time::Duration::from_millis(10));
        let yaml2 = r#"
tasks:
  - id: "1"
    name: "Version 1"
    status: done
  - id: "2"
    name: "Version 2"
    status: todo
"#;
        std::fs::write(&path, yaml2).unwrap();

        // Wywołaj check_tasks_reload
        app.check_tasks_reload();

        // Mtime powinien się zmienić (plik został zmieniony)
        assert_ne!(app.last_tasks_mtime, initial_mtime);
    }

    #[test]
    fn check_tasks_reload_no_change_when_mtime_same() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = r#"
tasks:
  - id: "1"
    name: "Unchanged"
    status: todo
"#;
        let path = create_test_tasks_yml(tmp.path(), yaml);

        let mut app = default_run_app();
        app.set_tasks_file(path.clone());

        let initial_mtime = app.last_tasks_mtime;

        // Wymuś wstecz last_tasks_check o 3s (throttle przepuści)
        app.last_tasks_check = Instant::now() - std::time::Duration::from_secs(3);

        // Wywołaj check_tasks_reload bez zmiany pliku
        app.check_tasks_reload();

        // Mtime powinien pozostać taki sam (brak reload)
        assert_eq!(app.last_tasks_mtime, initial_mtime);
    }

    #[test]
    fn tick_event_triggers_check_tasks_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = r#"
tasks:
  - id: "1"
    name: "Initial"
    status: todo
"#;
        let path = create_test_tasks_yml(tmp.path(), yaml);

        let mut app = default_run_app();
        app.set_tasks_file(path.clone());

        let initial_mtime = app.last_tasks_mtime;

        // Wymuś wstecz last_tasks_check o 3s
        app.last_tasks_check = Instant::now() - std::time::Duration::from_secs(3);

        // Zmień plik
        std::thread::sleep(std::time::Duration::from_millis(10));
        let yaml2 = r#"
tasks:
  - id: "1"
    name: "Updated"
    status: done
"#;
        std::fs::write(&path, yaml2).unwrap();

        let resolver = KeybindingResolver::with_defaults();
        // Wywołaj handle_event(Tick) — powinien wywołać check_tasks_reload
        let result = app.handle_event(AppEvent::Tick, &resolver);
        assert_eq!(result, EventResult::Consumed);

        // Mtime powinien się zmienić (reload przez Tick event)
        assert_ne!(app.last_tasks_mtime, initial_mtime);
    }

    #[test]
    fn check_tasks_reload_file_deleted_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = r#"
tasks:
  - id: "1"
    name: "Will be deleted"
    status: todo
"#;
        let path = create_test_tasks_yml(tmp.path(), yaml);

        let mut app = default_run_app();
        app.set_tasks_file(path.clone());

        let initial_mtime = app.last_tasks_mtime;

        // Usuń plik
        std::fs::remove_file(&path).unwrap();

        // Wymuś wstecz last_tasks_check
        app.last_tasks_check = Instant::now() - std::time::Duration::from_secs(3);

        // check_tasks_reload powinien nie panikować (metadata() zwraca Err)
        app.check_tasks_reload();

        // Mtime nie powinien się zmienić (plik nie istnieje → early return)
        assert_eq!(app.last_tasks_mtime, initial_mtime);
    }

    // ── Resize integration tests ────────────────────────────────

    /// Test: Large → Medium resize → layout się zmienia
    #[test]
    fn resize_large_to_medium_changes_layout() {
        let mut app = default_run_app();
        app.push_lines(vec![Line::raw("Test output")]);

        let resolver = KeybindingResolver::with_defaults();
        // Start w Large (120x30)
        app.handle_event(AppEvent::Resize(120, 30), &resolver);
        let backend_large = TestBackend::new(120, 30);
        let mut terminal_large = ratatui::Terminal::new(backend_large).unwrap();
        terminal_large
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .unwrap();
        let large_output_width = app.last_output_area.width;

        // Resize do Medium (100x30)
        app.handle_event(AppEvent::Resize(100, 30), &resolver);
        let backend_medium = TestBackend::new(100, 30);
        let mut terminal_medium = ratatui::Terminal::new(backend_medium).unwrap();
        terminal_medium
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .unwrap();
        let medium_output_width = app.last_output_area.width;

        // Medium ma węższą sidebar (3 vs 24), więc output powinien być szerszy
        assert!(medium_output_width > large_output_width);
        assert_eq!(app.current_breakpoint, Breakpoint::Medium);
    }

    /// Test: Medium → Small resize → sidebar znika
    #[test]
    fn resize_medium_to_small_removes_sidebar() {
        let mut app = default_run_app();
        app.push_lines(vec![Line::raw("Test")]);

        let resolver = KeybindingResolver::with_defaults();
        // Start w Medium (100x30)
        app.handle_event(AppEvent::Resize(100, 30), &resolver);
        let backend_medium = TestBackend::new(100, 30);
        let mut terminal_medium = ratatui::Terminal::new(backend_medium).unwrap();
        terminal_medium
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .unwrap();
        let medium_output_width = app.last_output_area.width;

        // Resize do Small (70x20)
        app.handle_event(AppEvent::Resize(70, 20), &resolver);
        let backend_small = TestBackend::new(70, 20);
        let mut terminal_small = ratatui::Terminal::new(backend_small).unwrap();
        terminal_small
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .unwrap();
        let small_output_width = app.last_output_area.width;

        // Small ma pełną szerokość (70), Medium ma output - sidebar (97)
        assert_eq!(small_output_width, 70);
        assert!(small_output_width < medium_output_width);
        assert_eq!(app.current_breakpoint, Breakpoint::Small);
    }

    /// Test: Small → Large resize → sidebar i header się pojawiają
    #[test]
    fn resize_small_to_large_adds_sidebar_and_header() {
        let mut app = default_run_app();
        app.push_lines(vec![Line::raw("Test")]);

        let resolver = KeybindingResolver::with_defaults();
        // Start w Small (70x20)
        app.handle_event(AppEvent::Resize(70, 20), &resolver);
        let backend_small = TestBackend::new(70, 20);
        let mut terminal_small = ratatui::Terminal::new(backend_small).unwrap();
        terminal_small
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .unwrap();
        assert_eq!(app.current_breakpoint, Breakpoint::Small);

        // Resize do Large (120x30)
        app.handle_event(AppEvent::Resize(120, 30), &resolver);
        let backend_large = TestBackend::new(120, 30);
        let mut terminal_large = ratatui::Terminal::new(backend_large).unwrap();
        terminal_large
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .unwrap();

        assert_eq!(app.current_breakpoint, Breakpoint::Large);
        // Large layout ma sidebar (24 cols) → output jest węższy niż 120
        assert!(app.last_output_area.width < 120);
    }

    /// Test: Multiple resizes — breakpoint śledzi zmiany
    #[test]
    fn multiple_resizes_track_breakpoint() {
        let mut app = default_run_app();
        let resolver = KeybindingResolver::with_defaults();

        app.handle_event(AppEvent::Resize(120, 30), &resolver);
        assert_eq!(app.current_breakpoint, Breakpoint::Large);

        app.handle_event(AppEvent::Resize(100, 30), &resolver);
        assert_eq!(app.current_breakpoint, Breakpoint::Medium);

        app.handle_event(AppEvent::Resize(70, 20), &resolver);
        assert_eq!(app.current_breakpoint, Breakpoint::Small);

        app.handle_event(AppEvent::Resize(80, 25), &resolver);
        assert_eq!(app.current_breakpoint, Breakpoint::Medium);

        app.handle_event(AppEvent::Resize(150, 40), &resolver);
        assert_eq!(app.current_breakpoint, Breakpoint::Large);
    }

    /// Test: Breakpoint consistency — draw() sync with handle_event
    #[test]
    fn draw_syncs_breakpoint_with_area() {
        let mut app = default_run_app();
        app.current_breakpoint = Breakpoint::Large;

        // draw() z Small area → current_breakpoint powinien się zaktualizować
        let backend = TestBackend::new(70, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.draw(frame, area);
            })
            .unwrap();

        assert_eq!(app.current_breakpoint, Breakpoint::Small);
    }

    // ── Integration tests: key sequences → state verification ──────

    /// Integration test: q → quit confirm → q → shutdown
    ///
    /// Weryfikuje quit confirmation flow:
    /// 1. Pierwszy 'q' wchodzi w quit_pending
    /// 2. Drugi 'q' potwierdza i zwraca EventResult::Quit
    #[test]
    fn integration_quit_confirmation_flow() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 80, 24);

        // Stan początkowy: quit_pending = false
        app.assert_state(|s| !s.quit_pending);

        // Krok 1: Pierwszy 'q' → quit_pending = true
        app.inject_key(make_key(KeyCode::Char('q')));
        app.step();
        app.assert_state(|s| s.quit_pending);

        // Krok 2: Drugi 'q' → EventResult::Quit (nie możemy bezpośrednio asertować EventResult,
        // ale możemy sprawdzić że state nie zmienił quit_pending ponownie)
        app.inject_key(make_key(KeyCode::Char('q')));
        let result = app.step();
        assert!(result.is_some(), "Second 'q' should trigger quit");

        // Note: W prawdziwym App EventResult::Quit powoduje wyjście z event loop,
        // tu tylko weryfikujemy że event został przetworzony i quit_pending pozostał true
        // (bo drugi 'q' zwraca Quit bez zmiany state)
    }

    /// Integration test: q → quit confirm → Enter → shutdown
    ///
    /// Alternatywny sposób potwierdzenia quit przez Enter zamiast drugiego 'q'.
    #[test]
    fn integration_quit_confirmation_enter() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 80, 24);

        // Krok 1: Pierwszy 'q' → quit_pending
        app.inject_key(make_key(KeyCode::Char('q')));
        app.step();
        app.assert_state(|s| s.quit_pending);

        // Krok 2: Enter → potwierdza quit
        app.inject_key(make_key(KeyCode::Enter));
        let result = app.step();
        assert!(result.is_some(), "Enter should confirm quit");
    }

    /// Integration test: q → quit confirm → Esc → cancel
    ///
    /// Weryfikuje anulowanie quit przez Esc.
    #[test]
    fn integration_quit_cancel_with_esc() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 80, 24);

        // Krok 1: Pierwszy 'q' → quit_pending
        app.inject_key(make_key(KeyCode::Char('q')));
        app.step();
        app.assert_state(|s| s.quit_pending);

        // Krok 2: Esc → anuluje quit_pending
        app.inject_key(make_key(KeyCode::Esc));
        app.step();
        app.assert_state(|s| !s.quit_pending);
    }

    /// Integration test: t → sidebar toggle
    ///
    /// Weryfikuje toggling sidebar visibility przez klawisz 't'.
    #[test]
    fn integration_sidebar_toggle() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 80, 24);

        // Stan początkowy: sidebar visible = true
        app.assert_state(|s| s.sidebar.visible);

        // Krok 1: 't' → sidebar hidden
        app.inject_key(make_key(KeyCode::Char('t')));
        app.step();
        app.assert_state(|s| !s.sidebar.visible);

        // Krok 2: 't' ponownie → sidebar visible
        app.inject_key(make_key(KeyCode::Char('t')));
        app.step();
        app.assert_state(|s| s.sidebar.visible);

        // Krok 3: 't' trzeci raz → sidebar hidden
        app.inject_key(make_key(KeyCode::Char('t')));
        app.step();
        app.assert_state(|s| !s.sidebar.visible);
    }

    /// Integration test: ↑↓ → output scroll
    ///
    /// Weryfikuje scrollowanie output view przez strzałki góra/dół.
    #[test]
    fn integration_output_scroll_arrows() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 80, 24);

        // Setup: dodaj dużo contentu do scrollowania
        for i in 0..50 {
            app.state_mut()
                .push_lines(vec![Line::raw(format!("Line {i}"))]);
        }

        // Stan początkowy: auto_follow = true, scroll_offset = 0
        app.assert_state(|s| s.output_view_state.auto_follow);
        app.assert_state(|s| s.output_view_state.scroll_offset == 0);

        // Krok 1: Strzałka w górę → scroll_offset = 3 (scroll_step=3), auto_follow wyłączony
        app.inject_key(make_key(KeyCode::Up));
        app.step();
        app.assert_state(|s| !s.output_view_state.auto_follow);
        app.assert_state(|s| s.output_view_state.scroll_offset == 3);

        // Krok 2: Kolejne strzałki w górę → scroll_offset rośnie o scroll_step=3
        app.inject_key(make_key(KeyCode::Up));
        app.step();
        app.assert_state(|s| s.output_view_state.scroll_offset == 6);

        app.inject_key(make_key(KeyCode::Up));
        app.step();
        app.assert_state(|s| s.output_view_state.scroll_offset == 9);

        // Krok 3: Strzałka w dół → scroll_offset maleje o scroll_step=3
        app.inject_key(make_key(KeyCode::Down));
        app.step();
        app.assert_state(|s| s.output_view_state.scroll_offset == 6);

        app.inject_key(make_key(KeyCode::Down));
        app.step();
        app.assert_state(|s| s.output_view_state.scroll_offset == 3);

        // Krok 4: End → auto_follow włączony, scroll_offset = 0
        app.inject_key(make_key(KeyCode::End));
        app.step();
        app.assert_state(|s| s.output_view_state.auto_follow);
        app.assert_state(|s| s.output_view_state.scroll_offset == 0);
    }

    /// Integration test: PageUp/PageDown → output scroll duże kroki
    ///
    /// Weryfikuje scrollowanie przez PageUp/PageDown (po 10 linii).
    #[test]
    fn integration_output_scroll_page_keys() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 80, 24);

        // Setup: dodaj dużo contentu
        for i in 0..100 {
            app.state_mut()
                .push_lines(vec![Line::raw(format!("Line {i}"))]);
        }

        // Stan początkowy
        app.assert_state(|s| s.output_view_state.scroll_offset == 0);

        // Krok 1: PageUp → scroll_offset = 9 (scroll_step * 3 = 3 * 3 = 9)
        app.inject_key(make_key(KeyCode::PageUp));
        app.step();
        app.assert_state(|s| s.output_view_state.scroll_offset == 9);

        // Krok 2: Kolejny PageUp → scroll_offset = 18
        app.inject_key(make_key(KeyCode::PageUp));
        app.step();
        app.assert_state(|s| s.output_view_state.scroll_offset == 18);

        // Krok 3: PageDown → scroll_offset = 9
        app.inject_key(make_key(KeyCode::PageDown));
        app.step();
        app.assert_state(|s| s.output_view_state.scroll_offset == 9);
    }

    /// Integration test: Home → scroll to top
    ///
    /// Weryfikuje scrollowanie do początku bufora przez Home.
    #[test]
    fn integration_output_scroll_home() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 80, 24);

        // Setup: dodaj dużo contentu
        for i in 0..100 {
            app.state_mut()
                .push_lines(vec![Line::raw(format!("Line {i}"))]);
        }

        // Renderuj aby wypełnić last_output_area (Home używa cache'owanego rozmiaru)
        app.render();

        // Stan początkowy: auto_follow = true
        app.assert_state(|s| s.output_view_state.auto_follow);

        // Krok 1: Home → scroll do początku, auto_follow wyłączony
        app.inject_key(make_key(KeyCode::Home));
        app.step();
        app.assert_state(|s| !s.output_view_state.auto_follow);
        // scroll_offset powinien być > 0 (scrollujemy do góry)
        app.assert_state(|s| s.output_view_state.scroll_offset > 0);
    }

    /// Integration test: Tab → focus switch
    ///
    /// Weryfikuje przełączanie focus między Output i Sidebar przez Tab.
    #[test]
    fn integration_focus_switch_tab() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 80, 24);

        // Stan początkowy: focus = Output
        app.assert_state(|s| s.focus == FocusArea::Output);

        // Krok 1: Tab → focus = Sidebar
        app.inject_key(make_key(KeyCode::Tab));
        app.step();
        app.assert_state(|s| s.focus == FocusArea::Sidebar);

        // Krok 2: Tab ponownie → focus = Output
        app.inject_key(make_key(KeyCode::Tab));
        app.step();
        app.assert_state(|s| s.focus == FocusArea::Output);

        // Krok 3: Tab trzeci raz → focus = Sidebar
        app.inject_key(make_key(KeyCode::Tab));
        app.step();
        app.assert_state(|s| s.focus == FocusArea::Sidebar);
    }

    /// Integration test: Tab + arrow keys w Sidebar
    ///
    /// Weryfikuje że po przełączeniu focus do Sidebar, strzałki nawigują po sidebar.
    #[test]
    fn integration_focus_sidebar_navigation() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 80, 24);

        // Załaduj taski do sidebar
        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"
tasks:
  - id: "1"
    name: "Task A"
    status: todo
  - id: "2"
    name: "Task B"
    status: in_progress
  - id: "3"
    name: "Task C"
    status: done
"#,
        )
        .unwrap();
        app.state_mut().sidebar.refresh(&tf);

        // Stan początkowy: focus = Output, sidebar.selected_index = 0
        app.assert_state(|s| s.focus == FocusArea::Output);
        app.assert_state(|s| s.sidebar.selected_index == 0);

        // Krok 1: Tab → focus = Sidebar
        app.inject_key(make_key(KeyCode::Tab));
        app.step();
        app.assert_state(|s| s.focus == FocusArea::Sidebar);

        // Krok 2: Strzałka w dół → selected_index = 1
        app.inject_key(make_key(KeyCode::Down));
        app.step();
        app.assert_state(|s| s.sidebar.selected_index == 1);

        // Krok 3: Strzałka w dół → selected_index = 2
        app.inject_key(make_key(KeyCode::Down));
        app.step();
        app.assert_state(|s| s.sidebar.selected_index == 2);

        // Krok 4: Strzałka w górę → selected_index = 1
        app.inject_key(make_key(KeyCode::Up));
        app.step();
        app.assert_state(|s| s.sidebar.selected_index == 1);
    }

    /// Integration test: Complex key sequence
    ///
    /// Weryfikuje złożoną sekwencję: toggle sidebar, scroll, switch focus, quit cancel.
    #[test]
    fn integration_complex_key_sequence() {
        use crate::tui::test_helpers::{TestApp, make_key};

        let mut app = TestApp::new(default_run_app(), 100, 30);

        // Setup: dodaj content
        for i in 0..20 {
            app.state_mut()
                .push_lines(vec![Line::raw(format!("Line {i}"))]);
        }

        // Sekwencja:
        // 1. Ukryj sidebar ('t')
        app.inject_key(make_key(KeyCode::Char('t')));
        app.step();
        app.assert_state(|s| !s.sidebar.visible);

        // 2. Scroll w górę (↑ x3) — każdy krok = scroll_step=3, razem = 9
        app.inject_keys(vec![
            make_key(KeyCode::Up),
            make_key(KeyCode::Up),
            make_key(KeyCode::Up),
        ]);
        app.drain_events();
        app.assert_state(|s| s.output_view_state.scroll_offset == 9);
        app.assert_state(|s| !s.output_view_state.auto_follow);

        // 3. Pokaż sidebar ('t')
        app.inject_key(make_key(KeyCode::Char('t')));
        app.step();
        app.assert_state(|s| s.sidebar.visible);

        // 4. Przełącz focus (Tab)
        app.inject_key(make_key(KeyCode::Tab));
        app.step();
        app.assert_state(|s| s.focus == FocusArea::Sidebar);

        // 5. Wejdź w quit_pending ('q')
        app.inject_key(make_key(KeyCode::Char('q')));
        app.step();
        app.assert_state(|s| s.quit_pending);

        // 6. Anuluj quit (Esc)
        app.inject_key(make_key(KeyCode::Esc));
        app.step();
        app.assert_state(|s| !s.quit_pending);

        // 7. Przełącz focus z powrotem do Output (Tab)
        app.inject_key(make_key(KeyCode::Tab));
        app.step();
        app.assert_state(|s| s.focus == FocusArea::Output);

        // 8. Scroll output do końca (End)
        app.inject_key(make_key(KeyCode::End));
        app.step();
        app.assert_state(|s| s.output_view_state.auto_follow);
        app.assert_state(|s| s.output_view_state.scroll_offset == 0);
    }

    // ── Task 8.4: Sidebar toggle/resize integration tests ──────────

    /// Test 8.4.1: 't' → assert sidebar hidden, 't' → assert sidebar visible
    #[test]
    fn task_8_4_sidebar_toggle() {
        let mut app = default_run_app();

        // Początkowy stan — sidebar visible
        assert!(app.sidebar.visible, "Sidebar should be visible by default");

        let resolver = KeybindingResolver::with_defaults();
        // Naciśnij 't' → sidebar hidden
        let result = app.handle_event(AppEvent::Key(key(KeyCode::Char('t'))), &resolver);
        assert_eq!(
            result,
            EventResult::Consumed,
            "Toggle sidebar event should be consumed"
        );
        assert!(!app.sidebar.visible, "Sidebar should be hidden after 't'");

        // Naciśnij 't' ponownie → sidebar visible
        let result = app.handle_event(AppEvent::Key(key(KeyCode::Char('t'))), &resolver);
        assert_eq!(
            result,
            EventResult::Consumed,
            "Toggle sidebar event should be consumed"
        );
        assert!(
            app.sidebar.visible,
            "Sidebar should be visible again after second 't'"
        );
    }

    /// Test 8.4.2: ']' 5x → assert sidebar width increased by 5
    #[test]
    fn task_8_4_sidebar_grow_by_5() {
        let mut app = default_run_app();

        let initial_width = app.sidebar.width();
        assert_eq!(
            initial_width, 40,
            "Default sidebar width should be 40 (DEFAULT_WIDTH)"
        );

        let resolver = KeybindingResolver::with_defaults();
        // Naciśnij ']' 5 razy
        for i in 1..=5 {
            let result = app.handle_event(AppEvent::Key(key(KeyCode::Char(']'))), &resolver);
            assert_eq!(
                result,
                EventResult::Consumed,
                "Grow sidebar event {} should be consumed",
                i
            );
            assert_eq!(
                app.sidebar.width(),
                initial_width + i,
                "Sidebar width should increase by {} after {} ']' presses",
                i,
                i
            );
        }

        assert_eq!(
            app.sidebar.width(),
            45,
            "Sidebar width should be 45 after 5 ']' presses"
        );
    }

    /// Test 8.4.3: '[' 3x → assert sidebar width decreased by 3
    #[test]
    fn task_8_4_sidebar_shrink_by_3() {
        let mut app = default_run_app();

        let initial_width = app.sidebar.width();
        assert_eq!(initial_width, 40, "Default sidebar width should be 40");

        let resolver = KeybindingResolver::with_defaults();
        // Naciśnij '[' 3 razy
        for i in 1..=3 {
            let result = app.handle_event(AppEvent::Key(key(KeyCode::Char('['))), &resolver);
            assert_eq!(
                result,
                EventResult::Consumed,
                "Shrink sidebar event {} should be consumed",
                i
            );
            assert_eq!(
                app.sidebar.width(),
                initial_width - i,
                "Sidebar width should decrease by {} after {} '[' presses",
                i,
                i
            );
        }

        assert_eq!(
            app.sidebar.width(),
            37,
            "Sidebar width should be 37 after 3 '[' presses"
        );
    }

    /// Test 8.4.4: '[' many times → assert min width (15) not exceeded
    #[test]
    fn task_8_4_sidebar_shrink_min_width_not_exceeded() {
        let mut app = default_run_app();

        let initial_width = app.sidebar.width();
        assert_eq!(initial_width, 40, "Default sidebar width should be 40");

        let resolver = KeybindingResolver::with_defaults();
        // Naciśnij '[' 30 razy (więcej niż potrzeba do dotarcia do MIN_WIDTH=15)
        for _ in 0..30 {
            app.handle_event(AppEvent::Key(key(KeyCode::Char('['))), &resolver);
        }

        // Szerokość nie powinna być mniejsza niż MIN_WIDTH (15)
        assert_eq!(
            app.sidebar.width(),
            15,
            "Sidebar width should not go below MIN_WIDTH (15)"
        );

        // Dodatkowe naciśnięcie '[' nie powinno zmniejszyć poniżej 15
        app.handle_event(AppEvent::Key(key(KeyCode::Char('['))), &resolver);
        assert_eq!(
            app.sidebar.width(),
            15,
            "Sidebar width should remain at MIN_WIDTH (15) after additional '[' presses"
        );
    }

    /// Test 8.4 bonus: ']' many times → assert max width (50) not exceeded
    #[test]
    fn task_8_4_sidebar_grow_max_width_not_exceeded() {
        let mut app = default_run_app();

        let initial_width = app.sidebar.width();
        assert_eq!(initial_width, 40, "Default sidebar width should be 40");

        let resolver = KeybindingResolver::with_defaults();
        // Naciśnij ']' 30 razy (więcej niż potrzeba do dotarcia do MAX_WIDTH=60)
        for _ in 0..30 {
            app.handle_event(AppEvent::Key(key(KeyCode::Char(']'))), &resolver);
        }

        // Szerokość nie powinna być większa niż MAX_WIDTH (60)
        assert_eq!(
            app.sidebar.width(),
            60,
            "Sidebar width should not exceed MAX_WIDTH (60)"
        );

        // Dodatkowe naciśnięcie ']' nie powinno zwiększyć powyżej 60
        app.handle_event(AppEvent::Key(key(KeyCode::Char(']'))), &resolver);
        assert_eq!(
            app.sidebar.width(),
            60,
            "Sidebar width should remain at MAX_WIDTH (60) after additional ']' presses"
        );
    }

    /// Test 8.4 bonus: Combined toggle and resize operations
    #[test]
    fn task_8_4_sidebar_toggle_and_resize_combined() {
        let mut app = default_run_app();

        // Początkowy stan
        assert!(app.sidebar.visible, "Sidebar should be visible");
        assert_eq!(app.sidebar.width(), 40, "Default width should be 40");

        let resolver = KeybindingResolver::with_defaults();
        // Zwiększ szerokość o 5
        for _ in 0..5 {
            app.handle_event(AppEvent::Key(key(KeyCode::Char(']'))), &resolver);
        }
        assert_eq!(app.sidebar.width(), 45, "Width should be 45");

        // Ukryj sidebar
        app.handle_event(AppEvent::Key(key(KeyCode::Char('t'))), &resolver);
        assert!(!app.sidebar.visible, "Sidebar should be hidden");

        // Szerokość powinna pozostać zachowana
        assert_eq!(
            app.sidebar.width(),
            45,
            "Width should remain 45 even when hidden"
        );

        // Pokaż sidebar ponownie
        app.handle_event(AppEvent::Key(key(KeyCode::Char('t'))), &resolver);
        assert!(app.sidebar.visible, "Sidebar should be visible again");
        assert_eq!(
            app.sidebar.width(),
            45,
            "Width should still be 45 after showing"
        );

        // Zmniejsz szerokość o 10
        for _ in 0..10 {
            app.handle_event(AppEvent::Key(key(KeyCode::Char('['))), &resolver);
        }
        assert_eq!(
            app.sidebar.width(),
            35,
            "Width should be 35 after shrinking"
        );
    }

    /// Test 8.4 bonus: Resize sequence (grow → shrink → grow)
    #[test]
    fn task_8_4_sidebar_resize_sequence() {
        let mut app = default_run_app();

        assert_eq!(app.sidebar.width(), 40, "Initial width should be 40");

        let resolver = KeybindingResolver::with_defaults();
        // Grow by 10
        for _ in 0..10 {
            app.handle_event(AppEvent::Key(key(KeyCode::Char(']'))), &resolver);
        }
        assert_eq!(app.sidebar.width(), 50, "Width should be 50 after growing");

        // Shrink by 15
        for _ in 0..15 {
            app.handle_event(AppEvent::Key(key(KeyCode::Char('['))), &resolver);
        }
        assert_eq!(
            app.sidebar.width(),
            35,
            "Width should be 35 after shrinking"
        );

        // Grow by 8
        for _ in 0..8 {
            app.handle_event(AppEvent::Key(key(KeyCode::Char(']'))), &resolver);
        }
        assert_eq!(
            app.sidebar.width(),
            43,
            "Width should be 43 after final grow"
        );
    }

    // ── handle_mouse tests ──────────────────────────────────────────

    /// Helper: tworzy MouseEvent dla testów.
    fn make_mouse(kind: crossterm::event::MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    /// Klik w sidebar_rect → focus = Sidebar.
    #[test]
    fn mouse_left_click_in_sidebar_changes_focus() {
        let mut app = default_run_app();
        // Ustaw ręcznie sidebar_rect (symuluje wynik draw())
        let sidebar_rect = Rect::new(0, 0, 40, 20);
        app.sidebar_rect = Some(sidebar_rect);
        assert_eq!(app.focus, FocusArea::Output);

        let click = make_mouse(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            5, // col w obrębie sidebar
            5, // row w obrębie sidebar
        );
        let result = app.handle_mouse(click);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focus, FocusArea::Sidebar);
    }

    /// Klik poza sidebar_rect → focus bez zmian.
    #[test]
    fn mouse_left_click_outside_sidebar_no_focus_change() {
        let mut app = default_run_app();
        let sidebar_rect = Rect::new(0, 0, 40, 20);
        app.sidebar_rect = Some(sidebar_rect);
        app.focus = FocusArea::Output;

        // Klik poza sidebar (col = 50, poza obszarem 0..40)
        let click = make_mouse(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            50,
            5,
        );
        let result = app.handle_mouse(click);
        assert_eq!(result, EventResult::Ignored);
        assert_eq!(app.focus, FocusArea::Output);
    }

    /// Klik gdy brak sidebar_rect (sidebar ukryty) → Ignored.
    #[test]
    fn mouse_left_click_no_sidebar_rect_returns_ignored() {
        let mut app = default_run_app();
        assert!(app.sidebar_rect.is_none());

        let click = make_mouse(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            5,
            5,
        );
        let result = app.handle_mouse(click);
        assert_eq!(result, EventResult::Ignored);
    }

    /// Klik na konkretny task w sidebar zaznacza ten task.
    #[test]
    fn mouse_left_click_selects_task_by_row() {
        let mut app = default_run_app();

        // Załaduj 3 taski do sidebara
        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"
tasks:
  - id: "1"
    name: "Task A"
    status: todo
  - id: "2"
    name: "Task B"
    status: in_progress
  - id: "3"
    name: "Task C"
    status: done
"#,
        )
        .unwrap();
        app.sidebar.refresh(&tf);

        // Sidebar rect z offsetem y=0, padding top = SIDEBAR_PADDING_TOP (2)
        let sidebar_rect = Rect::new(0, 0, 40, 20);
        app.sidebar_rect = Some(sidebar_rect);

        // Klik na wiersz inner_y + 1 → task index = scroll_offset + 1 = 0 + 1 = 1
        let inner_y = SIDEBAR_PADDING_TOP;
        let click = make_mouse(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            5,
            inner_y + 1, // row wiersza drugiego tasku (index 1)
        );
        let result = app.handle_mouse(click);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.sidebar.selected_index, 1);
    }

    /// Klik na pierwszy task w sidebar zaznacza index 0.
    #[test]
    fn mouse_left_click_selects_first_task() {
        let mut app = default_run_app();

        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"
tasks:
  - id: "1"
    name: "Task A"
    status: todo
  - id: "2"
    name: "Task B"
    status: done
"#,
        )
        .unwrap();
        app.sidebar.refresh(&tf);
        app.sidebar.selected_index = 1; // ustaw na 2. task

        let sidebar_rect = Rect::new(0, 0, 40, 20);
        app.sidebar_rect = Some(sidebar_rect);

        // Klik na row = inner_y + 0 → index = 0
        let inner_y = SIDEBAR_PADDING_TOP;
        let click = make_mouse(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            5,
            inner_y,
        );
        let result = app.handle_mouse(click);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.sidebar.selected_index, 0);
    }

    /// Klik na row powyżej inner_y (w padding) → nie zmienia selected_index.
    #[test]
    fn mouse_left_click_in_sidebar_padding_no_task_select() {
        let mut app = default_run_app();

        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"
tasks:
  - id: "1"
    name: "Task A"
    status: todo
"#,
        )
        .unwrap();
        app.sidebar.refresh(&tf);
        app.sidebar.selected_index = 0;

        let sidebar_rect = Rect::new(0, 0, 40, 20);
        app.sidebar_rect = Some(sidebar_rect);

        // Klik na row = inner_y - 1 (w obszarze paddingu, przed listą tasków)
        let inner_y = SIDEBAR_PADDING_TOP;
        if inner_y > 0 {
            let click = make_mouse(
                crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                5,
                inner_y - 1, // w obszarze paddingu
            );
            let result = app.handle_mouse(click);
            // Klik jest w sidebar_rect → focus zmienia się, ale selected_index NIE
            assert_eq!(result, EventResult::Consumed);
            assert_eq!(app.focus, FocusArea::Sidebar);
            // selected_index nie zmienia się (row < inner_y)
            assert_eq!(app.sidebar.selected_index, 0);
        }
    }

    /// Scroll myszy poza sidebar i output → Ignored.
    #[test]
    fn mouse_scroll_outside_all_rects_returns_ignored() {
        let mut app = default_run_app();
        // sidebar: x=0..40, y=0..20
        app.sidebar_rect = Some(Rect::new(0, 0, 40, 20));
        // output: x=40..120, y=0..20
        app.output_rect = Some(Rect::new(40, 0, 80, 20));

        // Kursor na kolumnie 120 — poza oboma prostokątami
        let scroll = make_mouse(crossterm::event::MouseEventKind::ScrollUp, 120, 5);
        let result = app.handle_mouse(scroll);
        assert_eq!(result, EventResult::Ignored);
    }

    // ── handle_mouse_scroll tests ────────────────────────────────────

    /// ScrollUp nad sidebar → select_prev (nawigacja, krok=1).
    #[test]
    fn mouse_scroll_up_in_sidebar_navigates_prev() {
        let mut app = default_run_app();

        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"tasks:
  - id: "1"
    name: "Task A"
    status: todo
  - id: "2"
    name: "Task B"
    status: todo
"#,
        )
        .unwrap();
        app.sidebar.refresh(&tf);
        // Zaznacz drugi task
        app.sidebar.select_index(1);
        assert_eq!(app.sidebar.selected_index, 1);

        app.sidebar_rect = Some(Rect::new(0, 0, 40, 20));

        let scroll = make_mouse(crossterm::event::MouseEventKind::ScrollUp, 5, 5);
        let result = app.handle_mouse(scroll);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.sidebar.selected_index, 0);
    }

    /// ScrollDown nad sidebar → select_next (nawigacja, krok=1).
    #[test]
    fn mouse_scroll_down_in_sidebar_navigates_next() {
        let mut app = default_run_app();

        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"tasks:
  - id: "1"
    name: "Task A"
    status: todo
  - id: "2"
    name: "Task B"
    status: todo
"#,
        )
        .unwrap();
        app.sidebar.refresh(&tf);
        assert_eq!(app.sidebar.selected_index, 0);

        app.sidebar_rect = Some(Rect::new(0, 0, 40, 20));

        let scroll = make_mouse(crossterm::event::MouseEventKind::ScrollDown, 5, 5);
        let result = app.handle_mouse(scroll);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.sidebar.selected_index, 1);
    }

    /// ScrollUp nad output → scroll_up output buffera (krok=scroll_step).
    #[test]
    fn mouse_scroll_up_in_output_scrolls_output() {
        let mut app = default_run_app();
        app.scroll_step = 3;
        // Output at x=40..120, y=0..20
        app.output_rect = Some(Rect::new(40, 0, 80, 20));

        // Symuluj treść w buforze (żeby auto_follow = false po scroll_up)
        let initial_offset = app.output_view_state.scroll_offset;

        let scroll = make_mouse(crossterm::event::MouseEventKind::ScrollUp, 50, 5);
        let result = app.handle_mouse(scroll);
        assert_eq!(result, EventResult::Consumed);
        // scroll_up zwiększa scroll_offset (przewijanie wstecz) i wyłącza auto_follow
        assert_eq!(app.output_view_state.scroll_offset, initial_offset + 3);
        assert!(!app.output_view_state.auto_follow);
    }

    /// ScrollDown nad output → scroll_down output buffera (krok=scroll_step).
    #[test]
    fn mouse_scroll_down_in_output_scrolls_output() {
        let mut app = default_run_app();
        app.scroll_step = 3;
        app.output_rect = Some(Rect::new(40, 0, 80, 20));

        // Najpierw scrolluj w górę, żeby mieć co scrollować w dół
        app.output_view_state.scroll_offset = 10;
        app.output_view_state.auto_follow = false;

        let scroll = make_mouse(crossterm::event::MouseEventKind::ScrollDown, 50, 5);
        let result = app.handle_mouse(scroll);
        assert_eq!(result, EventResult::Consumed);
        // scroll_down zmniejsza scroll_offset (do minimum 0)
        assert_eq!(app.output_view_state.scroll_offset, 7);
    }

    /// Sidebar ma priorytet nad output przy nakładaniu się (Small mode overlay).
    #[test]
    fn mouse_scroll_sidebar_priority_over_output() {
        let mut app = default_run_app();

        let tf: crate::shared::tasks::TasksFile = serde_yaml::from_str(
            r#"tasks:
  - id: "1"
    name: "Task A"
    status: todo
  - id: "2"
    name: "Task B"
    status: todo
"#,
        )
        .unwrap();
        app.sidebar.refresh(&tf);
        app.sidebar.select_index(1);

        // Sidebar i output pokrywają ten sam obszar (jak w Small mode overlay)
        app.sidebar_rect = Some(Rect::new(0, 0, 80, 20));
        app.output_rect = Some(Rect::new(0, 0, 80, 20));

        let scroll = make_mouse(crossterm::event::MouseEventKind::ScrollUp, 5, 5);
        let result = app.handle_mouse(scroll);
        assert_eq!(result, EventResult::Consumed);
        // Sidebar obsłużył scroll (not output)
        assert_eq!(app.sidebar.selected_index, 0);
    }

    /// handle_event deleguje AppEvent::Mouse do handle_mouse.
    #[test]
    fn handle_event_mouse_delegates_to_handle_mouse() {
        let mut app = default_run_app();
        let sidebar_rect = Rect::new(0, 0, 40, 20);
        app.sidebar_rect = Some(sidebar_rect);
        assert_eq!(app.focus, FocusArea::Output);

        let resolver = KeybindingResolver::with_defaults();
        let click = make_mouse(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            5,
            5,
        );
        let result = app.handle_event(AppEvent::Mouse(click), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focus, FocusArea::Sidebar);
    }
}
