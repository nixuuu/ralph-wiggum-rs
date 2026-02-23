//! Konfigurowalny system keybindingów.
//!
//! Definiuje schemat skrótów klawiszowych w TOML i odpowiadające typy Rust.
//! Sekcje: `[keybindings.global]`, `[keybindings.orchestrate]`, `[keybindings.run]`,
//! `[keybindings.explorer]`.
//!
//! Format TOML:
//! ```toml
//! [keybindings.global]
//! quit = "q"
//! toggle_sidebar = "t"
//! command_palette = "Ctrl+p"
//! ```

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// ── KeyCombo ─────────────────────────────────────────────────────────

/// Kombinacja klawiszy: klawisz bazowy + modyfikatory (Ctrl, Shift, Alt).
///
/// Parsowana z formatu stringa: `"Ctrl+p"`, `"Shift+Tab"`, `"Up"`, `"q"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub key: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombo {
    pub const fn new(key: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { key, modifiers }
    }

    /// Sprawdza czy KeyEvent dokładnie pasuje do tego combo.
    ///
    /// Porównanie jest ścisłe: zarówno key code jak i modyfikatory muszą się zgadzać.
    pub fn matches(&self, event: &KeyEvent) -> bool {
        event.code == self.key && event.modifiers == self.modifiers
    }
}

/// Parser: String → KeyCombo.
///
/// Format: `[Modifier+]...Key`
/// - Modyfikatory: `Ctrl`, `Shift`, `Alt` (case-insensitive)
/// - Klucze: single char (`q`, `R`), named (`Up`, `Down`, `Enter`, `Esc`, `Tab`,
///   `Space`, `Backspace`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, `F1`-`F12`)
/// - Normalizacja: `Shift+Tab` → `BackTab` z modyfikatorem SHIFT
impl FromStr for KeyCombo {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err("Empty key binding string".to_string());
        }

        // Special case: just "+" character
        if s == "+" {
            return Ok(KeyCombo::new(KeyCode::Char('+'), KeyModifiers::NONE));
        }

        let parts: Vec<&str> = s.split('+').collect();
        let mut modifiers = KeyModifiers::NONE;

        // Wszystko przed ostatnim segmentem to modyfikatory
        for part in &parts[..parts.len() - 1] {
            match part.trim().to_lowercase().as_str() {
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                "alt" => modifiers |= KeyModifiers::ALT,
                other => return Err(format!("Unknown modifier: '{}'", other)),
            }
        }

        let key_str = parts.last().unwrap().trim();
        if key_str.is_empty() {
            return Err("Missing key after modifier".to_string());
        }

        let mut key = parse_key_code(key_str)?;

        // Normalizacja: Shift+Tab → BackTab (crossterm convention)
        if key == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
            key = KeyCode::BackTab;
        }

        Ok(KeyCombo { key, modifiers })
    }
}

/// Parsuj nazwę klawisza na KeyCode.
fn parse_key_code(s: &str) -> std::result::Result<KeyCode, String> {
    // Named keys (case-insensitive)
    match s.to_lowercase().as_str() {
        "up" => return Ok(KeyCode::Up),
        "down" => return Ok(KeyCode::Down),
        "left" => return Ok(KeyCode::Left),
        "right" => return Ok(KeyCode::Right),
        "enter" | "return" => return Ok(KeyCode::Enter),
        "esc" | "escape" => return Ok(KeyCode::Esc),
        "tab" => return Ok(KeyCode::Tab),
        "backtab" => return Ok(KeyCode::BackTab),
        "space" => return Ok(KeyCode::Char(' ')),
        "backspace" => return Ok(KeyCode::Backspace),
        "delete" | "del" => return Ok(KeyCode::Delete),
        "home" => return Ok(KeyCode::Home),
        "end" => return Ok(KeyCode::End),
        "pageup" => return Ok(KeyCode::PageUp),
        "pagedown" => return Ok(KeyCode::PageDown),
        "plus" => return Ok(KeyCode::Char('+')),
        _ => {}
    }

    // Single character (case-sensitive)
    let chars: Vec<char> = s.chars().collect();
    if chars.len() == 1 {
        return Ok(KeyCode::Char(chars[0]));
    }

    // F-keys: F1-F12
    if let Some(rest) = s.strip_prefix('F').or_else(|| s.strip_prefix('f'))
        && let Ok(num) = rest.parse::<u8>()
        && (1..=12).contains(&num)
    {
        return Ok(KeyCode::F(num));
    }

    Err(format!("Unknown key: '{}'", s))
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            write!(f, "Ctrl+")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            write!(f, "Shift+")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            write!(f, "Alt+")?;
        }
        match self.key {
            KeyCode::Char(' ') => write!(f, "Space"),
            KeyCode::Char(c) => write!(f, "{}", c),
            KeyCode::Up => write!(f, "Up"),
            KeyCode::Down => write!(f, "Down"),
            KeyCode::Left => write!(f, "Left"),
            KeyCode::Right => write!(f, "Right"),
            KeyCode::Enter => write!(f, "Enter"),
            KeyCode::Esc => write!(f, "Esc"),
            KeyCode::Tab => write!(f, "Tab"),
            KeyCode::BackTab => write!(f, "Tab"), // BackTab = Shift+Tab; "Shift+" already printed
            KeyCode::Backspace => write!(f, "Backspace"),
            KeyCode::Delete => write!(f, "Delete"),
            KeyCode::Home => write!(f, "Home"),
            KeyCode::End => write!(f, "End"),
            KeyCode::PageUp => write!(f, "PageUp"),
            KeyCode::PageDown => write!(f, "PageDown"),
            KeyCode::F(n) => write!(f, "F{}", n),
            _ => write!(f, "?"),
        }
    }
}

/// Custom serde serializer — serializuje KeyCombo do stringa (via Display).
impl Serialize for KeyCombo {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Custom serde deserializer — parsuje KeyCombo ze stringa TOML.
impl<'de> Deserialize<'de> for KeyCombo {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ── KeyAction ────────────────────────────────────────────────────────

/// Wszystkie możliwe akcje klawiszowe w aplikacji.
///
/// Warianty pogrupowane semantycznie: globalne, orchestrate, run, explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    // ── Global (wspólne dla wszystkich widoków) ──
    Quit,
    ForceQuit,
    Cancel,
    Confirm,
    ToggleSidebar,
    ScrollUp,
    ScrollDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,
    SwitchFocus,
    CommandPalette,
    ShrinkSidebar,
    GrowSidebar,

    // ── Orchestrate ──
    FocusNext,
    FocusPrev,
    TogglePreview,
    SendMessage,
    Reload,
    Restart,
    ConfirmRestart,
    CancelRestart,
    ToggleIdleWorkers,

    // ── Run ──
    ToggleExpand,

    // ── Explorer ──
    CycleSort,
    ReloadTasks,
    EnterFilter,
    ExpandAll,
    CollapseAll,
    VimUp,
    VimDown,
    VimLeft,
    VimRight,
    ExpandOrEnter,
}

// ── Per-section binding structs ──────────────────────────────────────

/// Globalne skróty klawiszowe (dostępne we wszystkich widokach).
///
/// Odpowiada sekcji `[keybindings.global]` w .ralph.toml.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct GlobalBindings {
    pub quit: KeyCombo,
    pub force_quit: KeyCombo,
    pub cancel: KeyCombo,
    pub confirm: KeyCombo,
    pub toggle_sidebar: KeyCombo,
    pub scroll_up: KeyCombo,
    pub scroll_down: KeyCombo,
    pub scroll_page_up: KeyCombo,
    pub scroll_page_down: KeyCombo,
    pub scroll_to_top: KeyCombo,
    pub scroll_to_bottom: KeyCombo,
    pub switch_focus: KeyCombo,
    pub command_palette: KeyCombo,
    pub shrink_sidebar: KeyCombo,
    pub grow_sidebar: KeyCombo,
}

impl Default for GlobalBindings {
    fn default() -> Self {
        Self {
            quit: KeyCombo::new(KeyCode::Char('q'), KeyModifiers::NONE),
            force_quit: KeyCombo::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            cancel: KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE),
            confirm: KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE),
            toggle_sidebar: KeyCombo::new(KeyCode::Char('t'), KeyModifiers::NONE),
            scroll_up: KeyCombo::new(KeyCode::Up, KeyModifiers::NONE),
            scroll_down: KeyCombo::new(KeyCode::Down, KeyModifiers::NONE),
            scroll_page_up: KeyCombo::new(KeyCode::PageUp, KeyModifiers::NONE),
            scroll_page_down: KeyCombo::new(KeyCode::PageDown, KeyModifiers::NONE),
            scroll_to_top: KeyCombo::new(KeyCode::Home, KeyModifiers::NONE),
            scroll_to_bottom: KeyCombo::new(KeyCode::End, KeyModifiers::NONE),
            switch_focus: KeyCombo::new(KeyCode::Tab, KeyModifiers::NONE),
            command_palette: KeyCombo::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            shrink_sidebar: KeyCombo::new(KeyCode::Char('['), KeyModifiers::NONE),
            grow_sidebar: KeyCombo::new(KeyCode::Char(']'), KeyModifiers::NONE),
        }
    }
}

/// Orchestrate-specific skróty klawiszowe.
///
/// Odpowiada sekcji `[keybindings.orchestrate]` w .ralph.toml.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OrchestrateBindings {
    pub focus_next: KeyCombo,
    pub focus_prev: KeyCombo,
    pub toggle_preview: KeyCombo,
    pub send_message: KeyCombo,
    pub reload: KeyCombo,
    pub restart: KeyCombo,
    pub confirm_restart: KeyCombo,
    pub cancel_restart: KeyCombo,
    pub toggle_idle_workers: KeyCombo,
}

impl Default for OrchestrateBindings {
    fn default() -> Self {
        Self {
            focus_next: KeyCombo::new(KeyCode::Tab, KeyModifiers::NONE),
            focus_prev: KeyCombo::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            toggle_preview: KeyCombo::new(KeyCode::Char('p'), KeyModifiers::NONE),
            send_message: KeyCombo::new(KeyCode::Char('i'), KeyModifiers::NONE),
            reload: KeyCombo::new(KeyCode::Char('r'), KeyModifiers::NONE),
            restart: KeyCombo::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
            confirm_restart: KeyCombo::new(KeyCode::Char('y'), KeyModifiers::NONE),
            cancel_restart: KeyCombo::new(KeyCode::Char('n'), KeyModifiers::NONE),
            toggle_idle_workers: KeyCombo::new(KeyCode::Char('h'), KeyModifiers::NONE),
        }
    }
}

/// Run-specific skróty klawiszowe.
///
/// Odpowiada sekcji `[keybindings.run]` w .ralph.toml.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct RunBindings {
    pub toggle_expand: KeyCombo,
}

impl Default for RunBindings {
    fn default() -> Self {
        Self {
            toggle_expand: KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE),
        }
    }
}

/// Explorer-specific skróty klawiszowe.
///
/// Odpowiada sekcji `[keybindings.explorer]` w .ralph.toml.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ExplorerBindings {
    pub cycle_sort: KeyCombo,
    pub reload_tasks: KeyCombo,
    pub enter_filter: KeyCombo,
    pub expand_all: KeyCombo,
    pub collapse_all: KeyCombo,
    pub expand_or_enter: KeyCombo,
    pub vim_up: KeyCombo,
    pub vim_down: KeyCombo,
    pub vim_left: KeyCombo,
    pub vim_right: KeyCombo,
}

impl Default for ExplorerBindings {
    fn default() -> Self {
        Self {
            cycle_sort: KeyCombo::new(KeyCode::Char('s'), KeyModifiers::NONE),
            reload_tasks: KeyCombo::new(KeyCode::Char('r'), KeyModifiers::NONE),
            enter_filter: KeyCombo::new(KeyCode::Char('f'), KeyModifiers::NONE),
            expand_all: KeyCombo::new(KeyCode::Char('e'), KeyModifiers::NONE),
            collapse_all: KeyCombo::new(KeyCode::Char('c'), KeyModifiers::NONE),
            expand_or_enter: KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE),
            vim_up: KeyCombo::new(KeyCode::Char('k'), KeyModifiers::NONE),
            vim_down: KeyCombo::new(KeyCode::Char('j'), KeyModifiers::NONE),
            vim_left: KeyCombo::new(KeyCode::Char('h'), KeyModifiers::NONE),
            vim_right: KeyCombo::new(KeyCode::Char('l'), KeyModifiers::NONE),
        }
    }
}

// ── KeybindingsConfig ────────────────────────────────────────────────

/// Główna struktura konfiguracji keybindingów.
///
/// Odpowiada sekcji `[keybindings]` w .ralph.toml z podsekcjami:
/// `[keybindings.global]`, `[keybindings.orchestrate]`, `[keybindings.run]`,
/// `[keybindings.explorer]`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    pub global: GlobalBindings,
    pub orchestrate: OrchestrateBindings,
    pub run: RunBindings,
    pub explorer: ExplorerBindings,
}

// ── Merge ────────────────────────────────────────────────────────────

/// Merge helper: overlay nadpisuje base gdy overlay ≠ default.
fn pick<T: PartialEq>(base: T, overlay: T, default: T) -> T {
    if overlay != default { overlay } else { base }
}

/// Macro do merge per-field — redukuje boilerplate w sekcjach z wieloma polami.
macro_rules! merge_bindings {
    ($base:expr, $overlay:expr, $default:expr; $($field:ident),+ $(,)?) => {
        Self {
            $($field: pick($base.$field, $overlay.$field, $default.$field)),+
        }
    };
}

impl GlobalBindings {
    pub(crate) fn merge(base: Self, overlay: Self) -> Self {
        let d = Self::default();
        merge_bindings!(base, overlay, d;
            quit, force_quit, cancel, confirm,
            toggle_sidebar, scroll_up, scroll_down,
            scroll_page_up, scroll_page_down,
            scroll_to_top, scroll_to_bottom,
            switch_focus, command_palette,
            shrink_sidebar, grow_sidebar,
        )
    }
}

impl OrchestrateBindings {
    pub(crate) fn merge(base: Self, overlay: Self) -> Self {
        let d = Self::default();
        merge_bindings!(base, overlay, d;
            focus_next, focus_prev, toggle_preview,
            send_message, reload, restart,
            confirm_restart, cancel_restart,
            toggle_idle_workers,
        )
    }
}

impl RunBindings {
    pub(crate) fn merge(base: Self, overlay: Self) -> Self {
        let d = Self::default();
        merge_bindings!(base, overlay, d; toggle_expand)
    }
}

impl ExplorerBindings {
    pub(crate) fn merge(base: Self, overlay: Self) -> Self {
        let d = Self::default();
        merge_bindings!(base, overlay, d;
            cycle_sort, reload_tasks, enter_filter,
            expand_all, collapse_all, expand_or_enter,
            vim_up, vim_down, vim_left, vim_right,
        )
    }
}

impl KeybindingsConfig {
    /// Merge dwóch warstw keybindingów: overlay nadpisuje base (non-default only).
    pub(crate) fn merge(base: Self, overlay: Self) -> Self {
        Self {
            global: GlobalBindings::merge(base.global, overlay.global),
            orchestrate: OrchestrateBindings::merge(base.orchestrate, overlay.orchestrate),
            run: RunBindings::merge(base.run, overlay.run),
            explorer: ExplorerBindings::merge(base.explorer, overlay.explorer),
        }
    }
}

// ── Resolve: KeyEvent → KeyAction ────────────────────────────────────

/// Macro do generowania resolve() — mapuje pola struct na warianty KeyAction.
macro_rules! resolve_bindings {
    ($self:expr, $event:expr; $($field:ident => $action:expr),+ $(,)?) => {{
        $(if $self.$field.matches($event) { return Some($action); })+
        None
    }};
}

/// Macro do generowania pairs() — zwraca wszystkie pary (KeyCombo, KeyAction).
///
/// Używane przez `KeybindingResolver::key_for_action()` do reverse lookup.
macro_rules! impl_pairs {
    ($t:ty; $($field:ident => $action:expr),+ $(,)?) => {
        impl $t {
            /// Zwraca wszystkie pary (KeyCombo, KeyAction) dla tej sekcji bindingów.
            pub fn pairs(&self) -> Vec<(KeyCombo, KeyAction)> {
                vec![$( (self.$field.clone(), $action) ),+]
            }
        }
    };
}

impl GlobalBindings {
    /// Rozwiąż KeyEvent na globalną KeyAction, jeśli pasuje do któregoś bindingu.
    pub fn resolve(&self, event: &KeyEvent) -> Option<KeyAction> {
        resolve_bindings!(self, event;
            quit => KeyAction::Quit,
            force_quit => KeyAction::ForceQuit,
            cancel => KeyAction::Cancel,
            confirm => KeyAction::Confirm,
            toggle_sidebar => KeyAction::ToggleSidebar,
            scroll_up => KeyAction::ScrollUp,
            scroll_down => KeyAction::ScrollDown,
            scroll_page_up => KeyAction::ScrollPageUp,
            scroll_page_down => KeyAction::ScrollPageDown,
            scroll_to_top => KeyAction::ScrollToTop,
            scroll_to_bottom => KeyAction::ScrollToBottom,
            switch_focus => KeyAction::SwitchFocus,
            command_palette => KeyAction::CommandPalette,
            shrink_sidebar => KeyAction::ShrinkSidebar,
            grow_sidebar => KeyAction::GrowSidebar,
        )
    }
}

impl OrchestrateBindings {
    /// Rozwiąż KeyEvent na orchestrate-specific KeyAction.
    pub fn resolve(&self, event: &KeyEvent) -> Option<KeyAction> {
        resolve_bindings!(self, event;
            focus_next => KeyAction::FocusNext,
            focus_prev => KeyAction::FocusPrev,
            toggle_preview => KeyAction::TogglePreview,
            send_message => KeyAction::SendMessage,
            reload => KeyAction::Reload,
            restart => KeyAction::Restart,
            confirm_restart => KeyAction::ConfirmRestart,
            cancel_restart => KeyAction::CancelRestart,
            toggle_idle_workers => KeyAction::ToggleIdleWorkers,
        )
    }
}

impl RunBindings {
    /// Rozwiąż KeyEvent na run-specific KeyAction.
    pub fn resolve(&self, event: &KeyEvent) -> Option<KeyAction> {
        resolve_bindings!(self, event;
            toggle_expand => KeyAction::ToggleExpand,
        )
    }
}

impl ExplorerBindings {
    /// Rozwiąż KeyEvent na explorer-specific KeyAction.
    pub fn resolve(&self, event: &KeyEvent) -> Option<KeyAction> {
        resolve_bindings!(self, event;
            cycle_sort => KeyAction::CycleSort,
            reload_tasks => KeyAction::ReloadTasks,
            enter_filter => KeyAction::EnterFilter,
            expand_all => KeyAction::ExpandAll,
            collapse_all => KeyAction::CollapseAll,
            expand_or_enter => KeyAction::ExpandOrEnter,
            vim_up => KeyAction::VimUp,
            vim_down => KeyAction::VimDown,
            vim_left => KeyAction::VimLeft,
            vim_right => KeyAction::VimRight,
        )
    }
}

// ── Pairs: action→combo reverse-lookup ──────────────────────────────

impl_pairs!(GlobalBindings;
    quit => KeyAction::Quit,
    force_quit => KeyAction::ForceQuit,
    cancel => KeyAction::Cancel,
    confirm => KeyAction::Confirm,
    toggle_sidebar => KeyAction::ToggleSidebar,
    scroll_up => KeyAction::ScrollUp,
    scroll_down => KeyAction::ScrollDown,
    scroll_page_up => KeyAction::ScrollPageUp,
    scroll_page_down => KeyAction::ScrollPageDown,
    scroll_to_top => KeyAction::ScrollToTop,
    scroll_to_bottom => KeyAction::ScrollToBottom,
    switch_focus => KeyAction::SwitchFocus,
    command_palette => KeyAction::CommandPalette,
    shrink_sidebar => KeyAction::ShrinkSidebar,
    grow_sidebar => KeyAction::GrowSidebar,
);

impl_pairs!(OrchestrateBindings;
    focus_next => KeyAction::FocusNext,
    focus_prev => KeyAction::FocusPrev,
    toggle_preview => KeyAction::TogglePreview,
    send_message => KeyAction::SendMessage,
    reload => KeyAction::Reload,
    restart => KeyAction::Restart,
    confirm_restart => KeyAction::ConfirmRestart,
    cancel_restart => KeyAction::CancelRestart,
    toggle_idle_workers => KeyAction::ToggleIdleWorkers,
);

impl_pairs!(RunBindings;
    toggle_expand => KeyAction::ToggleExpand,
);

impl_pairs!(ExplorerBindings;
    cycle_sort => KeyAction::CycleSort,
    reload_tasks => KeyAction::ReloadTasks,
    enter_filter => KeyAction::EnterFilter,
    expand_all => KeyAction::ExpandAll,
    collapse_all => KeyAction::CollapseAll,
    expand_or_enter => KeyAction::ExpandOrEnter,
    vim_up => KeyAction::VimUp,
    vim_down => KeyAction::VimDown,
    vim_left => KeyAction::VimLeft,
    vim_right => KeyAction::VimRight,
);

// ── View ─────────────────────────────────────────────────────────────

/// Widok aplikacji — określa kontekst dla resolwowania keybindingów.
///
/// Przekazywany do `KeybindingResolver::resolve()` żeby wybrać właściwe
/// view-specific bindingi przed globalnym fallbackiem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    /// Brak specyficznego widoku — tylko globalne bindingi.
    Global,
    /// Widok orkiestracji (orchestrate).
    Orchestrate,
    /// Widok uruchomienia (run).
    Run,
    /// Widok eksploratora tasków (explorer).
    Explorer,
}

// ── KeybindingResolver ────────────────────────────────────────────────

/// Resolver keybindingów — lookup chain: view-specific → global.
///
/// Zbudowany z `KeybindingsConfig` (zmerge'owanego z FileConfig + defaults).
///
/// # Lookup chain
/// 1. View-specific bindings (orchestrate / run / explorer)
/// 2. Global bindings (dostępne we wszystkich widokach)
///
/// View-specific wygrywa nad globalnym gdy oba pasują do tego samego klawisza.
///
/// # Przykład
/// ```rust,ignore
/// let resolver = KeybindingResolver::new(config);
///
/// // Forward lookup
/// if let Some(action) = resolver.resolve(&key_event, View::Orchestrate) {
///     // obsłuż akcję
/// }
///
/// // Reverse lookup dla hint display
/// if let Some(combo) = resolver.key_for_action(KeyAction::Quit) {
///     println!("Naciśnij {} aby wyjść", KeybindingResolver::format_key(&combo));
/// }
/// ```
#[derive(Debug, Clone)]
pub struct KeybindingResolver {
    config: KeybindingsConfig,
}

impl KeybindingResolver {
    /// Utwórz resolver z loaded config.
    ///
    /// Config powinien być już zmerge'owany z defaults (np. z `FileConfig.keybindings`).
    pub fn new(config: KeybindingsConfig) -> Self {
        Self { config }
    }

    /// Utwórz resolver z samych defaults (bez żadnych customizacji).
    pub fn with_defaults() -> Self {
        Self::new(KeybindingsConfig::default())
    }

    /// Utwórz resolver z customizacji użytkownika nałożonych na defaults.
    ///
    /// `user_config` to keybindings z FileConfig — nadpisują defaults tylko
    /// dla pól które użytkownik faktycznie zmienił (non-default values).
    pub fn from_user_config(user_config: KeybindingsConfig) -> Self {
        // defaults jako base, user_config jako overlay
        let merged = KeybindingsConfig::merge(KeybindingsConfig::default(), user_config);
        Self::new(merged)
    }

    /// Rozwiąż KeyEvent na KeyAction w kontekście podanego widoku.
    ///
    /// Lookup chain: view-specific → global.
    /// View-specific binding wygrywa gdy istnieje dla danego klawisza.
    pub fn resolve(&self, key: &KeyEvent, view: View) -> Option<KeyAction> {
        // 1. View-specific (ma priorytet nad globalnym)
        let view_action = match view {
            View::Orchestrate => self.config.orchestrate.resolve(key),
            View::Run => self.config.run.resolve(key),
            View::Explorer => self.config.explorer.resolve(key),
            View::Global => None,
        };

        if view_action.is_some() {
            return view_action;
        }

        // 2. Global fallback
        self.config.global.resolve(key)
    }

    /// Reverse lookup: znajdź KeyCombo przypisany do podanej akcji.
    ///
    /// Przeszukuje wszystkie sekcje bindingów (global → orchestrate → run → explorer).
    /// Zwraca pierwsze znalezione combo — używane do wyświetlania hints w UI.
    pub fn key_for_action(&self, action: KeyAction) -> Option<KeyCombo> {
        self.config
            .global
            .pairs()
            .into_iter()
            .chain(self.config.orchestrate.pairs())
            .chain(self.config.run.pairs())
            .chain(self.config.explorer.pairs())
            .find(|(_, a)| *a == action)
            .map(|(combo, _)| combo)
    }

    /// Format KeyCombo jako human-readable string z unicode symbolami.
    ///
    /// Modyfikatory używają unicode: Shift → `⇧`, Alt → `⌥`.
    /// Ctrl zachowany jako tekst: `Ctrl`.
    /// Klawisze strzałek jako unicode: `↑`, `↓`, `←`, `→`.
    /// Backspace jako `⌫`.
    ///
    /// Przykłady:
    /// - `Ctrl+p` → `"Ctrl+p"`
    /// - `Shift+Tab` → `"⇧+Tab"`
    /// - `Alt+Enter` → `"⌥+Enter"`
    /// - `Ctrl+Shift+a` → `"Ctrl+⇧+a"`
    /// - `Up` → `"↑"`
    pub fn format_key(combo: &KeyCombo) -> String {
        // Buduj prefiks modyfikatorów (Ctrl jako tekst, Shift/Alt jako symbole)
        let mut parts: Vec<&str> = Vec::new();
        if combo.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl");
        }
        if combo.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("⇧");
        }
        if combo.modifiers.contains(KeyModifiers::ALT) {
            parts.push("⌥");
        }

        // Klucz jako string — dynamiczne klucze (Char, F-keys) wymagają alloc
        let key_str: String = match combo.key {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::F(n) => format!("F{n}"),
            KeyCode::Up => "↑".to_string(),
            KeyCode::Down => "↓".to_string(),
            KeyCode::Left => "←".to_string(),
            KeyCode::Right => "→".to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::BackTab => "Tab".to_string(), // BackTab = Shift+Tab, Shift już w parts
            KeyCode::Backspace => "⌫".to_string(),
            KeyCode::Delete => "Del".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PgUp".to_string(),
            KeyCode::PageDown => "PgDn".to_string(),
            _ => "?".to_string(),
        };

        if parts.is_empty() {
            key_str
        } else {
            format!("{}+{}", parts.join("+"), key_str)
        }
    }

    /// Zwraca referencję do wewnętrznej konfiguracji.
    pub fn config(&self) -> &KeybindingsConfig {
        &self.config
    }
}

impl Default for KeybindingResolver {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// ── Testy ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── KeyCombo parser: simple keys ──

    #[test]
    fn parse_single_char() {
        let combo: KeyCombo = "q".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('q'));
        assert_eq!(combo.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_uppercase_char() {
        let combo: KeyCombo = "R".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('R'));
        assert_eq!(combo.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_space_named() {
        let combo: KeyCombo = "Space".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char(' '));
        assert_eq!(combo.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_plus_char() {
        let combo: KeyCombo = "+".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('+'));
        assert_eq!(combo.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_plus_named() {
        let combo: KeyCombo = "Plus".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('+'));
        assert_eq!(combo.modifiers, KeyModifiers::NONE);
    }

    // ── KeyCombo parser: named keys ──

    #[test]
    fn parse_arrow_keys() {
        assert_eq!("Up".parse::<KeyCombo>().unwrap().key, KeyCode::Up);
        assert_eq!("Down".parse::<KeyCombo>().unwrap().key, KeyCode::Down);
        assert_eq!("Left".parse::<KeyCombo>().unwrap().key, KeyCode::Left);
        assert_eq!("Right".parse::<KeyCombo>().unwrap().key, KeyCode::Right);
    }

    #[test]
    fn parse_navigation_keys() {
        assert_eq!("Enter".parse::<KeyCombo>().unwrap().key, KeyCode::Enter);
        assert_eq!("Return".parse::<KeyCombo>().unwrap().key, KeyCode::Enter);
        assert_eq!("Esc".parse::<KeyCombo>().unwrap().key, KeyCode::Esc);
        assert_eq!("Escape".parse::<KeyCombo>().unwrap().key, KeyCode::Esc);
        assert_eq!("Tab".parse::<KeyCombo>().unwrap().key, KeyCode::Tab);
        assert_eq!("BackTab".parse::<KeyCombo>().unwrap().key, KeyCode::BackTab);
    }

    #[test]
    fn parse_editing_keys() {
        assert_eq!(
            "Backspace".parse::<KeyCombo>().unwrap().key,
            KeyCode::Backspace
        );
        assert_eq!("Delete".parse::<KeyCombo>().unwrap().key, KeyCode::Delete);
        assert_eq!("Del".parse::<KeyCombo>().unwrap().key, KeyCode::Delete);
    }

    #[test]
    fn parse_scroll_keys() {
        assert_eq!("Home".parse::<KeyCombo>().unwrap().key, KeyCode::Home);
        assert_eq!("End".parse::<KeyCombo>().unwrap().key, KeyCode::End);
        assert_eq!("PageUp".parse::<KeyCombo>().unwrap().key, KeyCode::PageUp);
        assert_eq!(
            "PageDown".parse::<KeyCombo>().unwrap().key,
            KeyCode::PageDown
        );
    }

    #[test]
    fn parse_f_keys() {
        assert_eq!("F1".parse::<KeyCombo>().unwrap().key, KeyCode::F(1));
        assert_eq!("F12".parse::<KeyCombo>().unwrap().key, KeyCode::F(12));
        assert_eq!("f5".parse::<KeyCombo>().unwrap().key, KeyCode::F(5));
    }

    #[test]
    fn parse_named_keys_case_insensitive() {
        assert_eq!("up".parse::<KeyCombo>().unwrap().key, KeyCode::Up);
        assert_eq!("UP".parse::<KeyCombo>().unwrap().key, KeyCode::Up);
        assert_eq!("enter".parse::<KeyCombo>().unwrap().key, KeyCode::Enter);
        assert_eq!(
            "PAGEDOWN".parse::<KeyCombo>().unwrap().key,
            KeyCode::PageDown
        );
    }

    // ── KeyCombo parser: modifiers ──

    #[test]
    fn parse_ctrl_modifier() {
        let combo: KeyCombo = "Ctrl+p".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('p'));
        assert_eq!(combo.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_shift_modifier() {
        let combo: KeyCombo = "Shift+R".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('R'));
        assert_eq!(combo.modifiers, KeyModifiers::SHIFT);
    }

    #[test]
    fn parse_alt_modifier() {
        let combo: KeyCombo = "Alt+x".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('x'));
        assert_eq!(combo.modifiers, KeyModifiers::ALT);
    }

    #[test]
    fn parse_multiple_modifiers() {
        let combo: KeyCombo = "Ctrl+Shift+a".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('a'));
        assert!(combo.modifiers.contains(KeyModifiers::CONTROL));
        assert!(combo.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn parse_modifier_case_insensitive() {
        let combo: KeyCombo = "ctrl+p".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('p'));
        assert_eq!(combo.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_control_alias() {
        let combo: KeyCombo = "Control+c".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('c'));
        assert_eq!(combo.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_modifier_with_named_key() {
        let combo: KeyCombo = "Ctrl+Up".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Up);
        assert_eq!(combo.modifiers, KeyModifiers::CONTROL);
    }

    // ── KeyCombo parser: Shift+Tab → BackTab normalization ──

    #[test]
    fn parse_shift_tab_normalizes_to_backtab() {
        let combo: KeyCombo = "Shift+Tab".parse().unwrap();
        assert_eq!(combo.key, KeyCode::BackTab);
        assert_eq!(combo.modifiers, KeyModifiers::SHIFT);
    }

    // ── KeyCombo parser: whitespace handling ──

    #[test]
    fn parse_with_whitespace() {
        let combo: KeyCombo = "  Ctrl + p  ".parse().unwrap();
        assert_eq!(combo.key, KeyCode::Char('p'));
        assert_eq!(combo.modifiers, KeyModifiers::CONTROL);
    }

    // ── KeyCombo parser: error cases ──

    #[test]
    fn parse_empty_string_fails() {
        assert!("".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn parse_whitespace_only_fails() {
        assert!("   ".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn parse_unknown_key_fails() {
        assert!("FooBar".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn parse_unknown_modifier_fails() {
        assert!("Super+a".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn parse_trailing_plus_fails() {
        assert!("Ctrl+".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn parse_f_key_out_of_range() {
        assert!("F0".parse::<KeyCombo>().is_err());
        assert!("F13".parse::<KeyCombo>().is_err());
    }

    // ── KeyCombo Display ──

    #[test]
    fn display_simple_char() {
        let combo = KeyCombo::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(combo.to_string(), "q");
    }

    #[test]
    fn display_ctrl_modifier() {
        let combo = KeyCombo::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(combo.to_string(), "Ctrl+p");
    }

    #[test]
    fn display_shift_modifier() {
        let combo = KeyCombo::new(KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(combo.to_string(), "Shift+R");
    }

    #[test]
    fn display_multiple_modifiers() {
        let combo = KeyCombo::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(combo.to_string(), "Ctrl+Shift+a");
    }

    #[test]
    fn display_named_key() {
        let combo = KeyCombo::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(combo.to_string(), "Up");
    }

    #[test]
    fn display_space() {
        let combo = KeyCombo::new(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(combo.to_string(), "Space");
    }

    #[test]
    fn display_f_key() {
        let combo = KeyCombo::new(KeyCode::F(5), KeyModifiers::NONE);
        assert_eq!(combo.to_string(), "F5");
    }

    #[test]
    fn display_backtab() {
        let combo = KeyCombo::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(combo.to_string(), "Shift+Tab");
    }

    // ── KeyCombo serde round-trip ──

    #[test]
    fn keycombo_serialize_round_trip() {
        // Serialize KeyCombo → TOML string → deserialize → powinno wrócić do identycznej wartości
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            key: KeyCombo,
        }
        let combo = KeyCombo::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        let wrapper = Wrapper { key: combo.clone() };
        let s = toml::to_string(&wrapper).unwrap();
        let back: Wrapper = toml::from_str(&s).unwrap();
        assert_eq!(combo, back.key);
    }

    #[test]
    fn keycombo_serialize_round_trip_named_key() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            key: KeyCombo,
        }
        let combo = KeyCombo::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        let wrapper = Wrapper { key: combo.clone() };
        let s = toml::to_string(&wrapper).unwrap();
        let back: Wrapper = toml::from_str(&s).unwrap();
        assert_eq!(combo, back.key);
    }

    // ── KeyCombo Display round-trip ──

    #[test]
    fn round_trip_simple_char() {
        let original = "q";
        let combo: KeyCombo = original.parse().unwrap();
        let displayed = combo.to_string();
        let reparsed: KeyCombo = displayed.parse().unwrap();
        assert_eq!(combo, reparsed);
    }

    #[test]
    fn round_trip_ctrl_modifier() {
        let original = "Ctrl+p";
        let combo: KeyCombo = original.parse().unwrap();
        let displayed = combo.to_string();
        assert_eq!(displayed, "Ctrl+p");
        let reparsed: KeyCombo = displayed.parse().unwrap();
        assert_eq!(combo, reparsed);
    }

    #[test]
    fn round_trip_named_key() {
        let original = "PageDown";
        let combo: KeyCombo = original.parse().unwrap();
        let displayed = combo.to_string();
        let reparsed: KeyCombo = displayed.parse().unwrap();
        assert_eq!(combo, reparsed);
    }

    #[test]
    fn round_trip_f_key() {
        let original = "F12";
        let combo: KeyCombo = original.parse().unwrap();
        let displayed = combo.to_string();
        let reparsed: KeyCombo = displayed.parse().unwrap();
        assert_eq!(combo, reparsed);
    }

    #[test]
    fn round_trip_shift_tab() {
        // "Shift+Tab" parses to BackTab → displays as "Shift+Tab" → round-trip OK
        let original = "Shift+Tab";
        let combo: KeyCombo = original.parse().unwrap();
        let displayed = combo.to_string();
        assert_eq!(displayed, "Shift+Tab");
        let reparsed: KeyCombo = displayed.parse().unwrap();
        assert_eq!(combo, reparsed);
    }

    #[test]
    fn round_trip_multiple_modifiers() {
        let original = "Ctrl+Shift+a";
        let combo: KeyCombo = original.parse().unwrap();
        let displayed = combo.to_string();
        let reparsed: KeyCombo = displayed.parse().unwrap();
        assert_eq!(combo, reparsed);
    }

    // ── KeyCombo::matches ──

    #[test]
    fn matches_exact_event() {
        use crossterm::event::{KeyEventKind, KeyEventState};
        let combo = KeyCombo::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let event = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(combo.matches(&event));
    }

    #[test]
    fn matches_rejects_wrong_modifier() {
        use crossterm::event::{KeyEventKind, KeyEventState};
        let combo = KeyCombo::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let event = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(!combo.matches(&event));
    }

    #[test]
    fn matches_rejects_wrong_key() {
        use crossterm::event::{KeyEventKind, KeyEventState};
        let combo = KeyCombo::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let event = KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(!combo.matches(&event));
    }

    #[test]
    fn matches_ctrl_combo() {
        use crossterm::event::{KeyEventKind, KeyEventState};
        let combo = KeyCombo::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let event = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(combo.matches(&event));
    }

    // ── Default bindings ──

    #[test]
    fn default_global_bindings() {
        let bindings = GlobalBindings::default();
        assert_eq!(bindings.quit.key, KeyCode::Char('q'));
        assert_eq!(bindings.force_quit.key, KeyCode::Char('c'));
        assert_eq!(bindings.force_quit.modifiers, KeyModifiers::CONTROL);
        assert_eq!(bindings.toggle_sidebar.key, KeyCode::Char('t'));
        assert_eq!(bindings.scroll_up.key, KeyCode::Up);
        assert_eq!(bindings.scroll_down.key, KeyCode::Down);
        assert_eq!(bindings.command_palette.key, KeyCode::Char('p'));
        assert_eq!(bindings.command_palette.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn default_orchestrate_bindings() {
        let bindings = OrchestrateBindings::default();
        assert_eq!(bindings.focus_next.key, KeyCode::Tab);
        assert_eq!(bindings.focus_prev.key, KeyCode::BackTab);
        assert_eq!(bindings.toggle_preview.key, KeyCode::Char('p'));
        assert_eq!(bindings.restart.key, KeyCode::Char('R'));
        assert_eq!(bindings.restart.modifiers, KeyModifiers::SHIFT);
    }

    #[test]
    fn default_run_bindings() {
        let bindings = RunBindings::default();
        assert_eq!(bindings.toggle_expand.key, KeyCode::Enter);
    }

    #[test]
    fn default_explorer_bindings() {
        let bindings = ExplorerBindings::default();
        assert_eq!(bindings.cycle_sort.key, KeyCode::Char('s'));
        assert_eq!(bindings.enter_filter.key, KeyCode::Char('f'));
        assert_eq!(bindings.vim_up.key, KeyCode::Char('k'));
        assert_eq!(bindings.vim_down.key, KeyCode::Char('j'));
    }

    #[test]
    fn default_keybindings_config() {
        let config = KeybindingsConfig::default();
        assert_eq!(config.global.quit.key, KeyCode::Char('q'));
        assert_eq!(config.orchestrate.focus_next.key, KeyCode::Tab);
        assert_eq!(config.run.toggle_expand.key, KeyCode::Enter);
        assert_eq!(config.explorer.cycle_sort.key, KeyCode::Char('s'));
    }

    // ── TOML deserialization ──

    #[test]
    fn toml_deserialize_global_section() {
        let toml_content = r#"
[global]
quit = "Esc"
command_palette = "Ctrl+Shift+p"
"#;
        let config: KeybindingsConfig = toml::from_str(toml_content).unwrap();
        // Overridden
        assert_eq!(config.global.quit.key, KeyCode::Esc);
        assert_eq!(config.global.command_palette.key, KeyCode::Char('p'));
        assert!(
            config
                .global
                .command_palette
                .modifiers
                .contains(KeyModifiers::CONTROL)
        );
        assert!(
            config
                .global
                .command_palette
                .modifiers
                .contains(KeyModifiers::SHIFT)
        );
        // Non-overridden fields keep defaults
        assert_eq!(config.global.toggle_sidebar.key, KeyCode::Char('t'));
    }

    #[test]
    fn toml_deserialize_orchestrate_section() {
        let toml_content = r#"
[orchestrate]
restart = "Shift+F5"
toggle_preview = "v"
"#;
        let config: KeybindingsConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.orchestrate.restart.key, KeyCode::F(5));
        assert_eq!(config.orchestrate.restart.modifiers, KeyModifiers::SHIFT);
        assert_eq!(config.orchestrate.toggle_preview.key, KeyCode::Char('v'));
        // Defaults preserved
        assert_eq!(config.orchestrate.reload.key, KeyCode::Char('r'));
    }

    #[test]
    fn toml_deserialize_explorer_section() {
        let toml_content = r#"
[explorer]
enter_filter = "/"
vim_up = "w"
"#;
        let config: KeybindingsConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.explorer.enter_filter.key, KeyCode::Char('/'));
        assert_eq!(config.explorer.vim_up.key, KeyCode::Char('w'));
        // Defaults
        assert_eq!(config.explorer.vim_down.key, KeyCode::Char('j'));
    }

    #[test]
    fn toml_deserialize_empty_config() {
        let config: KeybindingsConfig = toml::from_str("").unwrap();
        assert_eq!(config, KeybindingsConfig::default());
    }

    #[test]
    fn toml_deserialize_full_config() {
        let toml_content = r#"
[global]
quit = "q"
toggle_sidebar = "t"
scroll_up = "Up"
scroll_down = "Down"
command_palette = "Ctrl+p"

[orchestrate]
focus_next = "Tab"
focus_prev = "Shift+Tab"

[run]
toggle_expand = "Enter"

[explorer]
cycle_sort = "s"
enter_filter = "f"
"#;
        let config: KeybindingsConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.global.quit.key, KeyCode::Char('q'));
        assert_eq!(config.orchestrate.focus_next.key, KeyCode::Tab);
        assert_eq!(config.orchestrate.focus_prev.key, KeyCode::BackTab);
        assert_eq!(config.run.toggle_expand.key, KeyCode::Enter);
        assert_eq!(config.explorer.cycle_sort.key, KeyCode::Char('s'));
    }

    #[test]
    fn toml_deserialize_invalid_key_fails() {
        let toml_content = r#"
[global]
quit = "InvalidKeyName"
"#;
        let result = toml::from_str::<KeybindingsConfig>(toml_content);
        assert!(result.is_err());
    }

    #[test]
    fn toml_deserialize_invalid_modifier_fails() {
        let toml_content = r#"
[global]
quit = "Super+q"
"#;
        let result = toml::from_str::<KeybindingsConfig>(toml_content);
        assert!(result.is_err());
    }

    // ── TOML as nested [keybindings.*] in full config ──

    #[test]
    fn toml_nested_keybindings_section() {
        // Symulacja [keybindings.global] z pełnego .ralph.toml
        #[derive(Deserialize)]
        struct MockConfig {
            #[serde(default)]
            keybindings: KeybindingsConfig,
        }
        let toml_content = r#"
[keybindings.global]
quit = "Esc"
command_palette = "Ctrl+p"

[keybindings.orchestrate]
restart = "F5"
"#;
        let config: MockConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.keybindings.global.quit.key, KeyCode::Esc);
        assert_eq!(
            config.keybindings.global.command_palette.key,
            KeyCode::Char('p')
        );
        assert_eq!(config.keybindings.orchestrate.restart.key, KeyCode::F(5));
    }

    // ── KeyAction enum ──

    #[test]
    fn key_action_variants_exist() {
        // Sprawdzenie że wszystkie wymagane warianty istnieją
        let actions = [
            KeyAction::Quit,
            KeyAction::ToggleSidebar,
            KeyAction::ScrollUp,
            KeyAction::ScrollDown,
            KeyAction::FocusNext,
            KeyAction::FocusPrev,
            KeyAction::Restart,
            KeyAction::TogglePreview,
            KeyAction::CommandPalette,
        ];
        // Warianty są unikalne
        for (i, a) in actions.iter().enumerate() {
            for (j, b) in actions.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    // ── Merge ──

    #[test]
    fn merge_overlay_overrides_non_default() {
        let base = KeybindingsConfig::default();
        let mut overlay = KeybindingsConfig::default();
        overlay.global.quit = KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE);

        let merged = KeybindingsConfig::merge(base, overlay);
        // Overlay zmienił quit → użyj overlay
        assert_eq!(merged.global.quit.key, KeyCode::Esc);
        // Inne pola zachowują base (tu base == default)
        assert_eq!(merged.global.toggle_sidebar.key, KeyCode::Char('t'));
    }

    #[test]
    fn merge_default_overlay_preserves_base() {
        let mut base = KeybindingsConfig::default();
        base.global.quit = KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE);
        let overlay = KeybindingsConfig::default();

        let merged = KeybindingsConfig::merge(base, overlay);
        // Overlay jest default → zachowaj base
        assert_eq!(merged.global.quit.key, KeyCode::Esc);
    }

    #[test]
    fn merge_per_section_independence() {
        let base = KeybindingsConfig::default();
        let mut overlay = KeybindingsConfig::default();
        overlay.orchestrate.restart = KeyCombo::new(KeyCode::F(5), KeyModifiers::NONE);

        let merged = KeybindingsConfig::merge(base, overlay);
        // Orchestrate zmieniony
        assert_eq!(merged.orchestrate.restart.key, KeyCode::F(5));
        // Global niezmieniony
        assert_eq!(merged.global.quit.key, KeyCode::Char('q'));
    }

    #[test]
    fn merge_three_layers() {
        let defaults = KeybindingsConfig::default();

        let mut global = KeybindingsConfig::default();
        global.global.quit = KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE);
        global.orchestrate.restart = KeyCombo::new(KeyCode::F(5), KeyModifiers::NONE);

        let mut local = KeybindingsConfig::default();
        local.global.command_palette = KeyCombo::new(KeyCode::Char('k'), KeyModifiers::CONTROL);

        let step1 = KeybindingsConfig::merge(defaults, global);
        let merged = KeybindingsConfig::merge(step1, local);

        // quit z global (local nie zmienił)
        assert_eq!(merged.global.quit.key, KeyCode::Esc);
        // command_palette z local
        assert_eq!(merged.global.command_palette.key, KeyCode::Char('k'));
        // restart z global (local nie zmienił)
        assert_eq!(merged.orchestrate.restart.key, KeyCode::F(5));
    }

    // ── Resolve ──

    /// Helper: tworzy KeyEvent z podanego KeyCode i KeyModifiers.
    fn make_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        use crossterm::event::{KeyEventKind, KeyEventState};
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn resolve_global_quit() {
        let bindings = GlobalBindings::default();
        let event = make_event(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(bindings.resolve(&event), Some(KeyAction::Quit));
    }

    #[test]
    fn resolve_global_force_quit() {
        let bindings = GlobalBindings::default();
        let event = make_event(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(bindings.resolve(&event), Some(KeyAction::ForceQuit));
    }

    #[test]
    fn resolve_global_command_palette() {
        let bindings = GlobalBindings::default();
        let event = make_event(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(bindings.resolve(&event), Some(KeyAction::CommandPalette));
    }

    #[test]
    fn resolve_global_no_match() {
        let bindings = GlobalBindings::default();
        let event = make_event(KeyCode::Char('z'), KeyModifiers::NONE);
        assert_eq!(bindings.resolve(&event), None);
    }

    #[test]
    fn resolve_global_custom_binding() {
        let bindings = GlobalBindings {
            quit: KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE),
            ..GlobalBindings::default()
        };
        let event = make_event(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(bindings.resolve(&event), Some(KeyAction::Quit));
        // Stary quit ('q') już nie działa
        let old_event = make_event(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(bindings.resolve(&old_event), None);
    }

    #[test]
    fn resolve_orchestrate_toggle_preview() {
        let bindings = OrchestrateBindings::default();
        let event = make_event(KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(bindings.resolve(&event), Some(KeyAction::TogglePreview));
    }

    #[test]
    fn resolve_orchestrate_restart() {
        let bindings = OrchestrateBindings::default();
        let event = make_event(KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(bindings.resolve(&event), Some(KeyAction::Restart));
    }

    #[test]
    fn resolve_run_toggle_expand() {
        let bindings = RunBindings::default();
        let event = make_event(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(bindings.resolve(&event), Some(KeyAction::ToggleExpand));
    }

    #[test]
    fn resolve_explorer_vim_keys() {
        let bindings = ExplorerBindings::default();
        assert_eq!(
            bindings.resolve(&make_event(KeyCode::Char('k'), KeyModifiers::NONE)),
            Some(KeyAction::VimUp)
        );
        assert_eq!(
            bindings.resolve(&make_event(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(KeyAction::VimDown)
        );
        assert_eq!(
            bindings.resolve(&make_event(KeyCode::Char('h'), KeyModifiers::NONE)),
            Some(KeyAction::VimLeft)
        );
        assert_eq!(
            bindings.resolve(&make_event(KeyCode::Char('l'), KeyModifiers::NONE)),
            Some(KeyAction::VimRight)
        );
    }

    #[test]
    fn resolve_explorer_cycle_sort() {
        let bindings = ExplorerBindings::default();
        let event = make_event(KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(bindings.resolve(&event), Some(KeyAction::CycleSort));
    }

    // ── KeybindingResolver: forward lookup ──

    #[test]
    fn resolver_default_global_quit() {
        let resolver = KeybindingResolver::with_defaults();
        let event = make_event(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(
            resolver.resolve(&event, View::Global),
            Some(KeyAction::Quit)
        );
    }

    #[test]
    fn resolver_global_fallback_from_orchestrate_view() {
        // 'q' nie jest w orchestrate-specific → fallback do global
        let resolver = KeybindingResolver::with_defaults();
        let event = make_event(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(
            resolver.resolve(&event, View::Orchestrate),
            Some(KeyAction::Quit)
        );
    }

    #[test]
    fn resolver_view_specific_wins_over_global() {
        // 'Enter' jest zarówno global::confirm jak i run::toggle_expand
        // W widoku Run → ToggleExpand (view-specific wygrywa)
        let resolver = KeybindingResolver::with_defaults();
        let event = make_event(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            resolver.resolve(&event, View::Run),
            Some(KeyAction::ToggleExpand)
        );
        // W widoku Global → Confirm (tylko globalny)
        assert_eq!(
            resolver.resolve(&event, View::Global),
            Some(KeyAction::Confirm)
        );
    }

    #[test]
    fn resolver_orchestrate_view_specific() {
        let resolver = KeybindingResolver::with_defaults();
        // 'p' w orchestrate → TogglePreview (view-specific)
        let event = make_event(KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(
            resolver.resolve(&event, View::Orchestrate),
            Some(KeyAction::TogglePreview)
        );
        // 'p' w widoku Global → nic (nie ma 'p' w global bez Ctrl)
        assert_eq!(resolver.resolve(&event, View::Global), None);
    }

    #[test]
    fn resolver_explorer_view_specific() {
        let resolver = KeybindingResolver::with_defaults();
        let event = make_event(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(
            resolver.resolve(&event, View::Explorer),
            Some(KeyAction::VimUp)
        );
    }

    #[test]
    fn resolver_unknown_key_returns_none() {
        let resolver = KeybindingResolver::with_defaults();
        let event = make_event(KeyCode::Char('z'), KeyModifiers::NONE);
        assert_eq!(resolver.resolve(&event, View::Global), None);
        assert_eq!(resolver.resolve(&event, View::Orchestrate), None);
        assert_eq!(resolver.resolve(&event, View::Explorer), None);
    }

    #[test]
    fn resolver_custom_keybinding_used() {
        // Użytkownik zmapował 'q' na Esc
        let mut user_config = KeybindingsConfig::default();
        user_config.global.quit = KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE);
        let resolver = KeybindingResolver::from_user_config(user_config);

        // Esc → Quit (custom binding działa)
        let esc = make_event(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(resolver.resolve(&esc, View::Global), Some(KeyAction::Quit));

        // Stary 'q' już nie daje Quit (ale może dać Cancel przez global::cancel = Esc... hmm)
        // Uwaga: cancel też jest Esc w defaults, więc sprawdzamy 'q' bez Esc
        let q = make_event(KeyCode::Char('q'), KeyModifiers::NONE);
        // 'q' nie jest już quit (zmienione), a nie jest też inną akcją globalną
        assert_eq!(resolver.resolve(&q, View::Global), None);
    }

    #[test]
    fn resolver_no_custom_keybinding_uses_default() {
        // Pusta customizacja → defaults powinny działać
        let resolver = KeybindingResolver::from_user_config(KeybindingsConfig::default());
        let event = make_event(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(
            resolver.resolve(&event, View::Global),
            Some(KeyAction::CommandPalette)
        );
    }

    #[test]
    fn resolver_global_view_no_view_specific() {
        // View::Global nigdy nie sprawdza view-specific bindingów
        let resolver = KeybindingResolver::with_defaults();
        // 's' jest tylko w ExplorerBindings
        let event = make_event(KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(resolver.resolve(&event, View::Global), None);
    }

    // ── KeybindingResolver: reverse lookup ──

    #[test]
    fn resolver_key_for_action_global() {
        let resolver = KeybindingResolver::with_defaults();
        let combo = resolver.key_for_action(KeyAction::Quit).unwrap();
        assert_eq!(combo.key, KeyCode::Char('q'));
        assert_eq!(combo.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn resolver_key_for_action_orchestrate() {
        let resolver = KeybindingResolver::with_defaults();
        let combo = resolver.key_for_action(KeyAction::TogglePreview).unwrap();
        assert_eq!(combo.key, KeyCode::Char('p'));
        assert_eq!(combo.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn resolver_key_for_action_explorer() {
        let resolver = KeybindingResolver::with_defaults();
        let combo = resolver.key_for_action(KeyAction::VimUp).unwrap();
        assert_eq!(combo.key, KeyCode::Char('k'));
    }

    #[test]
    fn resolver_key_for_action_run() {
        let resolver = KeybindingResolver::with_defaults();
        let combo = resolver.key_for_action(KeyAction::ToggleExpand).unwrap();
        assert_eq!(combo.key, KeyCode::Enter);
    }

    #[test]
    fn resolver_key_for_action_custom() {
        // Użytkownik zmienił quit na F1
        let mut user_config = KeybindingsConfig::default();
        user_config.global.quit = KeyCombo::new(KeyCode::F(1), KeyModifiers::NONE);
        let resolver = KeybindingResolver::from_user_config(user_config);

        let combo = resolver.key_for_action(KeyAction::Quit).unwrap();
        assert_eq!(combo.key, KeyCode::F(1));
    }

    // ── KeybindingResolver: pairs() ──

    #[test]
    fn global_pairs_covers_all_actions() {
        let bindings = GlobalBindings::default();
        let pairs = bindings.pairs();
        // Sprawdź że Quit jest w parach
        assert!(pairs.iter().any(|(_, a)| *a == KeyAction::Quit));
        assert!(pairs.iter().any(|(_, a)| *a == KeyAction::CommandPalette));
        assert!(pairs.iter().any(|(_, a)| *a == KeyAction::ScrollUp));
    }

    #[test]
    fn explorer_pairs_covers_vim_keys() {
        let bindings = ExplorerBindings::default();
        let pairs = bindings.pairs();
        assert!(pairs.iter().any(|(_, a)| *a == KeyAction::VimUp));
        assert!(pairs.iter().any(|(_, a)| *a == KeyAction::VimDown));
        assert!(pairs.iter().any(|(_, a)| *a == KeyAction::VimLeft));
        assert!(pairs.iter().any(|(_, a)| *a == KeyAction::VimRight));
    }

    // ── KeybindingResolver: format_key ──

    #[test]
    fn format_key_simple_char() {
        let combo = KeyCombo::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(KeybindingResolver::format_key(&combo), "q");
    }

    #[test]
    fn format_key_ctrl() {
        let combo = KeyCombo::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(KeybindingResolver::format_key(&combo), "Ctrl+p");
    }

    #[test]
    fn format_key_shift() {
        let combo = KeyCombo::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(KeybindingResolver::format_key(&combo), "⇧+Enter");
    }

    #[test]
    fn format_key_alt() {
        let combo = KeyCombo::new(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(KeybindingResolver::format_key(&combo), "⌥+x");
    }

    #[test]
    fn format_key_ctrl_shift() {
        let combo = KeyCombo::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(KeybindingResolver::format_key(&combo), "Ctrl+⇧+a");
    }

    #[test]
    fn format_key_arrow_unicode() {
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::Up, KeyModifiers::NONE)),
            "↑"
        );
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::Down, KeyModifiers::NONE)),
            "↓"
        );
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::Left, KeyModifiers::NONE)),
            "←"
        );
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::Right, KeyModifiers::NONE)),
            "→"
        );
    }

    #[test]
    fn format_key_special_keys() {
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE)),
            "Enter"
        );
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE)),
            "Esc"
        );
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::Tab, KeyModifiers::NONE)),
            "Tab"
        );
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::Backspace, KeyModifiers::NONE)),
            "⌫"
        );
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::PageUp, KeyModifiers::NONE)),
            "PgUp"
        );
        assert_eq!(
            KeybindingResolver::format_key(&KeyCombo::new(KeyCode::PageDown, KeyModifiers::NONE)),
            "PgDn"
        );
    }

    #[test]
    fn format_key_f_key() {
        let combo = KeyCombo::new(KeyCode::F(5), KeyModifiers::NONE);
        assert_eq!(KeybindingResolver::format_key(&combo), "F5");
    }

    #[test]
    fn format_key_shift_tab() {
        // BackTab = Shift+Tab
        let combo = KeyCombo::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(KeybindingResolver::format_key(&combo), "⇧+Tab");
    }

    #[test]
    fn format_key_space() {
        let combo = KeyCombo::new(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(KeybindingResolver::format_key(&combo), "Space");
    }

    // ── Lookup chain priority (collision global vs view-specific) ──

    #[test]
    fn lookup_chain_view_specific_wins_collision() {
        // Stwórz resolver gdzie 'Enter' jest zarówno global::confirm (default)
        // jak i run::toggle_expand (default) — view-specific musi wygrać
        let resolver = KeybindingResolver::with_defaults();
        let enter = make_event(KeyCode::Enter, KeyModifiers::NONE);

        // View::Run → ToggleExpand (view-specific)
        assert_eq!(
            resolver.resolve(&enter, View::Run),
            Some(KeyAction::ToggleExpand)
        );
        // View::Orchestrate → Confirm (global fallback, bo Orchestrate nie ma Enter)
        assert_eq!(
            resolver.resolve(&enter, View::Orchestrate),
            Some(KeyAction::Confirm)
        );
        // View::Explorer → ExpandOrEnter (view-specific)
        assert_eq!(
            resolver.resolve(&enter, View::Explorer),
            Some(KeyAction::ExpandOrEnter)
        );
    }

    #[test]
    fn lookup_chain_custom_global_overrides_default() {
        // Użytkownik ustawił 'g' jako scroll_to_top (zamiast Home)
        let mut user_config = KeybindingsConfig::default();
        user_config.global.scroll_to_top = KeyCombo::new(KeyCode::Char('g'), KeyModifiers::NONE);
        let resolver = KeybindingResolver::from_user_config(user_config);

        let g = make_event(KeyCode::Char('g'), KeyModifiers::NONE);
        assert_eq!(
            resolver.resolve(&g, View::Global),
            Some(KeyAction::ScrollToTop)
        );

        // Home nie jest już scroll_to_top (zastąpione przez 'g')
        let home = make_event(KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(resolver.resolve(&home, View::Global), None);
    }
}
