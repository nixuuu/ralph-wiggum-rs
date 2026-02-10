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

    #[error("Missing required file: {0}")]
    MissingFile(String),

    #[error("Task setup error: {0}")]
    TaskSetup(String),

    #[error("Orchestration error: {0}")]
    #[allow(dead_code)]
    Orchestrate(String),

    #[error("Git worktree error: {0}")]
    #[allow(dead_code)]
    WorktreeError(String),

    #[error("Merge conflict: {0}")]
    #[allow(dead_code)]
    MergeConflict(String),

    #[error("DAG cycle detected: {0:?}")]
    #[allow(dead_code)]
    DagCycle(Vec<String>),

    #[error("Lockfile held by another process: {0}")]
    #[allow(dead_code)]
    LockfileHeld(String),

    #[error("Session resume error: {0}")]
    #[allow(dead_code)]
    SessionResume(String),
}

pub type Result<T> = std::result::Result<T, RalphError>;
