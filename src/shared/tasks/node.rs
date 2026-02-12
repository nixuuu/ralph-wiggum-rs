use serde::{Deserialize, Serialize};

use crate::shared::progress::TaskStatus;

/// Recursive task node — can be a leaf (has `status`) or a parent (has `subtasks`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implementation_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<TaskNode>,
}

/// Flattened view of a leaf task (no subtasks).
#[derive(Debug, Clone)]
pub struct LeafTask {
    pub id: String,
    pub name: String,
    pub component: String,
    pub status: TaskStatus,
    pub deps: Vec<String>,
    pub model: Option<String>,
    pub description: Option<String>,
    pub related_files: Vec<String>,
    pub implementation_steps: Vec<String>,
}

// ── TaskNode helpers ────────────────────────────────────────────────

impl TaskNode {
    /// Whether this node is a leaf (has status, no subtasks).
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.subtasks.is_empty()
    }
}
