/// Get icon for a tool name
pub fn tool_icon(name: &str, nerd_font: bool) -> &'static str {
    if nerd_font {
        match name {
            "Read" => "\u{f02d}",            //  nf-fa-book
            "Write" => "\u{f044}",           //  nf-fa-pencil_square_o
            "Edit" => "\u{f040}",            //  nf-fa-pencil
            "Bash" => "\u{f120}",            //  nf-fa-terminal
            "Glob" => "\u{f002}",            //  nf-fa-search
            "Grep" => "\u{f0b0}",            //  nf-fa-filter
            "Task" => "\u{f085}",            //  nf-fa-cogs
            "WebFetch" => "\u{f0ac}",        //  nf-fa-globe
            "WebSearch" => "\u{f002}",       //  nf-fa-search
            "TodoWrite" => "\u{f046}",       //  nf-fa-check_square_o
            "AskUserQuestion" => "\u{f059}", //  nf-fa-question_circle
            _ => "\u{f0ad}",                 //  nf-fa-wrench
        }
    } else {
        match name {
            "Read" => "[R]",
            "Write" => "[W]",
            "Edit" => "[E]",
            "Bash" => "[>]",
            "Glob" => "[?]",
            "Grep" => "[/]",
            "Task" => "[T]",
            "WebFetch" => "[~]",
            "WebSearch" => "[S]",
            "TodoWrite" => "[L]",
            "AskUserQuestion" => "[?]",
            _ => "[*]",
        }
    }
}

/// Check mark for completion status
pub fn status_check(nerd_font: bool) -> &'static str {
    if nerd_font {
        "\u{f00c}" //  nf-fa-check
    } else {
        "[v]"
    }
}

/// Cross mark for failure status
pub fn status_fail(nerd_font: bool) -> &'static str {
    if nerd_font {
        "\u{f00d}" //  nf-fa-times
    } else {
        "[x]"
    }
}

/// Pause icon for interrupted status
pub fn status_pause(nerd_font: bool) -> &'static str {
    if nerd_font {
        "\u{f04c}" //  nf-fa-pause
    } else {
        "[||]"
    }
}

/// Clock icon for elapsed time
pub fn status_clock(nerd_font: bool) -> &'static str {
    if nerd_font {
        "\u{f017}" //  nf-fa-clock_o
    } else {
        "[t]"
    }
}

/// Speed/bolt icon for task throughput
pub fn status_speed(nerd_font: bool) -> &'static str {
    if nerd_font {
        "\u{f0e7}" //  nf-fa-bolt
    } else {
        "^"
    }
}
