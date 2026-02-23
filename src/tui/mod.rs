/// TUI module — terminal user interface components and theming
pub mod app;
#[allow(dead_code)]
pub mod events;
pub mod formatter;
#[allow(dead_code)]
pub mod formatting;
#[allow(dead_code)]
pub mod fuzzy;
pub mod responsive;
pub mod ring_buffer;
#[allow(dead_code)]
pub mod theme;
#[allow(dead_code)]
pub mod tool_formatting;
#[allow(dead_code)]
pub mod widgets;

#[cfg(test)]
pub mod test_helpers;

#[allow(unused_imports)] // Re-exporty — będą używane przez per-command widoki TUI
pub use app::{App, AppState};
#[allow(unused_imports)] // Re-exporty — będą używane przez per-command widoki TUI
pub use events::{
    AppEvent, EventDispatcher, EventResult, KeyHandler, dispatch_key, is_ctrl_c, is_quit_key,
    is_scroll_down, is_scroll_up,
};
#[allow(unused_imports)] // Re-exporty — będą używane przez per-command widoki TUI
pub use formatting::{
    format_duration_span, format_duration_string, format_tokens_span, format_tokens_string,
};
// Re-eksport fuzzy matching API — używane przez widgety z filtrowaniem (np. task explorer)
#[allow(unused_imports)]
pub use fuzzy::{FuzzyMatch, fuzzy_filter, fuzzy_match};
// Re-eksport publicznego API formattera — używane przez przyszłe implementacje RatuiFormatter
#[allow(unused_imports)]
pub use formatter::{
    RatuiFormatter, bold_span, colored_bold_span, colored_span, icon_span, muted_span, styled_span,
};
// Re-eksport tool formatting utilities — Span-based output dla tool calls
#[allow(unused_imports)]
pub use responsive::{Breakpoint, LayoutAreas};
#[allow(unused_imports)]
pub use ring_buffer::OutputRingBuffer;
#[allow(unused_imports)] // Re-exporty — będą używane przez per-command widoki TUI
pub use theme::{DEFAULT_THEME, Theme};
#[allow(unused_imports)]
pub use tool_formatting::{colorize_tool_name, format_tool_details, shorten_path, truncate_string};
#[allow(unused_imports)] // Re-exporty — będą używane przez per-command widoki TUI
pub use widgets::{
    AskUserAction, AskUserQuestion, AskUserWidget, CommandPaletteState, CommandPaletteWidget,
    FlatRow, Header, HeaderData, InputAction, MultiSelectOption, MultiSelectState,
    MultiSelectWidget, PaletteAction, PaletteItem, ProgressData, QuestionKind, SplashScreen,
    StatusBar, StatusBarData, TaskSidebar, TaskSidebarState, TaskTreeWidget, TextInputOverlay,
    TextInputState, TextInputWidget, TreeState, render_task_preview,
};
