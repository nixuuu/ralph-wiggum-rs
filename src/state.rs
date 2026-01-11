use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::config::Config;
use crate::error::{RalphError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopState {
    pub iteration: u32,
    #[serde(default = "default_min_iterations")]
    pub min_iterations: u32,
    pub max_iterations: u32,
    pub completion_promise: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<DateTime<Utc>>,
}

fn default_min_iterations() -> u32 {
    1
}

pub struct StateManager {
    state: LoopState,
    prompt: String,
    file_path: PathBuf,
}

impl StateManager {
    pub fn new(config: &Config) -> Self {
        Self {
            state: LoopState {
                iteration: config.starting_iteration,
                min_iterations: config.min_iterations,
                max_iterations: config.max_iterations,
                completion_promise: config.completion_promise.clone(),
                started_at: Some(Utc::now()),
                last_updated: Some(Utc::now()),
            },
            prompt: config.prompt.clone(),
            file_path: config.state_file.clone(),
        }
    }

    /// Parse markdown file with YAML frontmatter
    /// Returns (state, prompt)
    pub fn load_from_file(path: &Path) -> Result<(LoopState, String)> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| RalphError::StateFile(format!("Failed to read state file: {}", e)))?;

        // Find frontmatter boundaries
        let lines: Vec<&str> = content.lines().collect();

        if lines.is_empty() || lines[0] != "---" {
            return Err(RalphError::StateFile(
                "State file must start with --- (YAML frontmatter)".into(),
            ));
        }

        // Find closing ---
        let mut end_idx = None;
        for (i, line) in lines.iter().enumerate().skip(1) {
            if *line == "---" {
                end_idx = Some(i);
                break;
            }
        }

        let end_idx = end_idx.ok_or_else(|| {
            RalphError::StateFile("State file missing closing --- for frontmatter".into())
        })?;

        // Parse YAML frontmatter
        let yaml_content: String = lines[1..end_idx].join("\n");
        let state: LoopState = serde_yaml::from_str(&yaml_content)?;

        // Extract prompt (everything after closing ---)
        let prompt: String = if end_idx + 1 < lines.len() {
            lines[end_idx + 1..].join("\n").trim().to_string()
        } else {
            String::new()
        };

        if prompt.is_empty() {
            return Err(RalphError::StateFile(
                "State file contains no prompt text".into(),
            ));
        }

        Ok((state, prompt))
    }

    /// Save state to markdown file
    pub async fn save(&self) -> Result<()> {
        // Create parent directory if needed
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let yaml = serde_yaml::to_string(&self.state)?;
        let content = format!("---\n{}---\n\n{}", yaml, self.prompt);

        fs::write(&self.file_path, content).await?;
        Ok(())
    }

    /// Increment iteration and save
    pub async fn increment_iteration(&mut self) -> Result<()> {
        self.state.iteration += 1;
        self.state.last_updated = Some(Utc::now());
        self.save().await
    }

    /// Check if max iterations reached
    pub fn is_max_reached(&self) -> bool {
        self.state.max_iterations > 0 && self.state.iteration >= self.state.max_iterations
    }

    /// Check if we've done enough iterations to accept a promise
    pub fn can_accept_promise(&self) -> bool {
        self.state.iteration >= self.state.min_iterations
    }

    pub fn min_iterations(&self) -> u32 {
        self.state.min_iterations
    }

    pub fn iteration(&self) -> u32 {
        self.state.iteration
    }

    pub fn completion_promise(&self) -> &str {
        &self.state.completion_promise
    }

    /// Delete state file (on successful completion)
    pub async fn cleanup(&self) -> Result<()> {
        if self.file_path.exists() {
            fs::remove_file(&self.file_path).await?;
        }
        Ok(())
    }
}
