//! TaskExplorerApp — główny stan aplikacji Task Explorer.
//!
//! Zawiera struktury danych (Panel, SortMode, TaskExplorerApp)
//! i logikę zarządzania stanem (nawigacja, sortowanie, filtrowanie).

use std::cmp::Ordering as CmpOrdering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;

use crate::shared::progress::TaskStatus;
use crate::shared::tasks::{TaskNode, TasksFile};
use crate::tui::widgets::task_tree::{self, FlatRow, TreeState};

// ── Input mode ───────────────────────────────────────────────────────

/// Tryb wprowadzania danych — determinuje jak reaguje UI na wciśnięcia klawiszy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Normalny tryb — nawigacja i komendy
    Normal,
    /// Tryb filtrowania — wpisywanie tekstu filtra
    Filter,
}

// ── Panel focus ──────────────────────────────────────────────────────

/// Aktywny panel w UI — determinuje dokąd trafiają key events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// Panel drzewa zadań (lewy)
    Tree,
    /// Panel szczegółów wybranego taska (prawy)
    Detail,
}

// ── Sort mode ────────────────────────────────────────────────────────

/// Tryb sortowania widocznych wierszy drzewa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    /// Domyślna kolejność z pliku YAML (wg ID)
    Id,
    /// Sortowanie wg statusu (done → in_progress → todo → blocked)
    Status,
    /// Sortowanie wg komponentu (alfabetycznie)
    Component,
}

impl SortMode {
    /// Przełącz na następny tryb sortowania (cyklicznie).
    pub fn next(self) -> Self {
        match self {
            SortMode::Id => SortMode::Status,
            SortMode::Status => SortMode::Component,
            SortMode::Component => SortMode::Id,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortMode::Id => "ID",
            SortMode::Status => "Status",
            SortMode::Component => "Component",
        }
    }
}

/// Priorytet sortowania statusu (mniejszy = wyżej).
/// Kolejność: done → in_progress → todo → blocked
fn status_sort_key(status: &Option<TaskStatus>) -> u8 {
    match status {
        Some(TaskStatus::Done) => 0,
        Some(TaskStatus::InProgress) => 1,
        Some(TaskStatus::Todo) => 2,
        Some(TaskStatus::Blocked) => 3,
        None => 4,
    }
}

// ── TaskExplorerApp ──────────────────────────────────────────────────

/// Główny stan aplikacji Task Explorer.
///
/// Przechowuje drzewo zadań, stan nawigacji, filtrowania i sortowania.
/// Implementuje `AppState` do użycia z `App::run()`.
pub struct TaskExplorerApp {
    /// Załadowane drzewo zadań z .ralph/tasks.yml
    pub(crate) tasks: TasksFile,
    /// Ścieżka do pliku tasks.yml (do ewentualnego reloadu)
    pub(crate) tasks_path: PathBuf,
    /// Stan drzewa (expanded nodes, selection, scroll)
    pub(crate) tree_state: TreeState,
    /// ID wybranego taska (zsynchronizowane z tree_state.selected)
    pub(crate) selected_id: Option<String>,
    /// Aktywny panel (Tree/Detail)
    pub(crate) focus: Panel,
    /// Tryb wprowadzania danych (Normal/Filter)
    pub(crate) input_mode: InputMode,
    /// Tekst filtra — pusty = brak filtrowania
    pub(crate) filter: String,
    /// Tryb sortowania
    pub(crate) sort_mode: SortMode,
    /// Scroll offset panelu Detail (do przewijania długich opisów)
    pub(crate) detail_scroll: usize,
    /// Liczba linii przewijana przy zdarzeniu scroll myszy (z TuiConfig)
    pub(crate) scroll_step: usize,
    /// Cache prostokątów wierszy drzewa: (visible_index, Rect).
    /// Aktualizowany w każdym draw() — używany do wykrywania kliknięć myszy.
    pub(crate) task_row_rects: Vec<(usize, Rect)>,
}

impl TaskExplorerApp {
    /// Utwórz nowy explorer z załadowanymi danymi z podanej ścieżki.
    pub fn load(tasks_path: &Path) -> crate::shared::error::Result<Self> {
        let tasks = TasksFile::load(tasks_path)?;

        // Domyślnie rozwiń root nodes (epiki) dla lepszego overview
        let mut expanded = HashSet::new();
        for node in &tasks.tasks {
            expanded.insert(node.id.clone());
        }

        let tree_state = TreeState {
            expanded,
            selected: 0,
            scroll_offset: 0,
        };

        // Oblicz initial selected_id
        let rows = task_tree::flatten_nodes(&tasks.tasks, &tree_state.expanded);
        let selected_id = rows.first().map(|r| r.id.clone());

        Ok(Self {
            tasks,
            tasks_path: tasks_path.to_path_buf(),
            tree_state,
            selected_id,
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Id,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
        })
    }

    /// Ustaw konfigurowalny scroll step (z TuiConfig). Builder pattern.
    pub fn with_scroll_step(mut self, step: u16) -> Self {
        self.scroll_step = step.max(1) as usize;
        self
    }

    /// Odśwież drzewo zadań z dysku.
    pub fn reload(&mut self) -> crate::shared::error::Result<()> {
        self.tasks = TasksFile::load(&self.tasks_path)?;
        self.sync_selected_id();
        Ok(())
    }

    /// Zsynchronizuj selected_id na podstawie tree_state.selected.
    /// Clamp selekcji do zakresu widocznych wierszy (ważne po zmianie filtra).
    pub(crate) fn sync_selected_id(&mut self) {
        let rows = self.visible_rows();
        self.tree_state.clamp(rows.len());
        self.selected_id = rows.get(self.tree_state.selected).map(|r| r.id.clone());
        // Reset detail scroll po zmianie selekcji
        self.detail_scroll = 0;
    }

    /// Po zmianie sortowania: znajdź nowy indeks dla bieżącego selected_id.
    /// Jeśli ID nie istnieje w nowym widoku, clamp do zakresu.
    pub(crate) fn restore_selection_by_id(&mut self) {
        let rows = self.visible_rows();
        if let Some(ref id) = self.selected_id {
            if let Some(pos) = rows.iter().position(|r| &r.id == id) {
                self.tree_state.selected = pos;
            } else {
                self.tree_state.clamp(rows.len());
                self.selected_id = rows.get(self.tree_state.selected).map(|r| r.id.clone());
            }
        }
    }

    /// Pobierz widoczne wiersze (po flatten, filtrze i sortowaniu).
    pub(crate) fn visible_rows(&self) -> Vec<FlatRow> {
        let mut rows = task_tree::flatten_nodes(&self.tasks.tasks, &self.tree_state.expanded);

        // Filtrowanie — dopasuj ID, nazwę, komponent lub status (case-insensitive)
        if !self.filter.is_empty() {
            let filter_lower = self.filter.to_lowercase();
            rows.retain(|row| {
                row.id.to_lowercase().contains(&filter_lower)
                    || row.name.to_lowercase().contains(&filter_lower)
                    || row
                        .component
                        .as_deref()
                        .is_some_and(|c| c.to_lowercase().contains(&filter_lower))
                    || row.status.as_ref().is_some_and(|s| {
                        let status_text = match s {
                            TaskStatus::Done => "done",
                            TaskStatus::InProgress => "in_progress progress",
                            TaskStatus::Todo => "todo",
                            TaskStatus::Blocked => "blocked",
                        };
                        status_text.contains(&*filter_lower)
                    })
            });
        }

        // Sortowanie — tylko sibling-level (zachowujemy grupowanie po depth)
        match self.sort_mode {
            SortMode::Id => {} // Domyślna kolejność z YAML
            SortMode::Status => {
                sort_siblings_by(&mut rows, |a, b| {
                    status_sort_key(&a.status).cmp(&status_sort_key(&b.status))
                });
            }
            SortMode::Component => {
                sort_siblings_by(&mut rows, |a, b| {
                    let ca = a.component.as_deref().unwrap_or("");
                    let cb = b.component.as_deref().unwrap_or("");
                    ca.cmp(cb)
                });
            }
        }

        rows
    }

    /// Pobierz wybrany node (jeśli istnieje selected_id).
    pub(crate) fn selected_node(&self) -> Option<&TaskNode> {
        self.selected_id
            .as_deref()
            .and_then(|id| self.tasks.find_node(id))
    }

    /// Oblicz podsumowanie postępu (done/total).
    pub(crate) fn progress_counts(&self) -> (usize, usize) {
        let summary = self.tasks.to_summary();
        (summary.done, summary.total())
    }

    /// Rozwiń wszystkie nodes rekursywnie.
    pub(crate) fn expand_all(&mut self) {
        self.tree_state.expanded = self.tasks.all_ids();
    }
}

/// Sortuj sibling rows (wiersze z tym samym depth) zachowując hierarchię.
///
/// Wiersze na głębszym level niż rodzic trzymane są razem z rodzicem.
/// Sortowanie odbywa się stabilnie w grupach sibling — np. depth=0 sortowane
/// razem, ich dzieci (depth=1) sortowane w ramach swojego parenta itd.
fn sort_siblings_by(rows: &mut Vec<FlatRow>, cmp: impl Fn(&FlatRow, &FlatRow) -> CmpOrdering) {
    if rows.len() <= 1 {
        return;
    }

    // Zbierz grupy na depth=0 (top-level siblings), posortuj, złóż z powrotem
    let groups = split_into_groups(rows, 0);
    let mut sorted_groups = groups;
    sorted_groups.sort_by(|a, b| cmp(&a[0], &b[0]));

    rows.clear();
    for mut group in sorted_groups {
        // Rekursywnie sortuj children w ramach każdej grupy
        if group.len() > 1 {
            let mut children: Vec<FlatRow> = group.drain(1..).collect();
            sort_children_recursive(&mut children, group[0].depth + 1, &cmp);
            rows.push(group.into_iter().next().unwrap());
            rows.extend(children);
        } else {
            rows.extend(group);
        }
    }
}

/// Rekursywne sortowanie children na podanym depth level.
fn sort_children_recursive(
    rows: &mut Vec<FlatRow>,
    target_depth: usize,
    cmp: &impl Fn(&FlatRow, &FlatRow) -> CmpOrdering,
) {
    if rows.len() <= 1 {
        return;
    }

    let groups = split_into_groups(rows, target_depth);
    let mut sorted_groups = groups;
    sorted_groups.sort_by(|a, b| cmp(&a[0], &b[0]));

    rows.clear();
    for mut group in sorted_groups {
        if group.len() > 1 {
            let mut children: Vec<FlatRow> = group.drain(1..).collect();
            sort_children_recursive(&mut children, target_depth + 1, cmp);
            rows.push(group.into_iter().next().unwrap());
            rows.extend(children);
        } else {
            rows.extend(group);
        }
    }
}

/// Podziel wiersze na grupy — każda grupa zaczyna się wierszem o `target_depth`,
/// następne elementy grupy mają depth > target_depth (są potomkami).
fn split_into_groups(rows: &[FlatRow], target_depth: usize) -> Vec<Vec<FlatRow>> {
    let mut groups: Vec<Vec<FlatRow>> = Vec::new();
    for row in rows {
        if row.depth == target_depth {
            groups.push(vec![row.clone()]);
        } else if let Some(last_group) = groups.last_mut() {
            last_group.push(row.clone());
        }
    }
    groups
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::KeybindingResolver;
    use crate::tui::app::AppState;
    use crate::tui::events::{AppEvent, EventResult};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn sample_yaml() -> &'static str {
        r#"
default_model: claude-sonnet-4-5-20250929

tasks:
  - id: "1"
    name: "Epic One"
    component: parser
    subtasks:
      - id: "1.1"
        name: "Subtask A"
        status: done
        component: parser
        description: "Parse frontmatter section"
        related_files:
          - "src/parser.rs"
        implementation_steps:
          - "Define struct"
          - "Implement parser"
      - id: "1.2"
        name: "Subtask B"
        status: in_progress
        component: parser
        deps: ["1.1"]
  - id: "2"
    name: "Epic Two"
    component: dag
    subtasks:
      - id: "2.1"
        name: "Cycle detect"
        status: todo
        component: dag
      - id: "2.2"
        name: "Topo sort"
        status: blocked
        component: dag
        deps: ["2.1", "1.2"]
        profiles: ["backend"]
"#
    }

    fn make_app() -> TaskExplorerApp {
        let tasks: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let mut expanded = HashSet::new();
        for node in &tasks.tasks {
            expanded.insert(node.id.clone());
        }
        let tree_state = TreeState {
            expanded,
            selected: 0,
            scroll_offset: 0,
        };
        let rows = task_tree::flatten_nodes(&tasks.tasks, &tree_state.expanded);
        let selected_id = rows.first().map(|r| r.id.clone());

        TaskExplorerApp {
            tasks,
            tasks_path: PathBuf::from(".ralph/tasks.yml"),
            tree_state,
            selected_id,
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Id,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
        }
    }

    fn press(code: KeyCode) -> AppEvent {
        AppEvent::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    // ── Panel & SortMode tests ──

    #[test]
    fn panel_default_is_tree() {
        let app = make_app();
        assert_eq!(app.focus, Panel::Tree);
    }

    #[test]
    fn sort_mode_cycles() {
        assert_eq!(SortMode::Id.next(), SortMode::Status);
        assert_eq!(SortMode::Status.next(), SortMode::Component);
        assert_eq!(SortMode::Component.next(), SortMode::Id);
    }

    #[test]
    fn sort_mode_labels() {
        assert_eq!(SortMode::Id.label(), "ID");
        assert_eq!(SortMode::Status.label(), "Status");
        assert_eq!(SortMode::Component.label(), "Component");
    }

    // ── Navigation tests ──

    #[test]
    fn navigate_down_changes_selection() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(app.selected_id, Some("1".to_string()));

        app.handle_event(press(KeyCode::Down), &resolver);
        assert_eq!(app.selected_id, Some("1.1".to_string()));

        app.handle_event(press(KeyCode::Down), &resolver);
        assert_eq!(app.selected_id, Some("1.2".to_string()));
    }

    #[test]
    fn navigate_up_changes_selection() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.handle_event(press(KeyCode::Down), &resolver);
        app.handle_event(press(KeyCode::Down), &resolver);
        assert_eq!(app.selected_id, Some("1.2".to_string()));

        app.handle_event(press(KeyCode::Up), &resolver);
        assert_eq!(app.selected_id, Some("1.1".to_string()));
    }

    #[test]
    fn navigate_up_at_top_stays() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.handle_event(press(KeyCode::Up), &resolver);
        assert_eq!(app.selected_id, Some("1".to_string()));
    }

    #[test]
    fn vim_keys_navigate() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.handle_event(press(KeyCode::Char('j')), &resolver);
        assert_eq!(app.selected_id, Some("1.1".to_string()));

        app.handle_event(press(KeyCode::Char('k')), &resolver);
        assert_eq!(app.selected_id, Some("1".to_string()));
    }

    #[test]
    fn home_goes_to_first() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.handle_event(press(KeyCode::Down), &resolver);
        app.handle_event(press(KeyCode::Down), &resolver);
        app.handle_event(press(KeyCode::Home), &resolver);
        assert_eq!(app.selected_id, Some("1".to_string()));
    }

    #[test]
    fn end_goes_to_last() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.handle_event(press(KeyCode::End), &resolver);
        // Root nodes expanded: 1, 1.1, 1.2, 2, 2.1, 2.2 — last is "2.2"
        assert_eq!(app.selected_id, Some("2.2".to_string()));
    }

    // ── Focus switching ──

    #[test]
    fn tab_switches_focus() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(app.focus, Panel::Tree);

        app.handle_event(press(KeyCode::Tab), &resolver);
        assert_eq!(app.focus, Panel::Detail);

        app.handle_event(press(KeyCode::Tab), &resolver);
        assert_eq!(app.focus, Panel::Tree);
    }

    #[test]
    fn enter_on_leaf_switches_to_detail() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.handle_event(press(KeyCode::Down), &resolver);
        assert_eq!(app.selected_id, Some("1.1".to_string()));

        app.handle_event(press(KeyCode::Enter), &resolver);
        assert_eq!(app.focus, Panel::Detail);
    }

    #[test]
    fn left_in_detail_goes_back_to_tree() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.focus = Panel::Detail;

        app.handle_event(press(KeyCode::Left), &resolver);
        assert_eq!(app.focus, Panel::Tree);
    }

    // ── Expand/Collapse ──

    #[test]
    fn collapse_all_then_expand_all() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        let initial_rows = app.visible_rows().len();
        assert!(initial_rows > 2);

        app.handle_event(press(KeyCode::Char('c')), &resolver);
        let collapsed_rows = app.visible_rows().len();
        assert_eq!(collapsed_rows, 2);

        app.handle_event(press(KeyCode::Char('e')), &resolver);
        let expanded_rows = app.visible_rows().len();
        assert!(expanded_rows >= initial_rows);
    }

    #[test]
    fn enter_on_parent_toggles_expand() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.tree_state.expanded.clear();
        let rows_before = app.visible_rows().len();
        assert_eq!(rows_before, 2);

        app.handle_event(press(KeyCode::Enter), &resolver);
        let rows_after = app.visible_rows().len();
        assert!(rows_after > rows_before);
    }

    // ── Sort mode toggle ──

    #[test]
    fn s_key_cycles_sort_mode() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(app.sort_mode, SortMode::Id);

        app.handle_event(press(KeyCode::Char('s')), &resolver);
        assert_eq!(app.sort_mode, SortMode::Status);

        app.handle_event(press(KeyCode::Char('s')), &resolver);
        assert_eq!(app.sort_mode, SortMode::Component);

        app.handle_event(press(KeyCode::Char('s')), &resolver);
        assert_eq!(app.sort_mode, SortMode::Id);
    }

    #[test]
    fn s_key_preserves_selection_after_sort_change() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        // Navigate to "1.2"
        app.handle_event(press(KeyCode::Down), &resolver); // → 1.1
        app.handle_event(press(KeyCode::Down), &resolver); // → 1.2
        assert_eq!(app.selected_id, Some("1.2".to_string()));

        // Change sort mode — selected_id should stay on "1.2"
        app.handle_event(press(KeyCode::Char('s')), &resolver);
        assert_eq!(app.sort_mode, SortMode::Status);
        assert_eq!(app.selected_id, Some("1.2".to_string()));

        // Again
        app.handle_event(press(KeyCode::Char('s')), &resolver);
        assert_eq!(app.sort_mode, SortMode::Component);
        assert_eq!(app.selected_id, Some("1.2".to_string()));
    }

    // ── Sort mode actually applies ──

    #[test]
    fn sort_by_status_reorders_siblings() {
        let mut app = make_app();
        app.sort_mode = SortMode::Status;

        let rows = app.visible_rows();
        // Depth-0: "1" (no status) and "2" (no status) — both None, stable
        // Depth-1 under "1": "1.1" done (prio 0) before "1.2" in_progress (prio 1)
        let epic1_children: Vec<&FlatRow> = rows
            .iter()
            .filter(|r| r.depth == 1 && r.id.starts_with("1."))
            .collect();
        if epic1_children.len() == 2 {
            assert_eq!(epic1_children[0].id, "1.1"); // done = 0
            assert_eq!(epic1_children[1].id, "1.2"); // in_progress = 1
        }
    }

    #[test]
    fn sort_by_component_reorders_siblings() {
        // Setup z mixed components na tym samym depth
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "1"
    name: "Zebra epic"
    component: zebra
    subtasks:
      - id: "1.1"
        name: "Z-child"
        status: todo
        component: zebra
  - id: "2"
    name: "Alpha epic"
    component: alpha
    subtasks:
      - id: "2.1"
        name: "A-child"
        status: todo
        component: alpha
"#;
        let tasks: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let app = TaskExplorerApp {
            tasks,
            tasks_path: PathBuf::from("test.yml"),
            tree_state: TreeState {
                expanded: HashSet::from(["1".to_string(), "2".to_string()]),
                selected: 0,
                scroll_offset: 0,
            },
            selected_id: Some("1".to_string()),
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Component,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
        };

        let rows = app.visible_rows();
        // depth=0 siblings sorted by component: alpha < zebra
        assert_eq!(rows[0].id, "2"); // alpha
        assert_eq!(rows[1].id, "2.1"); // alpha's child
        assert_eq!(rows[2].id, "1"); // zebra
        assert_eq!(rows[3].id, "1.1"); // zebra's child
    }

    // ── Filter tests ──

    #[test]
    fn filter_narrows_visible_rows() {
        let mut app = make_app();
        app.filter = "Cycle".to_string();

        let rows = app.visible_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "2.1");
    }

    #[test]
    fn filter_is_case_insensitive() {
        let mut app = make_app();
        app.filter = "cycle".to_string();

        let rows = app.visible_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "2.1");
    }

    #[test]
    fn filter_by_component() {
        let mut app = make_app();
        app.filter = "dag".to_string();

        let rows = app.visible_rows();
        // Matching: "2" (component=dag), "2.1" (component=dag), "2.2" (component=dag)
        assert!(
            rows.iter()
                .all(|r| r.component.as_deref() == Some("dag") || r.id == "2")
        );
    }

    #[test]
    fn empty_filter_shows_all() {
        let mut app = make_app();
        app.filter = String::new();

        let rows = app.visible_rows();
        // All expanded: 1, 1.1, 1.2, 2, 2.1, 2.2
        assert_eq!(rows.len(), 6);
    }

    // ── Detail scroll ──

    #[test]
    fn detail_scroll_increments() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.focus = Panel::Detail;

        // scroll_step domyślnie = 3; detail_scroll ograniczony do detail_line_count-1
        assert_eq!(app.detail_scroll, 0);
        app.handle_event(press(KeyCode::Down), &resolver);
        // Offset rośnie o scroll_step=3 (ale jest clampowany do max_scroll)
        // Weryfikujemy że offset > 0 (aktualna wartość zależy od liczby linii w testowym tasku)
        assert!(app.detail_scroll > 0);
    }

    #[test]
    fn detail_scroll_does_not_go_below_zero() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.focus = Panel::Detail;

        app.handle_event(press(KeyCode::Up), &resolver);
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn detail_home_resets_scroll() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.focus = Panel::Detail;
        app.detail_scroll = 5;

        app.handle_event(press(KeyCode::Home), &resolver);
        assert_eq!(app.detail_scroll, 0);
    }

    // ── Selected node ──

    #[test]
    fn selected_node_returns_correct_node() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.handle_event(press(KeyCode::Down), &resolver);
        let node = app.selected_node().unwrap();
        assert_eq!(node.id, "1.1");
        assert_eq!(node.name, "Subtask A");
    }

    #[test]
    fn selected_node_on_empty_tree_is_none() {
        let tasks = TasksFile {
            default_model: None,
            tasks: vec![],
        };
        let app = TaskExplorerApp {
            tasks,
            tasks_path: PathBuf::from("test.yml"),
            tree_state: TreeState::default(),
            selected_id: None,
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Id,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
        };
        assert!(app.selected_node().is_none());
    }

    // ── Progress counts ──

    #[test]
    fn progress_counts_correct() {
        let app = make_app();
        let (done, total) = app.progress_counts();
        assert_eq!(done, 1); // 1.1 is done
        assert_eq!(total, 4); // 1.1, 1.2, 2.1, 2.2
    }

    // ── EventResult tests ──

    #[test]
    fn tick_returns_ignored() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(
            app.handle_event(AppEvent::Tick, &resolver),
            EventResult::Ignored
        );
    }

    #[test]
    fn resize_returns_consumed() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(
            app.handle_event(AppEvent::Resize(80, 24), &resolver),
            EventResult::Consumed
        );
    }

    #[test]
    fn unknown_key_returns_ignored() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(press(KeyCode::F(12)), &resolver);
        assert_eq!(result, EventResult::Ignored);
    }

    #[test]
    fn esc_in_tree_returns_quit() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.focus = Panel::Tree;
        let result = app.handle_event(press(KeyCode::Esc), &resolver);
        assert_eq!(result, EventResult::Quit);
    }

    #[test]
    fn esc_in_detail_returns_to_tree() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.focus = Panel::Detail;
        let result = app.handle_event(press(KeyCode::Esc), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focus, Panel::Tree);
    }

    #[test]
    fn double_esc_from_detail_quits() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.focus = Panel::Detail;

        // Pierwszy Esc: wraca do Tree
        let result = app.handle_event(press(KeyCode::Esc), &resolver);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focus, Panel::Tree);

        // Drugi Esc: quit
        let result = app.handle_event(press(KeyCode::Esc), &resolver);
        assert_eq!(result, EventResult::Quit);
    }

    // ── Sync selected_id ──

    #[test]
    fn sync_resets_detail_scroll() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.detail_scroll = 5;
        app.handle_event(press(KeyCode::Down), &resolver);
        assert_eq!(app.detail_scroll, 0);
    }

    // ── AppState trait compliance ──

    #[test]
    fn app_state_is_object_safe() {
        fn accept(_: &dyn AppState) {}
        let app = make_app();
        accept(&app);
    }

    // ── InputMode tests ──

    #[test]
    fn input_mode_default_is_normal() {
        let app = make_app();
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn f_key_activates_filter_mode() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        assert_eq!(app.input_mode, InputMode::Normal);

        app.handle_event(press(KeyCode::Char('f')), &resolver);
        assert_eq!(app.input_mode, InputMode::Filter);
    }

    #[test]
    fn esc_in_filter_mode_clears_filter_and_returns_to_normal() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.input_mode = InputMode::Filter;
        app.filter = "test".to_string();

        app.handle_event(press(KeyCode::Esc), &resolver);
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.filter, "");
    }

    #[test]
    fn enter_in_filter_mode_applies_filter_and_returns_to_normal() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.input_mode = InputMode::Filter;
        app.filter = "test".to_string();

        app.handle_event(press(KeyCode::Enter), &resolver);
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.filter, "test");
    }

    #[test]
    fn typing_in_filter_mode_updates_filter() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.input_mode = InputMode::Filter;

        app.handle_event(press(KeyCode::Char('t')), &resolver);
        app.handle_event(press(KeyCode::Char('e')), &resolver);
        app.handle_event(press(KeyCode::Char('s')), &resolver);
        app.handle_event(press(KeyCode::Char('t')), &resolver);

        assert_eq!(app.filter, "test");
    }

    #[test]
    fn backspace_in_filter_mode_removes_last_char() {
        let mut app = make_app();
        let resolver = KeybindingResolver::with_defaults();
        app.input_mode = InputMode::Filter;
        app.filter = "test".to_string();

        app.handle_event(press(KeyCode::Backspace), &resolver);
        assert_eq!(app.filter, "tes");

        app.handle_event(press(KeyCode::Backspace), &resolver);
        assert_eq!(app.filter, "te");
    }

    #[test]
    fn filter_by_status_text() {
        let mut app = make_app();
        app.filter = "progress".to_string();

        let rows = app.visible_rows();
        // Tylko "1.2" ma status in_progress
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "1.2");
    }

    #[test]
    fn filter_matches_status_done() {
        let mut app = make_app();
        app.filter = "done".to_string();

        let rows = app.visible_rows();
        // "1.1" ma status done
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "1.1");
    }

    #[test]
    fn filter_matches_status_blocked() {
        let mut app = make_app();
        app.filter = "blocked".to_string();

        let rows = app.visible_rows();
        // "2.2" ma status blocked
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "2.2");
    }
}
