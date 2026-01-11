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
}

pub type Result<T> = std::result::Result<T, RalphError>;
