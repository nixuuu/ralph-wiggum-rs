use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use crate::shared::error::{RalphError, Result};

/// Resolve input from file, prompt, or stdin.
/// Priority: file > prompt > stdin (if not terminal) > error
pub fn resolve_input(file: Option<&PathBuf>, prompt: Option<&str>) -> Result<String> {
    // 1. File has highest priority
    if let Some(path) = file {
        return std::fs::read_to_string(path).map_err(|e| {
            RalphError::TaskSetup(format!("Failed to read file {}: {}", path.display(), e))
        });
    }

    // 2. Prompt text
    if let Some(text) = prompt {
        return Ok(text.to_string());
    }

    // 3. Stdin if piped (not a terminal)
    if !io::stdin().is_terminal() {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| RalphError::TaskSetup(format!("Failed to read stdin: {}", e)))?;
        if !buf.trim().is_empty() {
            return Ok(buf);
        }
    }

    Err(RalphError::TaskSetup(
        "No input provided. Use --file <path>, --prompt <text>, or pipe via stdin.".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_resolve_input_prompt() {
        let result = resolve_input(None, Some("Hello world"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello world");
    }

    #[test]
    fn test_resolve_input_missing_file() {
        let path = PathBuf::from("/nonexistent/file.md");
        let result = resolve_input(Some(&path), None);
        assert!(result.is_err());
    }
}
