//! Worker status types for the orchestrator subsystem.

use crate::commands::task::orchestrate::events::WorkerPhase;

/// Per-worker status for display purposes.
#[derive(Debug, Clone)]
pub struct WorkerStatus {
    pub state: WorkerState,
    pub task_id: Option<String>,
    pub component: Option<String>,
    pub phase: Option<WorkerPhase>,
    /// Model alias (opus/sonnet/haiku) for the current task.
    /// Displayed in dashboard panels.
    pub model: Option<String>,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Verify phase profile statuses: (profile_name, success)
    /// None = in progress, Some(true) = success, Some(false) = failed
    pub verify_profiles: Vec<(String, Option<bool>)>,
}

impl WorkerStatus {
    /// Create an idle worker status.
    pub fn idle(_worker_id: u32) -> Self {
        Self {
            state: WorkerState::Idle,
            task_id: None,
            component: None,
            phase: None,
            model: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            verify_profiles: Vec::new(),
        }
    }

    /// Check if this worker is in Idle state.
    pub fn is_idle(&self) -> bool {
        self.state == WorkerState::Idle
    }
}

/// Worker state for display purposes.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerState {
    Idle,
    SettingUp,
    Implementing,
    Reviewing,
    Verifying,
    Merging,
    ResolvingConflicts,
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerState::Idle => write!(f, "idle"),
            WorkerState::SettingUp => write!(f, "setting up"),
            WorkerState::Implementing => write!(f, "implementing"),
            WorkerState::Reviewing => write!(f, "reviewing"),
            WorkerState::Verifying => write!(f, "verifying"),
            WorkerState::Merging => write!(f, "merging"),
            WorkerState::ResolvingConflicts => write!(f, "resolving conflicts"),
        }
    }
}

impl WorkerState {
    /// Get display color for this worker state.
    pub fn color(&self) -> ratatui::style::Color {
        use ratatui::style::Color;
        match self {
            WorkerState::Idle => Color::DarkGray,
            WorkerState::SettingUp => Color::Blue,
            WorkerState::Implementing => Color::Cyan,
            WorkerState::Reviewing => Color::Yellow,
            WorkerState::Verifying => Color::Magenta,
            WorkerState::Merging => Color::Green,
            WorkerState::ResolvingConflicts => Color::Red,
        }
    }

    /// Get icon and its color for this worker state.
    pub fn icon(&self) -> (&'static str, ratatui::style::Color) {
        use ratatui::style::Color;
        match self {
            WorkerState::Idle => ("○", Color::DarkGray),
            WorkerState::SettingUp => ("⚙", Color::Blue),
            WorkerState::Implementing => ("●", Color::Cyan),
            WorkerState::Reviewing => ("◎", Color::Yellow),
            WorkerState::Verifying => ("◉", Color::Magenta),
            WorkerState::Merging => ("⊕", Color::Green),
            WorkerState::ResolvingConflicts => ("⚡", Color::Red),
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::task::orchestrate::events::WorkerPhase;

    // ── WorkerStatus model field tests ───────────────────────────────

    #[test]
    fn test_worker_status_idle_model_is_none() {
        let status = WorkerStatus::idle(0);
        assert_eq!(status.model, None);
    }

    #[test]
    fn test_worker_status_with_model_set() {
        let mut status = WorkerStatus::idle(1);
        status.model = Some("sonnet".to_string());
        assert_eq!(status.model, Some("sonnet".to_string()));
    }

    #[test]
    fn test_worker_status_model_preserved_on_clone() {
        let mut status = WorkerStatus::idle(2);
        status.model = Some("sonnet".to_string());

        let cloned = status.clone();
        assert_eq!(cloned.model, Some("sonnet".to_string()));
        assert_eq!(status.model, cloned.model);
    }

    #[test]
    fn test_worker_status_model_update_on_resolving_conflicts() {
        let mut status = WorkerStatus::idle(3);
        status.model = Some("sonnet".to_string());
        status.state = WorkerState::ResolvingConflicts;

        assert_eq!(status.state, WorkerState::ResolvingConflicts);
        assert_eq!(status.model, Some("sonnet".to_string()));
    }

    #[test]
    fn test_worker_status_multiple_models() {
        let mut status1 = WorkerStatus::idle(4);
        status1.model = Some("opus".to_string());

        let mut status2 = WorkerStatus::idle(5);
        status2.model = Some("haiku".to_string());

        assert_eq!(status1.model, Some("opus".to_string()));
        assert_eq!(status2.model, Some("haiku".to_string()));
    }

    // ── Snapshot tests: WorkerState Display ────────────────────────────

    #[test]
    fn snapshot_worker_state_display_idle() {
        insta::assert_snapshot!(WorkerState::Idle.to_string(), @"idle");
    }

    #[test]
    fn snapshot_worker_state_display_setting_up() {
        insta::assert_snapshot!(WorkerState::SettingUp.to_string(), @"setting up");
    }

    #[test]
    fn snapshot_worker_state_display_implementing() {
        insta::assert_snapshot!(WorkerState::Implementing.to_string(), @"implementing");
    }

    #[test]
    fn snapshot_worker_state_display_reviewing() {
        insta::assert_snapshot!(WorkerState::Reviewing.to_string(), @"reviewing");
    }

    #[test]
    fn snapshot_worker_state_display_verifying() {
        insta::assert_snapshot!(WorkerState::Verifying.to_string(), @"verifying");
    }

    #[test]
    fn snapshot_worker_state_display_merging() {
        insta::assert_snapshot!(WorkerState::Merging.to_string(), @"merging");
    }

    #[test]
    fn snapshot_worker_state_display_resolving_conflicts() {
        insta::assert_snapshot!(WorkerState::ResolvingConflicts.to_string(), @"resolving conflicts");
    }

    // ── Snapshot tests: WorkerState color mapping ──────────────────────

    #[test]
    fn snapshot_worker_state_color_idle() {
        insta::assert_snapshot!(format!("{:?}", WorkerState::Idle.color()), @"DarkGray");
    }

    #[test]
    fn snapshot_worker_state_color_setting_up() {
        insta::assert_snapshot!(format!("{:?}", WorkerState::SettingUp.color()), @"Blue");
    }

    #[test]
    fn snapshot_worker_state_color_implementing() {
        insta::assert_snapshot!(format!("{:?}", WorkerState::Implementing.color()), @"Cyan");
    }

    #[test]
    fn snapshot_worker_state_color_reviewing() {
        insta::assert_snapshot!(format!("{:?}", WorkerState::Reviewing.color()), @"Yellow");
    }

    #[test]
    fn snapshot_worker_state_color_verifying() {
        insta::assert_snapshot!(format!("{:?}", WorkerState::Verifying.color()), @"Magenta");
    }

    #[test]
    fn snapshot_worker_state_color_merging() {
        insta::assert_snapshot!(format!("{:?}", WorkerState::Merging.color()), @"Green");
    }

    #[test]
    fn snapshot_worker_state_color_resolving_conflicts() {
        insta::assert_snapshot!(format!("{:?}", WorkerState::ResolvingConflicts.color()), @"Red");
    }

    // ── Snapshot tests: WorkerState icon mapping ───────────────────────

    #[test]
    fn snapshot_worker_state_icon_idle() {
        let (icon, color) = WorkerState::Idle.icon();
        insta::assert_snapshot!(format!("icon: '{icon}', color: {color:?}"), @"icon: '○', color: DarkGray");
    }

    #[test]
    fn snapshot_worker_state_icon_setting_up() {
        let (icon, color) = WorkerState::SettingUp.icon();
        insta::assert_snapshot!(format!("icon: '{icon}', color: {color:?}"), @"icon: '⚙', color: Blue");
    }

    #[test]
    fn snapshot_worker_state_icon_implementing() {
        let (icon, color) = WorkerState::Implementing.icon();
        insta::assert_snapshot!(format!("icon: '{icon}', color: {color:?}"), @"icon: '●', color: Cyan");
    }

    #[test]
    fn snapshot_worker_state_icon_reviewing() {
        let (icon, color) = WorkerState::Reviewing.icon();
        insta::assert_snapshot!(format!("icon: '{icon}', color: {color:?}"), @"icon: '◎', color: Yellow");
    }

    #[test]
    fn snapshot_worker_state_icon_verifying() {
        let (icon, color) = WorkerState::Verifying.icon();
        insta::assert_snapshot!(format!("icon: '{icon}', color: {color:?}"), @"icon: '◉', color: Magenta");
    }

    #[test]
    fn snapshot_worker_state_icon_merging() {
        let (icon, color) = WorkerState::Merging.icon();
        insta::assert_snapshot!(format!("icon: '{icon}', color: {color:?}"), @"icon: '⊕', color: Green");
    }

    #[test]
    fn snapshot_worker_state_icon_resolving_conflicts() {
        let (icon, color) = WorkerState::ResolvingConflicts.icon();
        insta::assert_snapshot!(format!("icon: '{icon}', color: {color:?}"), @"icon: '⚡', color: Red");
    }

    // ── Snapshot tests: WorkerStatus full rendering ─────────────────────

    /// Helper: Format WorkerStatus as debug snapshot string.
    fn format_worker_status(ws: &WorkerStatus) -> String {
        format!(
            "WorkerStatus {{\n\
             │ state: {:?}\n\
             │ task_id: {:?}\n\
             │ component: {:?}\n\
             │ phase: {:?}\n\
             │ model: {:?}\n\
             │ cost_usd: ${:.4}\n\
             │ input_tokens: {}\n\
             │ output_tokens: {}\n\
             │ verify_profiles: {:?}\n\
             }}",
            ws.state,
            ws.task_id,
            ws.component,
            ws.phase,
            ws.model,
            ws.cost_usd,
            ws.input_tokens,
            ws.output_tokens,
            ws.verify_profiles
        )
    }

    #[test]
    fn snapshot_worker_status_full_data() {
        let status = WorkerStatus {
            state: WorkerState::Implementing,
            task_id: Some("66.2".to_string()),
            component: Some("tests".to_string()),
            phase: Some(WorkerPhase::Implement),
            model: Some("claude-sonnet-4-5-20250929".to_string()),
            cost_usd: 0.1234,
            input_tokens: 5000,
            output_tokens: 3000,
            verify_profiles: Vec::new(),
        };

        insta::assert_snapshot!(format_worker_status(&status));
    }

    #[test]
    fn snapshot_worker_status_minimal_data() {
        let status = WorkerStatus {
            state: WorkerState::Idle,
            task_id: Some("1.1".to_string()),
            component: None,
            phase: None,
            model: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            verify_profiles: Vec::new(),
        };

        insta::assert_snapshot!(format_worker_status(&status));
    }

    #[test]
    fn snapshot_worker_status_verifying_with_profiles() {
        let status = WorkerStatus {
            state: WorkerState::Verifying,
            task_id: Some("42.1".to_string()),
            component: Some("core".to_string()),
            phase: Some(WorkerPhase::Verify),
            model: Some("claude-opus-4-6".to_string()),
            cost_usd: 0.5678,
            input_tokens: 12000,
            output_tokens: 8500,
            verify_profiles: vec![
                ("lint".to_string(), Some(true)),
                ("test".to_string(), Some(false)),
                ("build".to_string(), None),
            ],
        };

        insta::assert_snapshot!(format_worker_status(&status));
    }

    #[test]
    fn snapshot_worker_status_resolving_conflicts() {
        let status = WorkerStatus {
            state: WorkerState::ResolvingConflicts,
            task_id: Some("99.9".to_string()),
            component: Some("api".to_string()),
            phase: None,
            model: Some("claude-opus-4-6".to_string()),
            cost_usd: 0.9999,
            input_tokens: 25000,
            output_tokens: 15000,
            verify_profiles: Vec::new(),
        };

        insta::assert_snapshot!(format_worker_status(&status));
    }

    #[test]
    fn snapshot_worker_status_state_transition_idle_to_implementing() {
        let mut status = WorkerStatus::idle(1);
        insta::assert_snapshot!("transition_1_idle", format_worker_status(&status));

        // Transition to Implementing
        status.state = WorkerState::Implementing;
        status.task_id = Some("10.1".to_string());
        status.component = Some("ui".to_string());
        status.phase = Some(WorkerPhase::Implement);
        status.model = Some("claude-sonnet-4-5-20250929".to_string());

        insta::assert_snapshot!("transition_1_implementing", format_worker_status(&status));
    }

    #[test]
    fn snapshot_worker_status_state_transition_implementing_to_verifying() {
        let mut status = WorkerStatus {
            state: WorkerState::Implementing,
            task_id: Some("20.2".to_string()),
            component: Some("backend".to_string()),
            phase: Some(WorkerPhase::Implement),
            model: Some("claude-sonnet-4-5-20250929".to_string()),
            cost_usd: 0.05,
            input_tokens: 2000,
            output_tokens: 1500,
            verify_profiles: Vec::new(),
        };

        insta::assert_snapshot!("transition_2_implementing", format_worker_status(&status));

        // Transition to Verifying
        status.state = WorkerState::Verifying;
        status.phase = Some(WorkerPhase::Verify);
        status.cost_usd = 0.08;
        status.input_tokens = 3000;
        status.output_tokens = 2200;
        status.verify_profiles = vec![
            ("format".to_string(), Some(true)),
            ("clippy".to_string(), None),
        ];

        insta::assert_snapshot!("transition_2_verifying", format_worker_status(&status));
    }

    #[test]
    fn snapshot_worker_status_state_transition_verifying_to_done() {
        let mut status = WorkerStatus {
            state: WorkerState::Verifying,
            task_id: Some("30.3".to_string()),
            component: Some("db".to_string()),
            phase: Some(WorkerPhase::Verify),
            model: Some("claude-haiku-4-5-20251001".to_string()),
            cost_usd: 0.02,
            input_tokens: 1000,
            output_tokens: 800,
            verify_profiles: vec![
                ("unit-tests".to_string(), Some(true)),
                ("integration-tests".to_string(), Some(true)),
            ],
        };

        insta::assert_snapshot!("transition_3_verifying", format_worker_status(&status));

        // Transition to Merging (done phase)
        status.state = WorkerState::Merging;
        status.phase = None;
        status.cost_usd = 0.025;
        status.input_tokens = 1200;
        status.output_tokens = 900;
        status.verify_profiles = vec![
            ("unit-tests".to_string(), Some(true)),
            ("integration-tests".to_string(), Some(true)),
        ];

        insta::assert_snapshot!("transition_3_merging", format_worker_status(&status));
    }

    // ── Integrity tests ───────────────────────────────────────────────

    /// All WorkerState variants for exhaustive testing.
    fn all_states() -> [WorkerState; 7] {
        [
            WorkerState::Idle,
            WorkerState::SettingUp,
            WorkerState::Implementing,
            WorkerState::Reviewing,
            WorkerState::Verifying,
            WorkerState::Merging,
            WorkerState::ResolvingConflicts,
        ]
    }

    /// Verify that icon().1 (color) == color() for all WorkerState variants.
    #[test]
    fn test_color_icon_consistency() {
        for state in &all_states() {
            let color = state.color();
            let (_, icon_color) = state.icon();
            assert_eq!(
                color, icon_color,
                "Color mismatch for {state:?}: color()={color:?}, icon().1={icon_color:?}"
            );
        }
    }

    /// Verify that each WorkerState has a unique icon symbol.
    #[test]
    fn test_icons_are_unique() {
        let states = all_states();
        let icons: Vec<(&str, &WorkerState)> = states.iter().map(|s| (s.icon().0, s)).collect();
        for (i, (icon_a, state_a)) in icons.iter().enumerate() {
            for (icon_b, state_b) in &icons[i + 1..] {
                assert_ne!(
                    icon_a, icon_b,
                    "Duplicate icon '{icon_a}' for {state_a:?} and {state_b:?}"
                );
            }
        }
    }
}
