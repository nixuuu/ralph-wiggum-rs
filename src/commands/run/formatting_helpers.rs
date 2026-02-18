// Re-export formatowania z centralnego modułu tui::formatting (DRY — single source of truth)
pub(super) use crate::tui::formatting::format_duration_string as format_duration_short;
pub(super) use crate::tui::formatting::format_tokens_string as format_tokens;
