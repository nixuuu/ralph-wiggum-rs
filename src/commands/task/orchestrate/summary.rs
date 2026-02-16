use std::time::Duration;

use unicode_width::UnicodeWidthStr;

use crate::shared::diagnostics;

/// Per-task result for the summary table.
#[derive(Debug, Clone)]
pub struct TaskSummaryEntry {
    pub task_id: String,
    pub status: String,
    pub cost_usd: f64,
    pub duration: Duration,
    pub retries: u32,
}

/// Format the end-of-session summary table (plain-text version for log output).
///
/// TUI version: see `completion_summary.rs` for ratatui rendering.
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
#[allow(dead_code)] // Used in tests; will be used for diagnostic log output
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

    // Pre-calculate status total for width calculation
    let status_total = format!("{done_count}/{total_count} done");

    // Column widths (using unicode display width for proper alignment)
    let task_w = entries
        .iter()
        .map(|e| e.task_id.width())
        .max()
        .unwrap_or(4)
        .max("Task".width())
        .max("TOTAL".width());
    let status_w = entries
        .iter()
        .map(|e| e.status.width())
        .max()
        .unwrap_or(6)
        .max("Status".width())
        .max(status_total.width());
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

        // Pad strings to account for unicode width
        let task_pad = task_w.saturating_sub(entry.task_id.width());
        let status_pad = status_w.saturating_sub(entry.status.width());

        lines.push(format!(
            "│ {}{} │ {}{} │ {:<cw$} │ {:<tmw$} │ {:<rw$} │",
            entry.task_id,
            " ".repeat(task_pad),
            entry.status,
            " ".repeat(status_pad),
            cost_str,
            time_str,
            entry.retries,
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
    let cost_total = format!("${total_cost:.4}");
    let time_total = format_duration(total_time); // Fixed: use sum of task times, not wall clock

    // Pad totals row strings to account for unicode width
    let total_task_pad = task_w.saturating_sub("TOTAL".width());
    let total_status_pad = status_w.saturating_sub(status_total.width());

    lines.push(format!(
        "│ {}{} │ {}{} │ {:<cw$} │ {:<tmw$} │ {:<rw$} │",
        "TOTAL",
        " ".repeat(total_task_pad),
        status_total,
        " ".repeat(total_status_pad),
        cost_total,
        time_total,
        "",
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

    // Add log file path if available
    if let Some(log_path) = diagnostics::log_file_path() {
        lines.push(format!("Diagnostic log: {}", log_path.display()));
    }

    lines.join("\n")
}

/// Format a duration as a human-readable string.
#[allow(dead_code)] // Called by format_summary which is currently test-only
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

    // ========== Snapshot Tests ==========

    /// Normalizuje ścieżki do plików logów dla deterministycznych snapshotów.
    /// Zamienia rzeczywiste ścieżki jak "/Users/..." na stałą "DIAGNOSTIC_LOG_PATH".
    fn normalize_log_path(s: &str) -> String {
        let lines: Vec<String> = s
            .lines()
            .map(|line| {
                if line.starts_with("Diagnostic log:") {
                    "Diagnostic log: DIAGNOSTIC_LOG_PATH".to_string()
                } else {
                    line.to_string()
                }
            })
            .collect();
        lines.join("\n")
    }

    /// Snapshot test: Tabela z 1 taskiem (Done, $0.04, 45s, 0 retries)
    #[test]
    fn snapshot_summary_single_task() {
        let entries = vec![TaskSummaryEntry {
            task_id: "1.2".to_string(),
            status: "Done".to_string(),
            cost_usd: 0.04,
            duration: Duration::from_secs(45),
            retries: 0,
        }];
        let output = format_summary(&entries, Duration::from_secs(45));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Tabela z 5 taskami o różnych statusach
    #[test]
    fn snapshot_summary_five_tasks_mixed_status() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "1.1".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0234,
                duration: Duration::from_secs(28),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "1.2".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0567,
                duration: Duration::from_secs(52),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "2.1".to_string(),
                status: "Failed".to_string(),
                cost_usd: 0.0123,
                duration: Duration::from_secs(15),
                retries: 2,
            },
            TaskSummaryEntry {
                task_id: "2.2".to_string(),
                status: "Blocked".to_string(),
                cost_usd: 0.0089,
                duration: Duration::from_secs(10),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "3.1".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0987,
                duration: Duration::from_secs(135),
                retries: 1,
            },
        ];
        let output = format_summary(&entries, Duration::from_secs(150));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Tabela z taskiem Failed i retries > 0
    #[test]
    fn snapshot_summary_failed_with_retries() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "4.3".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0456,
                duration: Duration::from_secs(38),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "4.4".to_string(),
                status: "Failed".to_string(),
                cost_usd: 0.1234,
                duration: Duration::from_secs(95),
                retries: 5,
            },
        ];
        let output = format_summary(&entries, Duration::from_secs(100));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Tabela z bardzo długim task ID
    #[test]
    fn snapshot_summary_long_task_id() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "1.2.3.4.5".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0321,
                duration: Duration::from_secs(42),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "10.20.30".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0654,
                duration: Duration::from_secs(78),
                retries: 1,
            },
        ];
        let output = format_summary(&entries, Duration::from_secs(90));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Total row z sumą kosztów i czasu
    #[test]
    fn snapshot_summary_total_row() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "A".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.1111,
                duration: Duration::from_secs(60),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "B".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.2222,
                duration: Duration::from_secs(90),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "C".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.3333,
                duration: Duration::from_secs(120),
                retries: 0,
            },
        ];
        let output = format_summary(&entries, Duration::from_secs(100));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Tabela z 0 tasków (edge case)
    #[test]
    fn snapshot_summary_empty() {
        let entries = vec![];
        let output = format_summary(&entries, Duration::from_secs(0));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Tabela z zadaniami zawierającymi unicode w ID
    #[test]
    fn snapshot_summary_unicode_task_id() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "α-1".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0425,
                duration: Duration::from_secs(33),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "β-2".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0789,
                duration: Duration::from_secs(67),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "γ-3".to_string(),
                status: "Failed".to_string(),
                cost_usd: 0.0156,
                duration: Duration::from_secs(20),
                retries: 3,
            },
        ];
        let output = format_summary(&entries, Duration::from_secs(80));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Tabela z polskimi znakami w task ID
    #[test]
    fn snapshot_summary_polish_chars_task_id() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "Ążśź-1".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0512,
                duration: Duration::from_secs(41),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "Łódź-2".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0834,
                duration: Duration::from_secs(59),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "Ćwik-3".to_string(),
                status: "Błąd".to_string(), // Polish status too
                cost_usd: 0.0267,
                duration: Duration::from_secs(28),
                retries: 2,
            },
        ];
        let output = format_summary(&entries, Duration::from_secs(75));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Tabela z emoji w task ID
    #[test]
    fn snapshot_summary_emoji_task_id() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "🚀-1".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.0456,
                duration: Duration::from_secs(37),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "🎯-2".to_string(),
                status: "✅ OK".to_string(), // Emoji in status
                cost_usd: 0.0723,
                duration: Duration::from_secs(54),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "💥-3".to_string(),
                status: "❌ Failed".to_string(), // Emoji in status
                cost_usd: 0.0189,
                duration: Duration::from_secs(22),
                retries: 4,
            },
        ];
        let output = format_summary(&entries, Duration::from_secs(70));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Mieszane unicode characters (mixed width)
    #[test]
    fn snapshot_summary_mixed_unicode_width() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "中文-1".to_string(), // CJK characters (double width)
                status: "Done".to_string(),
                cost_usd: 0.0345,
                duration: Duration::from_secs(31),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "こんにちは-2".to_string(), // Japanese hiragana (double width)
                status: "Done".to_string(),
                cost_usd: 0.0678,
                duration: Duration::from_secs(62),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "ASCII-3".to_string(), // Regular ASCII
                status: "Failed".to_string(),
                cost_usd: 0.0123,
                duration: Duration::from_secs(15),
                retries: 1,
            },
            TaskSummaryEntry {
                task_id: "Ż🎯中-4".to_string(), // Mix of all types
                status: "Done".to_string(),
                cost_usd: 0.0901,
                duration: Duration::from_secs(89),
                retries: 0,
            },
        ];
        let output = format_summary(&entries, Duration::from_secs(120));
        let normalized = normalize_log_path(&output);
        insta::assert_snapshot!(normalized);
    }

    /// Snapshot test: Tabela z 50+ taskami (test wydajności i total row)
    ///
    /// Weryfikuje:
    /// - Rendering 50+ wierszy bez overflow
    /// - Total row: suma 50 kosztów i czasów (bez utraty precyzji)
    /// - Formatowanie pozostaje spójne przy dużej ilości danych
    /// - Czas renderowania < 1s (implicit w cargo test)
    #[test]
    fn snapshot_summary_50_plus_tasks() {
        let mut entries = Vec::new();

        // Generuj 55 tasków z różnymi statusami, kosztami i czasami
        for i in 1..=55 {
            let status = match i % 5 {
                0 => "Failed",
                1 => "Done",
                2 => "Done",
                3 => "Blocked",
                _ => "Done",
            };

            let retries = if status == "Failed" { i % 6 } else { 0 };

            // Zróżnicowane koszty: od $0.001 do $0.999
            let cost_usd = (i as f64) * 0.0178 + 0.001;

            // Zróżnicowane czasy: od 5s do 3600s (1h)
            let duration_secs = 5 + (i * 13) % 3600;

            entries.push(TaskSummaryEntry {
                task_id: format!("{}.{}", (i - 1) / 10 + 1, (i - 1) % 10 + 1),
                status: status.to_string(),
                cost_usd,
                duration: Duration::from_secs(duration_secs),
                retries: retries as u32,
            });
        }

        // Wall clock time: symuluj parallelizm (suma czasów / 4)
        let total_duration: Duration = entries.iter().map(|e| e.duration).sum();
        let wall_clock = Duration::from_secs(total_duration.as_secs() / 4);

        let output = format_summary(&entries, wall_clock);
        let normalized = normalize_log_path(&output);

        // Sprawdź że output zawiera wszystkie task IDs
        assert!(output.contains("1.1"));
        assert!(output.contains("5.5"));
        assert!(output.contains("6.5")); // 55th task (task ID 6.5)

        // Sprawdź że total row zawiera poprawną sumę (55 tasków * avg cost)
        let total_cost: f64 = entries.iter().map(|e| e.cost_usd).sum();
        assert!(total_cost > 25.0); // Sanity check: >$25 total
        assert!(output.contains(&format!("${total_cost:.4}")));

        // Sprawdź done count
        let done_count = entries.iter().filter(|e| e.status == "Done").count();
        assert!(output.contains(&format!("{done_count}/55 done")));

        insta::assert_snapshot!(normalized);
    }
}
