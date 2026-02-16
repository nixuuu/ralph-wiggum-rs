/// Check if text contains completion promise in <promise>...</promise> tags
#[must_use]
pub fn find_promise(text: &str, promise: &str) -> bool {
    let open_tag = "<promise>";
    let close_tag = "</promise>";

    let mut search_from = 0;
    while let Some(start) = text[search_from..].find(open_tag) {
        let content_start = search_from + start + open_tag.len();
        if let Some(end) = text[content_start..].find(close_tag) {
            let inner = &text[content_start..content_start + end];
            if inner.trim() == promise {
                return true;
            }
            search_from = content_start + end + close_tag.len();
        } else {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_promise_exact() {
        assert!(find_promise("<promise>done</promise>", "done"));
        assert!(find_promise(
            "Some text <promise>done</promise> more text",
            "done"
        ));
    }

    #[test]
    fn test_find_promise_with_whitespace() {
        assert!(find_promise("<promise> done </promise>", "done"));
        assert!(find_promise("<promise>\ndone\n</promise>", "done"));
    }

    #[test]
    fn test_find_promise_custom() {
        assert!(find_promise(
            "<promise>task completed</promise>",
            "task completed"
        ));
    }

    #[test]
    fn test_find_promise_not_found() {
        assert!(!find_promise("no promise here", "done"));
        assert!(!find_promise("<promise>wrong</promise>", "done"));
    }

    #[test]
    fn test_find_promise_special_chars() {
        assert!(find_promise("<promise>done!</promise>", "done!"));
        assert!(find_promise(
            "<promise>task (done)</promise>",
            "task (done)"
        ));
    }

    // Edge cases tests (task 57.3)

    /// Test: promise jako substring dłuższego słowa nie powinien matchować
    #[test]
    fn test_find_promise_partial_match_not_found() {
        // "done" jako część "undone" nie powinien matchować
        assert!(!find_promise("<promise>undone</promise>", "done"));
        assert!(!find_promise("<promise>donezo</promise>", "done"));
        assert!(!find_promise("<promise>not_done_yet</promise>", "done"));

        // Tylko exact match (z trim) powinien działać
        assert!(find_promise("<promise>done</promise>", "done"));
        assert!(find_promise("<promise> done </promise>", "done"));
    }

    /// Test: promise w środku linii z otaczającym tekstem
    #[test]
    fn test_find_promise_middle_of_line() {
        assert!(find_promise(
            "Beginning text <promise>done</promise> ending text",
            "done"
        ));
        assert!(find_promise(
            "Multiple words before <promise>complete</promise> and after.",
            "complete"
        ));
    }

    /// Test: unicode znaki wokół promise tagów i wewnątrz
    #[test]
    fn test_find_promise_with_unicode() {
        // Unicode dookoła tagów
        assert!(find_promise("🎉<promise>done</promise>🎊", "done"));
        assert!(find_promise(
            "Cześć <promise>gotowe</promise> świat!",
            "gotowe"
        ));

        // Unicode wewnątrz promise (powinien matchować dokładnie)
        assert!(find_promise("<promise>完成</promise>", "完成"));
        assert!(find_promise("<promise>готово</promise>", "готово"));
        assert!(find_promise("<promise>🎉done🎉</promise>", "🎉done🎉"));

        // Unicode w otaczającym tekście nie powinien wpływać na matching
        assert!(find_promise(
            "テキスト<promise>done</promise>もっと",
            "done"
        ));
    }

    /// Test: pusty string input
    #[test]
    fn test_find_promise_empty_input() {
        assert!(!find_promise("", "done"));
        assert!(!find_promise("", ""));
        assert!(!find_promise("<promise></promise>", "done"));

        // Edge: pusty promise string (tylko whitespace)
        assert!(find_promise("<promise>  </promise>", ""));
        assert!(find_promise("<promise>\n\t</promise>", ""));
    }

    /// Test: bardzo długi output (100KB) z promise na końcu
    #[test]
    fn test_find_promise_large_output() {
        // Buduj 100KB+ tekstu - każda linia to ~100 bajtów, potrzebujemy ~1100 linii
        let mut large_text = String::with_capacity(120_000);

        // Dodaj wystarczająco dużo tekstu aby przekroczyć 100KB
        // Linia ma około 95-105 bajtów, więc 1100 linii da nam ~105KB+
        for i in 0..1100 {
            large_text.push_str(&format!(
                "Line {:04}: Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt.\n",
                i
            ));
        }

        // Dodaj promise na końcu
        large_text.push_str("<promise>done</promise>");

        // Weryfikuj rozmiar przed testem
        assert!(
            large_text.len() > 100_000,
            "Text should be > 100KB, got {} bytes",
            large_text.len()
        );

        // Weryfikuj że promise jest znaleziony nawet w dużym tekście
        assert!(find_promise(&large_text, "done"));
    }

    /// Test: wielokrotne promises w jednym output
    #[test]
    fn test_find_promise_multiple_promises() {
        // Pierwsze wystąpienie powinno wystarczyć
        assert!(find_promise(
            "<promise>done</promise> some text <promise>done</promise>",
            "done"
        ));

        // Różne promises - szukaj konkretnego
        let text = r#"
            First: <promise>started</promise>
            Second: <promise>in_progress</promise>
            Third: <promise>done</promise>
        "#;
        assert!(find_promise(text, "started"));
        assert!(find_promise(text, "in_progress"));
        assert!(find_promise(text, "done"));
        assert!(!find_promise(text, "cancelled"));

        // Wiele różnych promises - funkcja znajduje pierwszy matching
        assert!(find_promise(
            "<promise>wrong</promise><promise>correct</promise>",
            "correct"
        ));
    }

    /// Test: promise z newlines i różnymi whitespace
    #[test]
    fn test_find_promise_various_whitespace() {
        assert!(find_promise("<promise>\n\tdone\n</promise>", "done"));
        assert!(find_promise("<promise>  \n  done  \n  </promise>", "done"));
        assert!(find_promise("<promise>\r\ndone\r\n</promise>", "done"));
    }

    /// Test: niepełne tagi (nie powinny matchować)
    #[test]
    fn test_find_promise_incomplete_tags() {
        assert!(!find_promise("<promise>done", "done"));
        assert!(!find_promise("done</promise>", "done"));
        assert!(!find_promise("<promise>done<promise>", "done"));
        assert!(!find_promise("</promise>done</promise>", "done"));
    }
}
