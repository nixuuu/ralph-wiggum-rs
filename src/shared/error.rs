use thiserror::Error;

#[derive(Error, Debug)]
pub enum RalphError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("State file error: {0}")]
    StateFile(String),

    #[error("Claude process error: {0}")]
    ClaudeProcess(String),

    #[error("JSON parsing error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("YAML parsing error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Max iterations reached: {0}")]
    MaxIterations(u32),

    #[error("Interrupted by user")]
    Interrupted,

    #[error("Back navigation")]
    Back,

    #[error("Missing required file: {0}")]
    MissingFile(String),

    #[error("Task setup error: {0}")]
    TaskSetup(String),

    #[error("Orchestration error: {0}")]
    Orchestrate(String),

    #[error("Git worktree error: {0}")]
    WorktreeError(String),

    #[error("Merge conflict: {0}")]
    MergeConflict(String),

    #[error("DAG cycle detected: {0:?}")]
    DagCycle(Vec<String>),

    #[error("Lockfile held by another process: {0}")]
    LockfileHeld(String),

    #[error("Session resume error: {0}")]
    SessionResume(String),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Notification error: {0}")]
    Notification(String),

    /// Plik nie jest obsługiwanym formatem obrazu.
    ///
    /// Zawiera ścieżkę do pliku oraz listę obsługiwanych formatów.
    #[error("Nieobsługiwany format obrazu: '{path}'. Obsługiwane formaty: {supported}")]
    UnsupportedImageFormat { path: String, supported: String },

    #[error("Image processing error: {0}")]
    ImageProcessing(String),
}

pub type Result<T> = std::result::Result<T, RalphError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn test_error_display_messages() {
        // Test wszystkich wariantów RalphError
        let errors = vec![
            (
                RalphError::Config("invalid timeout".to_string()),
                "Configuration error: invalid timeout",
            ),
            (
                RalphError::StateFile("corrupted state.json".to_string()),
                "State file error: corrupted state.json",
            ),
            (
                RalphError::ClaudeProcess("process exited with code 1".to_string()),
                "Claude process error: process exited with code 1",
            ),
            (
                RalphError::MaxIterations(100),
                "Max iterations reached: 100",
            ),
            (RalphError::Interrupted, "Interrupted by user"),
            (RalphError::Back, "Back navigation"),
            (
                RalphError::MissingFile("PROGRESS.md".to_string()),
                "Missing required file: PROGRESS.md",
            ),
            (
                RalphError::TaskSetup("task tree is empty".to_string()),
                "Task setup error: task tree is empty",
            ),
            (
                RalphError::Orchestrate("no workers available".to_string()),
                "Orchestration error: no workers available",
            ),
            (
                RalphError::WorktreeError("failed to create worktree".to_string()),
                "Git worktree error: failed to create worktree",
            ),
            (
                RalphError::MergeConflict("conflict in src/main.rs".to_string()),
                "Merge conflict: conflict in src/main.rs",
            ),
            (
                RalphError::DagCycle(vec![
                    "1.1".to_string(),
                    "1.2".to_string(),
                    "1.1".to_string(),
                ]),
                "DAG cycle detected: [\"1.1\", \"1.2\", \"1.1\"]",
            ),
            (
                RalphError::LockfileHeld("PID 12345".to_string()),
                "Lockfile held by another process: PID 12345",
            ),
            (
                RalphError::SessionResume("state file missing".to_string()),
                "Session resume error: state file missing",
            ),
            (
                RalphError::TaskNotFound("2.3.1".to_string()),
                "Task not found: 2.3.1",
            ),
            (
                RalphError::Mcp("connection refused".to_string()),
                "MCP error: connection refused",
            ),
            (
                RalphError::UnsupportedImageFormat {
                    path: "/tmp/photo.bmp".to_string(),
                    supported: "PNG, JPEG, GIF, WebP".to_string(),
                },
                "Nieobsługiwany format obrazu: '/tmp/photo.bmp'. Obsługiwane formaty: PNG, JPEG, GIF, WebP",
            ),
        ];

        for (error, expected_msg) in errors {
            let msg = error.to_string();

            // Komunikat nie jest pusty
            assert!(!msg.is_empty(), "Error message should not be empty");

            // Komunikat zawiera oczekiwaną treść
            assert_eq!(msg, expected_msg, "Error display mismatch for variant");
        }
    }

    #[test]
    fn test_error_source_preservation() {
        // Test wariantów z nested errors (#[from])

        // JsonParse - symulujemy błąd parsowania
        let json_err = serde_json::from_str::<serde_json::Value>("{invalid json").unwrap_err();
        let ralph_err = RalphError::from(json_err);

        let msg = ralph_err.to_string();
        assert!(
            msg.starts_with("JSON parsing error:"),
            "JsonParse error should have proper prefix"
        );
        assert!(!msg.is_empty());

        // Sprawdź że source() zwraca oryginalny błąd
        assert!(
            ralph_err.source().is_some(),
            "JsonParse should preserve source error"
        );

        // YamlParse - symulujemy błąd parsowania YAML
        let yaml_err = serde_yaml::from_str::<serde_yaml::Value>("invalid: [yaml").unwrap_err();
        let ralph_err = RalphError::from(yaml_err);

        let msg = ralph_err.to_string();
        assert!(
            msg.starts_with("YAML parsing error:"),
            "YamlParse error should have proper prefix"
        );
        assert!(!msg.is_empty());
        assert!(
            ralph_err.source().is_some(),
            "YamlParse should preserve source error"
        );

        // Io - symulujemy błąd IO
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let ralph_err = RalphError::from(io_err);

        let msg = ralph_err.to_string();
        assert!(
            msg.starts_with("IO error:"),
            "IO error should have proper prefix"
        );
        assert!(!msg.is_empty());
        assert!(
            ralph_err.source().is_some(),
            "IO error should preserve source error"
        );
    }

    #[test]
    fn test_error_readability() {
        // Test że komunikaty są rzeczywiście czytelne dla użytkownika
        let errors = vec![
            RalphError::Config("timeout must be positive".to_string()),
            RalphError::DagCycle(vec!["A".to_string(), "B".to_string(), "A".to_string()]),
            RalphError::MaxIterations(50),
        ];

        for error in errors {
            let msg = error.to_string();

            // Komunikat zawiera sensowną informację (min 10 znaków)
            assert!(msg.len() >= 10, "Error message too short: '{}'", msg);

            // Komunikat zaczyna się wielką literą lub zawiera prefix
            assert!(
                msg.chars().next().unwrap().is_uppercase() || msg.contains(':'),
                "Error message should be properly formatted: '{}'",
                msg
            );
        }
    }
}
