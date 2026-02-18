//! Responsive breakpoint system for terminal width adaptation.
//!
//! Provides three breakpoints:
//! - Large (≥120 cols): Sidebar + output + full status bar
//! - Medium (80-119 cols): Collapsed sidebar (icons only) + output
//! - Small (<80 cols): No sidebar, compact status bar
//!
//! The system detects terminal width and returns appropriate layout areas.

use ratatui::layout::{Constraint, Layout, Rect};

/// Responsive breakpoints based on terminal width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breakpoint {
    /// Large: ≥120 columns — full layout (sidebar + output + full status bar)
    Large,
    /// Medium: 80-119 columns — collapsed sidebar (icons) + output
    Medium,
    /// Small: <80 columns — no sidebar, compact status bar
    Small,
}

impl Breakpoint {
    /// Detect breakpoint from terminal width (in columns).
    ///
    /// # Examples
    /// ```
    /// use ralph_wiggum::tui::responsive::Breakpoint;
    ///
    /// assert_eq!(Breakpoint::detect(120), Breakpoint::Large);
    /// assert_eq!(Breakpoint::detect(119), Breakpoint::Medium);
    /// assert_eq!(Breakpoint::detect(80), Breakpoint::Medium);
    /// assert_eq!(Breakpoint::detect(79), Breakpoint::Small);
    /// ```
    pub fn detect(width: u16) -> Self {
        match width {
            w if w >= 120 => Breakpoint::Large,
            w if w >= 80 => Breakpoint::Medium,
            _ => Breakpoint::Small,
        }
    }

    /// Human-readable name for the breakpoint (used in debug/logging).
    #[allow(dead_code)]
    pub fn name(&self) -> &'static str {
        match self {
            Breakpoint::Large => "Large",
            Breakpoint::Medium => "Medium",
            Breakpoint::Small => "Small",
        }
    }
}

/// Layout areas divided by responsive breakpoint.
///
/// Areas are optional to accommodate Small breakpoint (no sidebar).
/// All areas except `status_bar` consume height; `content` is calculated residually.
#[derive(Debug, Clone)]
pub struct LayoutAreas {
    /// Header area (typically 1 line, optional)
    pub header: Option<Rect>,
    /// Sidebar area (only in Large breakpoint, width ~20-25%)
    pub sidebar: Option<Rect>,
    /// Main content area (remaining width, full height minus header/status)
    pub content: Rect,
    /// Status bar area (fixed ~1-2 lines at bottom)
    pub status_bar: Rect,
}

impl LayoutAreas {
    /// Calculate layout areas for a given terminal area and breakpoint.
    ///
    /// # Layout Rules
    /// - **Large (≥120)**: header(1) + [sidebar(20%) + content(80%)] + status(2)
    /// - **Medium (80-119)**: [collapsed_sidebar(3) + content] + status(1)
    /// - **Small (<80)**: content + status(1)
    ///
    /// # Examples
    /// ```
    /// use ratatui::layout::Rect;
    /// use ralph_wiggum::tui::responsive::{Breakpoint, LayoutAreas};
    ///
    /// let area = Rect { x: 0, y: 0, width: 120, height: 30 };
    /// let layout = LayoutAreas::for_breakpoint(Breakpoint::Large, area);
    /// assert!(layout.sidebar.is_some());
    /// assert!(layout.header.is_some());
    /// ```
    pub fn for_breakpoint(bp: Breakpoint, area: Rect) -> Self {
        match bp {
            Breakpoint::Large => Self::large_layout(area),
            Breakpoint::Medium => Self::medium_layout(area),
            Breakpoint::Small => Self::small_layout(area),
        }
    }

    /// Large layout: header + [sidebar(20%) | content(80%)] + status(2).
    fn large_layout(area: Rect) -> Self {
        let header_height = if area.height >= 6 { 1 } else { 0 };
        let status_height = if area.height >= 6 { 2 } else { 1 };

        // Vertical split: header | content+sidebar | status
        let v_chunks = Layout::vertical([
            Constraint::Length(header_height),
            Constraint::Min(0), // content area fills middle
            Constraint::Length(status_height),
        ])
        .split(area);

        let header = if header_height > 0 {
            Some(v_chunks[0])
        } else {
            None
        };

        let content_area = v_chunks[1];
        let status_bar = v_chunks[2];

        // Horizontal split: sidebar(20%) | content(80%)
        let sidebar_width = (content_area.width as f64 * 0.20).ceil() as u16;
        let h_chunks = Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(0)])
            .split(content_area);

        let sidebar = if sidebar_width > 0 {
            Some(h_chunks[0])
        } else {
            None
        };
        let content = h_chunks[1];

        LayoutAreas {
            header,
            sidebar,
            content,
            status_bar,
        }
    }

    /// Medium layout: [collapsed_sidebar(3 cols) | content] + status(1).
    fn medium_layout(area: Rect) -> Self {
        let status_height = if area.height >= 3 { 1 } else { 0 };

        // Vertical split: content+sidebar | status
        let v_chunks = Layout::vertical([
            Constraint::Min(0), // content area
            Constraint::Length(status_height),
        ])
        .split(area);

        let content_area = v_chunks[0];
        let status_bar = v_chunks[1];

        // Horizontal split: collapsed_sidebar(3) | content
        let sidebar_width = 3; // icons only
        let h_chunks = Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(0)])
            .split(content_area);

        let sidebar = if h_chunks[0].width > 0 {
            Some(h_chunks[0])
        } else {
            None
        };
        let content = h_chunks[1];

        LayoutAreas {
            header: None,
            sidebar,
            content,
            status_bar,
        }
    }

    /// Small layout: content + status(1), no sidebar.
    fn small_layout(area: Rect) -> Self {
        let status_height = if area.height >= 2 { 1 } else { 0 };

        let v_chunks = Layout::vertical([
            Constraint::Min(0), // full content
            Constraint::Length(status_height),
        ])
        .split(area);

        let content = v_chunks[0];
        let status_bar = v_chunks[1];

        LayoutAreas {
            header: None,
            sidebar: None,
            content,
            status_bar,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== Breakpoint Detection Tests =====

    #[test]
    fn detect_large_at_120_cols() {
        assert_eq!(Breakpoint::detect(120), Breakpoint::Large);
    }

    #[test]
    fn detect_large_at_200_cols() {
        assert_eq!(Breakpoint::detect(200), Breakpoint::Large);
    }

    #[test]
    fn detect_medium_at_119_cols() {
        assert_eq!(Breakpoint::detect(119), Breakpoint::Medium);
    }

    #[test]
    fn detect_medium_at_80_cols() {
        assert_eq!(Breakpoint::detect(80), Breakpoint::Medium);
    }

    #[test]
    fn detect_medium_at_100_cols() {
        assert_eq!(Breakpoint::detect(100), Breakpoint::Medium);
    }

    #[test]
    fn detect_small_at_79_cols() {
        assert_eq!(Breakpoint::detect(79), Breakpoint::Small);
    }

    #[test]
    fn detect_small_at_1_col() {
        assert_eq!(Breakpoint::detect(1), Breakpoint::Small);
    }

    #[test]
    fn breakpoint_name_large() {
        assert_eq!(Breakpoint::Large.name(), "Large");
    }

    #[test]
    fn breakpoint_name_medium() {
        assert_eq!(Breakpoint::Medium.name(), "Medium");
    }

    #[test]
    fn breakpoint_name_small() {
        assert_eq!(Breakpoint::Small.name(), "Small");
    }

    // ===== Large Layout Tests =====

    #[test]
    fn large_layout_has_sidebar() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Large, area);
        assert!(layout.sidebar.is_some());
    }

    #[test]
    fn large_layout_has_header() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Large, area);
        assert!(layout.header.is_some());
    }

    #[test]
    fn large_layout_sidebar_is_20_percent() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Large, area);
        let sidebar_width = layout.sidebar.unwrap().width;
        // Expected width is 20% of 120 = 24 (120 * 0.20 = 24.0, then ceil = 24)
        assert_eq!(sidebar_width, 24);
    }

    #[test]
    fn large_layout_content_is_80_percent() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Large, area);
        let content_width = layout.content.width as f64;
        let sidebar_width = layout.sidebar.unwrap().width as f64;
        let total_width = content_width + sidebar_width;
        // Content should be roughly 80% of content_area
        let percentage = (content_width / total_width) * 100.0;
        assert!(percentage > 75.0 && percentage < 85.0);
    }

    #[test]
    fn large_layout_status_is_2_lines() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Large, area);
        assert_eq!(layout.status_bar.height, 2);
    }

    #[test]
    fn large_layout_small_height_no_header() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 5,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Large, area);
        assert!(layout.header.is_none());
    }

    // ===== Medium Layout Tests =====

    #[test]
    fn medium_layout_has_sidebar() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Medium, area);
        assert!(layout.sidebar.is_some());
    }

    #[test]
    fn medium_layout_sidebar_is_3_cols() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Medium, area);
        assert_eq!(layout.sidebar.unwrap().width, 3);
    }

    #[test]
    fn medium_layout_no_header() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Medium, area);
        assert!(layout.header.is_none());
    }

    #[test]
    fn medium_layout_status_is_1_line() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Medium, area);
        assert_eq!(layout.status_bar.height, 1);
    }

    // ===== Small Layout Tests =====

    #[test]
    fn small_layout_no_sidebar() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 70,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Small, area);
        assert!(layout.sidebar.is_none());
    }

    #[test]
    fn small_layout_no_header() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 70,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Small, area);
        assert!(layout.header.is_none());
    }

    #[test]
    fn small_layout_status_is_1_line() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 70,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Small, area);
        assert_eq!(layout.status_bar.height, 1);
    }

    #[test]
    fn small_layout_content_fills_space() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 70,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Small, area);
        // Content should be the full width
        assert_eq!(layout.content.width, 70);
        // Content height should be area.height - status.height
        assert_eq!(layout.content.height, 29);
    }

    // ===== Boundary Tests =====

    #[test]
    fn boundary_79_is_small() {
        assert_eq!(Breakpoint::detect(79), Breakpoint::Small);
    }

    #[test]
    fn boundary_80_is_medium() {
        assert_eq!(Breakpoint::detect(80), Breakpoint::Medium);
    }

    #[test]
    fn boundary_119_is_medium() {
        assert_eq!(Breakpoint::detect(119), Breakpoint::Medium);
    }

    #[test]
    fn boundary_120_is_large() {
        assert_eq!(Breakpoint::detect(120), Breakpoint::Large);
    }

    // ===== Layout Area Coverage Tests =====

    #[test]
    fn large_layout_areas_do_not_overlap() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Large, area);

        // Verify header, sidebar, content, status do not overlap vertically
        let header_y = layout.header.map(|h| h.y + h.height).unwrap_or(0);
        let content_start_y = layout.content.y;
        let status_start_y = layout.status_bar.y;

        assert!(header_y <= content_start_y || header_y == 0);
        assert!(status_start_y >= content_start_y + layout.content.height);
    }

    #[test]
    fn medium_layout_areas_do_not_overlap() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Medium, area);

        // Verify sidebar and content share same y/height
        if let Some(sidebar) = layout.sidebar {
            assert_eq!(sidebar.y, layout.content.y);
            assert_eq!(sidebar.height, layout.content.height);
        }
        // Verify status is below content
        assert!(layout.status_bar.y >= layout.content.y + layout.content.height);
    }

    #[test]
    fn small_layout_areas_do_not_overlap() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 70,
            height: 30,
        };
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Small, area);

        // Verify content and status do not overlap
        assert!(layout.status_bar.y >= layout.content.y + layout.content.height);
    }

    // ===== Edge Case Tests =====

    #[test]
    fn breakpoint_equality() {
        assert_eq!(Breakpoint::Large, Breakpoint::Large);
        assert_ne!(Breakpoint::Large, Breakpoint::Medium);
    }

    #[test]
    fn breakpoint_copy_trait() {
        let bp = Breakpoint::Large;
        let bp2 = bp;
        assert_eq!(bp, bp2);
    }

    #[test]
    fn detect_zero_width_is_small() {
        assert_eq!(Breakpoint::detect(0), Breakpoint::Small);
    }

    #[test]
    fn zero_area_does_not_panic() {
        let area = Rect::new(0, 0, 0, 0);
        let _ = LayoutAreas::for_breakpoint(Breakpoint::Small, area);
        let _ = LayoutAreas::for_breakpoint(Breakpoint::Medium, area);
        let _ = LayoutAreas::for_breakpoint(Breakpoint::Large, area);
    }

    #[test]
    fn height_1_does_not_panic() {
        let area = Rect::new(0, 0, 120, 1);
        let layout = LayoutAreas::for_breakpoint(Breakpoint::Large, area);
        // With height=1, no room for header — should degrade gracefully
        assert!(layout.header.is_none());
    }
}
