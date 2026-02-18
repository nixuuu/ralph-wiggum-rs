/// TUI widgets — reusable UI components for status bars, panels, etc.
pub mod ask_user;
pub mod ask_user_choice;
pub mod ask_user_confirm;
pub mod ask_user_multi;
pub mod ask_user_text;
#[allow(dead_code)]
pub mod header;
pub mod splash;
#[allow(dead_code)]
pub mod status_bar;
#[allow(dead_code)] // TUI component — will be used when full TUI is integrated
mod task_detail;
#[allow(dead_code)] // TUI component — will be used when full TUI is integrated
pub mod task_preview;
#[allow(dead_code)]
mod task_sidebar;
#[allow(dead_code)]
pub mod task_tree;
pub mod text_input_overlay;

#[allow(dead_code)]
mod output_view;

#[cfg(test)]
mod ask_user_integration_tests;

pub use ask_user_choice::ChoiceState;
#[allow(unused_imports)] // Re-exported for tests and future TUI integration
pub use ask_user_choice::{ChoiceEvent, ChoiceWidget, QuestionOption};
pub use ask_user_confirm::ConfirmState;
#[allow(unused_imports)] // Re-exported for tests and future TUI integration
pub use ask_user_confirm::ConfirmWidget;
#[allow(unused_imports)] // Re-exported for tests and future TUI integration
pub use ask_user_multi::MultiSelectWidget;
pub use ask_user_multi::{MultiSelectOption, MultiSelectState};
pub use ask_user_text::TextInputState;
#[allow(unused_imports)] // Re-exported for tests and future TUI integration
pub use ask_user_text::TextInputWidget;
pub use header::{Header, HeaderData};
pub use splash::SplashScreen;
pub use status_bar::{ProgressData, StatusBar, StatusBarData};
#[allow(unused_imports)] // TUI component — will be used when full TUI is integrated
pub use task_detail::{TaskDetail, TaskDetailState};
#[allow(unused_imports)] // TUI component — will be used when full TUI is integrated
pub use task_preview::render_task_preview;
pub use task_sidebar::{TaskSidebar, TaskSidebarState, render_sidebar_overlay};
pub use task_tree::{FlatRow, TaskTreeWidget, TreeState};
pub use text_input_overlay::{InputAction, TextInputOverlay};

pub use ask_user::{AskUserAction, AskUserQuestion, AskUserWidget, QuestionKind};
pub use output_view::{OutputView, OutputViewState};
