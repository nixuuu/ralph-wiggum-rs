//! Command registry dla orchestrate command palette.
//!
//! Definiuje [`OrchestrateAction`] — aplikacyjne akcje dostępne z command palette.
//! Zawiera logikę budowania dynamicznej listy elementów oraz ich wykonywania.
//!
//! ## Architektura
//!
//! ```text
//! Ctrl+P → OrchestrateApp::open_command_palette()
//!        → build_orchestrate_items(app) → Vec<PaletteItem>
//!
//! Enter → PaletteAction::Select(id)
//!       → OrchestrateAction::from_palette_id(id)
//!       → execute_palette_action(action, app)
//! ```

use crate::commands::task::orchestrate::app::{OrchestrateApp, RestartState};
use crate::tui::widgets::PaletteItem;

// ── OrchestrateAction ──────────────────────────────────────────────────────

/// Aplikacyjna akcja orchestrate dostępna z command palette.
///
/// Każda akcja ma deterministyczny string ID kodowany w [`PaletteItem::id`],
/// co pozwala na konwersję w obie strony przez [`to_palette_id`] i [`from_palette_id`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestrateAction {
    /// Przełącz focus na konkretnego workera (1-based ID).
    FocusWorker(u32),
    /// Przełącz focus na workera aktualnie obsługującego dany task.
    FocusTask(String),
    /// Pokaż/ukryj sidebar z drzewem tasków.
    ToggleSidebar,
    /// Pokaż/ukryj podgląd tasku.
    TogglePreview,
    /// Przeładuj tasks.yml z dysku.
    ReloadTasks,
    /// Zrestartuj konkretnego workera (inicjuje stan `Pending`).
    RestartWorker(u32),
    /// Wyślij wiadomość do konkretnego workera (otwiera overlay input).
    SendMessage(u32),
    /// Graceful shutdown — zainicjuj zatrzymanie orchestratora.
    Quit,
}

// ── ID konstanty ──────────────────────────────────────────────────────────

// Prefiksy dla ID zawierających dane liczbowe (worker ID, task ID)
const ID_WORKER_FOCUS_PREFIX: &str = "worker.focus.";
const ID_TASK_FOCUS_PREFIX: &str = "task.focus.";
const ID_WORKER_RESTART_PREFIX: &str = "worker.restart.";
const ID_WORKER_MESSAGE_PREFIX: &str = "worker.message.";

// Stałe ID dla akcji bez parametrów
const ID_TOGGLE_SIDEBAR: &str = "setting.toggle_sidebar";
const ID_TOGGLE_PREVIEW: &str = "setting.toggle_preview";
const ID_RELOAD_TASKS: &str = "setting.reload_tasks";
const ID_QUIT: &str = "app.quit";

impl OrchestrateAction {
    /// Konwertuje akcję do unikalnego string ID używanego w [`PaletteItem::id`].
    pub fn to_palette_id(&self) -> String {
        match self {
            Self::FocusWorker(id) => format!("{ID_WORKER_FOCUS_PREFIX}{id}"),
            Self::FocusTask(task_id) => format!("{ID_TASK_FOCUS_PREFIX}{task_id}"),
            Self::ToggleSidebar => ID_TOGGLE_SIDEBAR.to_string(),
            Self::TogglePreview => ID_TOGGLE_PREVIEW.to_string(),
            Self::ReloadTasks => ID_RELOAD_TASKS.to_string(),
            Self::RestartWorker(id) => format!("{ID_WORKER_RESTART_PREFIX}{id}"),
            Self::SendMessage(id) => format!("{ID_WORKER_MESSAGE_PREFIX}{id}"),
            Self::Quit => ID_QUIT.to_string(),
        }
    }

    /// Parsuje string ID z powrotem do akcji. Zwraca `None` jeśli ID jest nieznane.
    ///
    /// Kolejność sprawdzania prefiksów ma znaczenie — sprawdzamy najpierw dłuższe
    /// (restart/message przed focus), aby uniknąć fałszywych dopasowań.
    pub fn from_palette_id(id: &str) -> Option<Self> {
        // Dłuższe prefiksy najpierw (worker.restart > worker.focus, worker.message > worker.focus)
        if let Some(rest) = id.strip_prefix(ID_WORKER_RESTART_PREFIX) {
            return rest.parse::<u32>().ok().map(Self::RestartWorker);
        }
        if let Some(rest) = id.strip_prefix(ID_WORKER_MESSAGE_PREFIX) {
            return rest.parse::<u32>().ok().map(Self::SendMessage);
        }
        if let Some(rest) = id.strip_prefix(ID_WORKER_FOCUS_PREFIX) {
            return rest.parse::<u32>().ok().map(Self::FocusWorker);
        }
        if let Some(rest) = id.strip_prefix(ID_TASK_FOCUS_PREFIX) {
            return Some(Self::FocusTask(rest.to_string()));
        }

        match id {
            ID_TOGGLE_SIDEBAR => Some(Self::ToggleSidebar),
            ID_TOGGLE_PREVIEW => Some(Self::TogglePreview),
            ID_RELOAD_TASKS => Some(Self::ReloadTasks),
            ID_QUIT => Some(Self::Quit),
            _ => None,
        }
    }
}

// ── build_orchestrate_items ───────────────────────────────────────────────

/// Buduje dynamiczną listę [`PaletteItem`] z aktualnego stanu [`OrchestrateApp`].
///
/// Grupy elementów (w kolejności wyświetlania):
/// 1. **Worker** — jeden item na workera (Worker N: task_id lub "idle")
/// 2. **Task** — listy tasków z `tasks_file` (do wyszukiwania po nazwie/ID)
/// 3. **Setting** — Toggle Sidebar, Toggle Preview, Reload Tasks
/// 4. **Command** — Restart Worker N, Send Message to Worker N, Quit
pub fn build_orchestrate_items(app: &OrchestrateApp) -> Vec<PaletteItem> {
    let mut items = Vec::new();

    // Posortowane IDs workerów (1..=worker_count)
    let worker_ids: Vec<u32> = (1..=app.worker_count).collect();

    // ── 1. Worker items ──────────────────────────────────────────────
    for &id in &worker_ids {
        let Some(panel) = app.panels.get(&id) else {
            continue;
        };

        // Etykieta: "Worker N: task_id" lub "Worker N" gdy idle
        let label = match panel.status.task_id.as_deref() {
            Some(tid) => format!("Worker {id}: {tid}"),
            None => format!("Worker {id}"),
        };

        items.push(PaletteItem {
            id: OrchestrateAction::FocusWorker(id).to_palette_id(),
            label,
            description: Some(panel.status.state.to_string()),
            icon: None,
            category: "Worker".into(),
        });
    }

    // ── 2. Task items — z tasks_file ─────────────────────────────────
    if let Some(tf) = app.tasks_file() {
        for leaf in tf.flatten_leaves() {
            let description = if leaf.component.is_empty() {
                None
            } else {
                Some(leaf.component.clone())
            };

            items.push(PaletteItem {
                id: OrchestrateAction::FocusTask(leaf.id.clone()).to_palette_id(),
                label: format!("{}: {}", leaf.id, leaf.name),
                description,
                icon: None,
                category: "Task".into(),
            });
        }
    }

    // ── 3. Settings ──────────────────────────────────────────────────
    items.push(PaletteItem {
        id: OrchestrateAction::ToggleSidebar.to_palette_id(),
        label: "Toggle Sidebar".into(),
        description: Some("Show/hide task tree sidebar".into()),
        icon: None,
        category: "Setting".into(),
    });
    items.push(PaletteItem {
        id: OrchestrateAction::TogglePreview.to_palette_id(),
        label: "Toggle Preview".into(),
        description: Some("Show/hide task preview overlay".into()),
        icon: None,
        category: "Setting".into(),
    });
    items.push(PaletteItem {
        id: OrchestrateAction::ReloadTasks.to_palette_id(),
        label: "Reload Tasks".into(),
        description: Some("Reload tasks.yml from disk".into()),
        icon: None,
        category: "Setting".into(),
    });

    // ── 4. Per-worker commands ────────────────────────────────────────
    for &id in &worker_ids {
        items.push(PaletteItem {
            id: OrchestrateAction::RestartWorker(id).to_palette_id(),
            label: format!("Restart Worker {id}"),
            description: Some("Re-queue current task and restart worker".into()),
            icon: None,
            category: "Command".into(),
        });
        items.push(PaletteItem {
            id: OrchestrateAction::SendMessage(id).to_palette_id(),
            label: format!("Send Message to Worker {id}"),
            description: Some("Open input overlay for this worker".into()),
            icon: None,
            category: "Command".into(),
        });
    }

    // ── Quit ─────────────────────────────────────────────────────────
    items.push(PaletteItem {
        id: OrchestrateAction::Quit.to_palette_id(),
        label: "Quit".into(),
        description: Some("Graceful shutdown".into()),
        icon: None,
        category: "Command".into(),
    });

    items
}

// ── execute_palette_action ────────────────────────────────────────────────

/// Wykonuje akcję wybraną z command palette.
///
/// Mapuje [`OrchestrateAction`] na mutację stanu [`OrchestrateApp`].
/// Zachowanie jest identyczne z bezpośrednimi klawiszami skrótów (np. `t` = ToggleSidebar).
pub fn execute_palette_action(action: OrchestrateAction, app: &mut OrchestrateApp) {
    match action {
        OrchestrateAction::FocusWorker(id) => {
            // Ustaw focus na workera — reset scroll następuje w set_focus()
            app.set_focus(Some(id));
        }

        OrchestrateAction::FocusTask(task_id) => {
            // Znajdź workera aktualnie obsługującego ten task
            let worker_id = app
                .panels
                .iter()
                .find(|(_, panel)| panel.status.task_id.as_deref() == Some(task_id.as_str()))
                .map(|(id, _)| *id);

            if let Some(wid) = worker_id {
                app.set_focus(Some(wid));
            }
            // Jeśli task nie jest aktualnie przetwarzany przez żadnego workera — brak akcji
        }

        OrchestrateAction::ToggleSidebar => {
            // Identyczna logika jak klawisz 't'
            app.toggle_sidebar();
        }

        OrchestrateAction::TogglePreview => {
            // Identyczna logika jak klawisz 'p'
            app.show_task_preview = !app.show_task_preview;
        }

        OrchestrateAction::ReloadTasks => {
            // Identyczna logika jak klawisz 'r'
            app.reload_requested = true;
        }

        OrchestrateAction::RestartWorker(id) => {
            // Inicjuj restart tylko dla aktywnych workerów
            if app.active_worker_ids.contains(&id) {
                app.restart_state = RestartState::Pending { worker_id: id };
            }
        }

        OrchestrateAction::SendMessage(id) => {
            use crate::commands::task::orchestrate::events::WorkerPhase;

            // Guard: worker musi być aktywny i w fazie Claude (Implement/Review/Fix/ReviewFix)
            // Identyczna logika jak handle_input_overlay_key() w app_keys.rs
            let is_claude_phase = app
                .panels
                .get(&id)
                .and_then(|p| p.status.phase.as_ref())
                .map(|phase| {
                    matches!(
                        phase,
                        WorkerPhase::Implement
                            | WorkerPhase::Review
                            | WorkerPhase::Fix
                            | WorkerPhase::ReviewFix
                    )
                })
                .unwrap_or(false);

            if app.active_worker_ids.contains(&id) && is_claude_phase {
                app.set_focus(Some(id));
                *app.shared_overlay.lock().expect("shared_overlay poisoned") =
                    Some(crate::tui::widgets::TextInputOverlay::new(id));
            }
        }

        OrchestrateAction::Quit => {
            // Graceful shutdown — identyczna logika jak qq lub Ctrl+C
            app.graceful_shutdown = true;
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::task::orchestrate::app::OrchestrateApp;
    use crate::commands::task::orchestrate::worker_status::WorkerState;
    use std::sync::{Arc, Mutex};

    fn make_app(worker_count: u32) -> OrchestrateApp {
        OrchestrateApp::new(worker_count, Arc::new(Mutex::new(None)))
    }

    // ── OrchestrateAction ID roundtrip tests ──────────────────────────

    #[test]
    fn test_focus_worker_id_roundtrip() {
        let action = OrchestrateAction::FocusWorker(3);
        let id = action.to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&id),
            Some(OrchestrateAction::FocusWorker(3))
        );
    }

    #[test]
    fn test_focus_task_id_roundtrip() {
        let action = OrchestrateAction::FocusTask("12.3".into());
        let id = action.to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&id),
            Some(OrchestrateAction::FocusTask("12.3".into()))
        );
    }

    #[test]
    fn test_toggle_sidebar_id_roundtrip() {
        let action = OrchestrateAction::ToggleSidebar;
        let id = action.to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&id),
            Some(OrchestrateAction::ToggleSidebar)
        );
    }

    #[test]
    fn test_toggle_preview_id_roundtrip() {
        let action = OrchestrateAction::TogglePreview;
        let id = action.to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&id),
            Some(OrchestrateAction::TogglePreview)
        );
    }

    #[test]
    fn test_reload_tasks_id_roundtrip() {
        let action = OrchestrateAction::ReloadTasks;
        let id = action.to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&id),
            Some(OrchestrateAction::ReloadTasks)
        );
    }

    #[test]
    fn test_restart_worker_id_roundtrip() {
        let action = OrchestrateAction::RestartWorker(2);
        let id = action.to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&id),
            Some(OrchestrateAction::RestartWorker(2))
        );
    }

    #[test]
    fn test_send_message_id_roundtrip() {
        let action = OrchestrateAction::SendMessage(1);
        let id = action.to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&id),
            Some(OrchestrateAction::SendMessage(1))
        );
    }

    #[test]
    fn test_quit_id_roundtrip() {
        let action = OrchestrateAction::Quit;
        let id = action.to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&id),
            Some(OrchestrateAction::Quit)
        );
    }

    #[test]
    fn test_from_palette_id_unknown_returns_none() {
        assert_eq!(OrchestrateAction::from_palette_id("unknown.action"), None);
        assert_eq!(OrchestrateAction::from_palette_id(""), None);
        assert_eq!(OrchestrateAction::from_palette_id("worker.focus."), None); // missing number
    }

    // Sprawdza że prefiksy nie kolidują (worker.restart nie pasuje do worker.focus)
    #[test]
    fn test_prefix_disambiguation() {
        let restart_id = OrchestrateAction::RestartWorker(1).to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&restart_id),
            Some(OrchestrateAction::RestartWorker(1))
        );
        // Nie może być parsowane jako FocusWorker
        assert_ne!(
            OrchestrateAction::from_palette_id(&restart_id),
            Some(OrchestrateAction::FocusWorker(1))
        );

        let message_id = OrchestrateAction::SendMessage(1).to_palette_id();
        assert_eq!(
            OrchestrateAction::from_palette_id(&message_id),
            Some(OrchestrateAction::SendMessage(1))
        );
    }

    // ── build_orchestrate_items tests ──────────────────────────────────

    #[test]
    fn test_build_items_contains_all_workers() {
        let app = make_app(3);
        let items = build_orchestrate_items(&app);

        let worker_items: Vec<_> = items.iter().filter(|i| i.category == "Worker").collect();
        assert_eq!(worker_items.len(), 3);

        // Worker IDs 1, 2, 3 powinny być obecne
        for id in 1..=3u32 {
            assert!(
                worker_items
                    .iter()
                    .any(|i| i.label.contains(&format!("Worker {id}"))),
                "Missing Worker {id} in palette items"
            );
        }
    }

    #[test]
    fn test_build_items_worker_shows_task_id_when_active() {
        let mut app = make_app(2);
        app.panels.get_mut(&1).unwrap().status.task_id = Some("7.2".into());

        let items = build_orchestrate_items(&app);
        let worker1 = items
            .iter()
            .find(|i| i.category == "Worker" && i.label.contains("Worker 1"))
            .expect("Worker 1 item missing");

        assert!(
            worker1.label.contains("7.2"),
            "Worker 1 label should show task ID"
        );
    }

    #[test]
    fn test_build_items_worker_shows_idle_label_when_no_task() {
        let app = make_app(2);
        let items = build_orchestrate_items(&app);
        let worker1 = items
            .iter()
            .find(|i| i.category == "Worker" && i.label.contains("Worker 1"))
            .expect("Worker 1 item missing");

        // Brak task_id → label to samo "Worker 1" (bez sufiksu)
        assert_eq!(worker1.label, "Worker 1");
    }

    #[test]
    fn test_build_items_contains_settings() {
        let app = make_app(1);
        let items = build_orchestrate_items(&app);

        let setting_ids: Vec<_> = items
            .iter()
            .filter(|i| i.category == "Setting")
            .map(|i| i.id.as_str())
            .collect();

        assert!(
            setting_ids.contains(&ID_TOGGLE_SIDEBAR),
            "Missing ToggleSidebar"
        );
        assert!(
            setting_ids.contains(&ID_TOGGLE_PREVIEW),
            "Missing TogglePreview"
        );
        assert!(
            setting_ids.contains(&ID_RELOAD_TASKS),
            "Missing ReloadTasks"
        );
    }

    #[test]
    fn test_build_items_contains_quit() {
        let app = make_app(1);
        let items = build_orchestrate_items(&app);

        assert!(items.iter().any(|i| i.id == ID_QUIT), "Missing Quit item");
    }

    #[test]
    fn test_build_items_commands_per_worker() {
        let app = make_app(2);
        let items = build_orchestrate_items(&app);

        // Dla 2 workerów powinny być 2 * restart + 2 * message + quit = 5 command items
        let command_items: Vec<_> = items.iter().filter(|i| i.category == "Command").collect();

        // Znajdź Restart Worker 1, Restart Worker 2, Message Worker 1, Message Worker 2, Quit
        let restart_1 = OrchestrateAction::RestartWorker(1).to_palette_id();
        let restart_2 = OrchestrateAction::RestartWorker(2).to_palette_id();
        let message_1 = OrchestrateAction::SendMessage(1).to_palette_id();
        let message_2 = OrchestrateAction::SendMessage(2).to_palette_id();

        assert!(command_items.iter().any(|i| i.id == restart_1));
        assert!(command_items.iter().any(|i| i.id == restart_2));
        assert!(command_items.iter().any(|i| i.id == message_1));
        assert!(command_items.iter().any(|i| i.id == message_2));
        assert!(command_items.iter().any(|i| i.id == ID_QUIT));
    }

    #[test]
    fn test_build_items_all_ids_parseable() {
        // Sprawdź że każdy item ma ID które można sparsować z powrotem do OrchestrateAction
        let app = make_app(3);
        let items = build_orchestrate_items(&app);

        for item in &items {
            assert!(
                OrchestrateAction::from_palette_id(&item.id).is_some(),
                "Item '{}' (id='{}') has unparseable ID",
                item.label,
                item.id
            );
        }
    }

    #[test]
    fn test_build_items_no_tasks_file_skips_task_items() {
        let app = make_app(1);
        let items = build_orchestrate_items(&app);

        // Brak tasks_file → brak task items
        assert!(
            items.iter().all(|i| i.category != "Task"),
            "Should not have Task items when tasks_file is None"
        );
    }

    // ── execute_palette_action tests ───────────────────────────────────

    #[test]
    fn test_execute_focus_worker_sets_focus() {
        let mut app = make_app(3);
        execute_palette_action(OrchestrateAction::FocusWorker(2), &mut app);
        assert_eq!(app.focused_worker(), Some(2));
    }

    #[test]
    fn test_execute_toggle_sidebar_changes_visibility() {
        let mut app = make_app(1);
        let initial = app.sidebar_state.visible;

        execute_palette_action(OrchestrateAction::ToggleSidebar, &mut app);

        assert_ne!(
            app.sidebar_state.visible, initial,
            "Sidebar visibility should toggle"
        );
    }

    #[test]
    fn test_execute_toggle_preview_flips_flag() {
        let mut app = make_app(1);
        assert!(!app.show_task_preview);

        execute_palette_action(OrchestrateAction::TogglePreview, &mut app);
        assert!(app.show_task_preview);

        execute_palette_action(OrchestrateAction::TogglePreview, &mut app);
        assert!(!app.show_task_preview);
    }

    #[test]
    fn test_execute_reload_tasks_sets_flag() {
        let mut app = make_app(1);
        assert!(!app.reload_requested);

        execute_palette_action(OrchestrateAction::ReloadTasks, &mut app);
        assert!(app.reload_requested);
    }

    #[test]
    fn test_execute_quit_sets_graceful_shutdown() {
        let mut app = make_app(1);
        assert!(!app.is_graceful_shutdown());

        execute_palette_action(OrchestrateAction::Quit, &mut app);
        assert!(app.is_graceful_shutdown());
    }

    #[test]
    fn test_execute_restart_worker_for_active_worker() {
        use crate::commands::task::orchestrate::app::RestartState;

        let mut app = make_app(3);
        // Aktywuj workera 2
        app.panels.get_mut(&2).unwrap().status.state = WorkerState::Implementing;
        app.refresh_active_worker_ids();

        execute_palette_action(OrchestrateAction::RestartWorker(2), &mut app);

        assert_eq!(app.restart_state(), &RestartState::Pending { worker_id: 2 });
    }

    #[test]
    fn test_execute_restart_worker_ignores_idle_worker() {
        use crate::commands::task::orchestrate::app::RestartState;

        let mut app = make_app(3);
        // Worker 1 jest idle (default) — restart powinien być zignorowany
        app.refresh_active_worker_ids();

        execute_palette_action(OrchestrateAction::RestartWorker(1), &mut app);

        assert_eq!(app.restart_state(), &RestartState::None);
    }

    #[test]
    fn test_execute_focus_task_finds_worker() {
        let mut app = make_app(3);
        // Worker 2 obsługuje task "5.3"
        app.panels.get_mut(&2).unwrap().status.task_id = Some("5.3".into());
        app.panels.get_mut(&2).unwrap().status.state = WorkerState::Implementing;
        app.refresh_active_worker_ids();

        execute_palette_action(OrchestrateAction::FocusTask("5.3".into()), &mut app);

        assert_eq!(app.focused_worker(), Some(2));
    }

    #[test]
    fn test_execute_focus_task_no_match_no_focus_change() {
        let mut app = make_app(3);
        app.set_focus(Some(1));

        // Task "99.9" nie jest obsługiwany przez żadnego workera
        execute_palette_action(OrchestrateAction::FocusTask("99.9".into()), &mut app);

        // Focus powinien pozostać bez zmian
        assert_eq!(app.focused_worker(), Some(1));
    }

    #[test]
    fn test_execute_send_message_requires_claude_phase() {
        use crate::commands::task::orchestrate::events::WorkerPhase;

        let mut app = make_app(3);
        app.panels.get_mut(&1).unwrap().status.state = WorkerState::Implementing;
        app.refresh_active_worker_ids();

        // Worker w fazie Setup — overlay nie powinien się otworzyć
        app.panels.get_mut(&1).unwrap().status.phase = Some(WorkerPhase::Setup);
        execute_palette_action(OrchestrateAction::SendMessage(1), &mut app);
        assert!(
            !app.is_overlay_active(),
            "SendMessage should not open overlay in Setup phase"
        );

        // Worker w fazie Implement — overlay powinien się otworzyć
        app.panels.get_mut(&1).unwrap().status.phase = Some(WorkerPhase::Implement);
        execute_palette_action(OrchestrateAction::SendMessage(1), &mut app);
        assert!(
            app.is_overlay_active(),
            "SendMessage should open overlay in Implement phase"
        );
    }

    #[test]
    fn test_execute_send_message_ignores_idle_worker() {
        let mut app = make_app(3);
        // Worker 1 jest idle (nie aktywny)
        app.refresh_active_worker_ids();

        execute_palette_action(OrchestrateAction::SendMessage(1), &mut app);
        assert!(
            !app.is_overlay_active(),
            "SendMessage should not open overlay for idle worker"
        );
    }

    // ── Registry completeness test ─────────────────────────────────────

    /// Test sprawdzający że registry zawiera wszystkie wymagane kategorie akcji.
    #[test]
    fn test_registry_contains_all_required_action_categories() {
        let app = make_app(2);
        let items = build_orchestrate_items(&app);

        // Worker focus items
        assert!(
            items
                .iter()
                .any(|i| OrchestrateAction::from_palette_id(&i.id)
                    == Some(OrchestrateAction::FocusWorker(1))),
            "Missing FocusWorker(1)"
        );
        assert!(
            items
                .iter()
                .any(|i| OrchestrateAction::from_palette_id(&i.id)
                    == Some(OrchestrateAction::FocusWorker(2))),
            "Missing FocusWorker(2)"
        );

        // Settings
        assert!(
            items
                .iter()
                .any(|i| OrchestrateAction::from_palette_id(&i.id)
                    == Some(OrchestrateAction::ToggleSidebar)),
            "Missing ToggleSidebar"
        );
        assert!(
            items
                .iter()
                .any(|i| OrchestrateAction::from_palette_id(&i.id)
                    == Some(OrchestrateAction::TogglePreview)),
            "Missing TogglePreview"
        );
        assert!(
            items
                .iter()
                .any(|i| OrchestrateAction::from_palette_id(&i.id)
                    == Some(OrchestrateAction::ReloadTasks)),
            "Missing ReloadTasks"
        );

        // Commands
        assert!(
            items
                .iter()
                .any(|i| OrchestrateAction::from_palette_id(&i.id)
                    == Some(OrchestrateAction::RestartWorker(1))),
            "Missing RestartWorker(1)"
        );
        assert!(
            items
                .iter()
                .any(|i| OrchestrateAction::from_palette_id(&i.id)
                    == Some(OrchestrateAction::SendMessage(1))),
            "Missing SendMessage(1)"
        );
        assert!(
            items
                .iter()
                .any(|i| OrchestrateAction::from_palette_id(&i.id)
                    == Some(OrchestrateAction::Quit)),
            "Missing Quit"
        );
    }
}
