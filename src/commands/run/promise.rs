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
}
