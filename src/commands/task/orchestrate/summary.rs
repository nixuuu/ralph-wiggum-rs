#![allow(dead_code)]
use std::time::Duration;

/// Per-task result for the summary table.
#[derive(Debug, Clone)]
pub struct TaskSummaryEntry {
    pub task_id: String,
    pub status: String,
    pub cost_usd: f64,
    pub duration: Duration,
    pub retries: u32,
}

/// Format the end-of-session summary table.
///
/// Output format:
/// ```text
/// ┌────────┬──────────┬──────────┬──────────┬─────────┐
/// │ Task   │ Status   │ Cost     │ Time     │ Retries │
/// ├────────┼──────────┼──────────┼──────────┼─────────┤
/// │ T01    │ Done     │ $0.0420  │ 45s      │ 0       │
/// │ T02    │ Done     │ $0.0380  │ 32s      │ 0       │
/// │ T03    │ Blocked  │ $0.0890  │ 1m20s    │ 3       │
/// ├────────┼──────────┼──────────┼──────────┼─────────┤
/// │ TOTAL  │ 2/3 done │ $0.1690  │ 2m37s    │         │
/// └────────┴──────────┴──────────┴──────────┴─────────┘
/// ```
pub fn format_summary(entries: &[TaskSummaryEntry], wall_clock: Duration) -> String {
    if entries.is_empty() {
        return "No tasks were executed.".to_string();
    }

    // Calculate totals
    let total_cost: f64 = entries.iter().map(|e| e.cost_usd).sum();
    let total_time: Duration = entries.iter().map(|e| e.duration).sum();
    let done_count = entries.iter().filter(|e| e.status == "Done").count();
    let total_count = entries.len();

    // Calculate parallelism speedup
    let speedup = if wall_clock.as_secs_f64() > 0.0 {
        total_time.as_secs_f64() / wall_clock.as_secs_f64()
    } else {
        1.0
    };

    // Column widths
    let task_w = entries
        .iter()
        .map(|e| e.task_id.len())
        .max()
        .unwrap_or(4)
        .max(5);
    let status_w = 8;
    let cost_w = 8;
    let time_w = 8;
    let retries_w = 7;

    let mut lines = Vec::new();

    // Top border
    lines.push(format!(
        "┌{:─<tw$}┬{:─<sw$}┬{:─<cw$}┬{:─<tmw$}┬{:─<rw$}┐",
        "",
        "",
        "",
        "",
        "",
        tw = task_w + 2,
        sw = status_w + 2,
        cw = cost_w + 2,
        tmw = time_w + 2,
        rw = retries_w + 2,
    ));

    // Header
    lines.push(format!(
        "│ {:<tw$} │ {:<sw$} │ {:<cw$} │ {:<tmw$} │ {:<rw$} │",
        "Task",
        "Status",
        "Cost",
        "Time",
        "Retries",
        tw = task_w,
        sw = status_w,
        cw = cost_w,
        tmw = time_w,
        rw = retries_w,
    ));

    // Header separator
    lines.push(format!(
        "├{:─<tw$}┼{:─<sw$}┼{:─<cw$}┼{:─<tmw$}┼{:─<rw$}┤",
        "",
        "",
        "",
        "",
        "",
        tw = task_w + 2,
        sw = status_w + 2,
        cw = cost_w + 2,
        tmw = time_w + 2,
        rw = retries_w + 2,
    ));

    // Task rows
    for entry in entries {
        let time_str = format_duration(entry.duration);
        let cost_str = format!("${:.4}", entry.cost_usd);
        lines.push(format!(
            "│ {:<tw$} │ {:<sw$} │ {:<cw$} │ {:<tmw$} │ {:<rw$} │",
            entry.task_id,
            entry.status,
            cost_str,
            time_str,
            entry.retries,
            tw = task_w,
            sw = status_w,
            cw = cost_w,
            tmw = time_w,
            rw = retries_w,
        ));
    }

    // Totals separator
    lines.push(format!(
        "├{:─<tw$}┼{:─<sw$}┼{:─<cw$}┼{:─<tmw$}┼{:─<rw$}┤",
        "",
        "",
        "",
        "",
        "",
        tw = task_w + 2,
        sw = status_w + 2,
        cw = cost_w + 2,
        tmw = time_w + 2,
        rw = retries_w + 2,
    ));

    // Totals row
    let status_total = format!("{done_count}/{total_count} done");
    let cost_total = format!("${total_cost:.4}");
    let time_total = format_duration(wall_clock);
    lines.push(format!(
        "│ {:<tw$} │ {:<sw$} │ {:<cw$} │ {:<tmw$} │ {:<rw$} │",
        "TOTAL",
        status_total,
        cost_total,
        time_total,
        "",
        tw = task_w,
        sw = status_w,
        cw = cost_w,
        tmw = time_w,
        rw = retries_w,
    ));

    // Bottom border
    lines.push(format!(
        "└{:─<tw$}┴{:─<sw$}┴{:─<cw$}┴{:─<tmw$}┴{:─<rw$}┘",
        "",
        "",
        "",
        "",
        "",
        tw = task_w + 2,
        sw = status_w + 2,
        cw = cost_w + 2,
        tmw = time_w + 2,
        rw = retries_w + 2,
    ));

    // Speedup metric
    lines.push(format!(
        "\nParallelism speedup: {speedup:.1}x (sum of task times / wall clock)"
    ));

    lines.join("\n")
}

/// Format a duration as a human-readable string.
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(Duration::from_secs(90)), "1m30s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h1m");
    }

    #[test]
    fn test_format_summary_empty() {
        let result = format_summary(&[], Duration::from_secs(60));
        assert_eq!(result, "No tasks were executed.");
    }

    #[test]
    fn test_format_summary_single_task() {
        let entries = vec![TaskSummaryEntry {
            task_id: "T01".to_string(),
            status: "Done".to_string(),
            cost_usd: 0.042,
            duration: Duration::from_secs(45),
            retries: 0,
        }];
        let result = format_summary(&entries, Duration::from_secs(45));

        assert!(result.contains("T01"));
        assert!(result.contains("Done"));
        assert!(result.contains("$0.042"));
        assert!(result.contains("45s"));
        assert!(result.contains("TOTAL"));
        assert!(result.contains("1/1 done"));
        assert!(result.contains("Parallelism speedup: 1.0x"));
    }

    #[test]
    fn test_format_summary_multiple_tasks() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.038,
                duration: Duration::from_secs(32),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T03".to_string(),
                status: "Blocked".to_string(),
                cost_usd: 0.089,
                duration: Duration::from_secs(80),
                retries: 3,
            },
        ];

        // Wall clock is less than sum of task times (parallel execution)
        let result = format_summary(&entries, Duration::from_secs(100));

        assert!(result.contains("T01"));
        assert!(result.contains("T02"));
        assert!(result.contains("T03"));
        assert!(result.contains("2/3 done"));
        assert!(result.contains("Parallelism speedup:"));

        // speedup = (45+32+80)/100 = 1.57
        assert!(result.contains("1.6x"));
    }

    #[test]
    fn test_format_summary_has_box_drawing() {
        let entries = vec![TaskSummaryEntry {
            task_id: "T01".to_string(),
            status: "Done".to_string(),
            cost_usd: 0.01,
            duration: Duration::from_secs(10),
            retries: 0,
        }];
        let result = format_summary(&entries, Duration::from_secs(10));

        assert!(result.contains('┌'));
        assert!(result.contains('┐'));
        assert!(result.contains('└'));
        assert!(result.contains('┘'));
        assert!(result.contains('│'));
        assert!(result.contains('─'));
        assert!(result.contains('├'));
        assert!(result.contains('┤'));
    }
}
