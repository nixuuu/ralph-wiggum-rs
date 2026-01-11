const SYSTEM_WRAPPER_TEMPLATE: &str = r#"## Ralph Wiggum Loop - Instrukcje systemowe

To jest iteracja #{iteration} pętli ralph-wiggum.

Pracujesz iteracyjnie w pętli. Oznacza to, że to samo zadanie dostajesz
kilka razy, aby w kolejnych iteracjach sprawdzić czy poprzednia iteracja
na pewno zrobiła to co było do zrobienia.

**Jak zakończyć pętlę:**
Gdy uznasz, że zadanie jest w pełni ukończone i zweryfikowane, zakończ
swoją odpowiedź tagiem:
<promise>{promise}</promise>

WAŻNE: Używaj tagu <promise> TYLKO gdy jesteś pewien, że zadanie jest
kompletne. Nie kłam aby wyjść z pętli!

---

## Zadanie użytkownika:

{prompt}"#;

/// Build a wrapped prompt with system instructions
pub fn build_wrapped_prompt(user_prompt: &str, completion_promise: &str, iteration: u32) -> String {
    SYSTEM_WRAPPER_TEMPLATE
        .replace("{iteration}", &iteration.to_string())
        .replace("{promise}", completion_promise)
        .replace("{prompt}", user_prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_wrapped_prompt() {
        let result = build_wrapped_prompt("Write tests", "done", 1);
        assert!(result.contains("Write tests"));
        assert!(result.contains("<promise>done</promise>"));
        assert!(result.contains("Ralph Wiggum Loop"));
        assert!(result.contains("iteracja #1"));
    }

    #[test]
    fn test_build_wrapped_prompt_iteration_2() {
        let result = build_wrapped_prompt("Write tests", "done", 2);
        assert!(result.contains("iteracja #2"));
    }
}
