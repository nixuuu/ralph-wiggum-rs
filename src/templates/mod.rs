pub const PRD_PROMPT: &str = include_str!("prd_prompt.md");
pub const ADD_PROMPT: &str = include_str!("add_prompt.md");
pub const EDIT_PROMPT: &str = include_str!("edit_prompt.md");
pub const CHANGENOTES_TEMPLATE: &str = include_str!("changenotes.md");
pub const ISSUES_TEMPLATE: &str = include_str!("implementation_issues.md");
pub const QUESTIONS_TEMPLATE: &str = include_str!("open_questions.md");

// Orchestration templates
#[allow(dead_code)]
pub const DEPS_GENERATION_PROMPT: &str = include_str!("deps_generation_prompt.md");
#[allow(dead_code)]
pub const CONFLICT_RESOLUTION_PROMPT: &str = include_str!("conflict_resolution_prompt.md");
#[allow(dead_code)]
pub const WORKER_IMPLEMENT_PROMPT: &str = include_str!("worker_implement_prompt.md");
#[allow(dead_code)]
pub const WORKER_REVIEW_PROMPT: &str = include_str!("worker_review_prompt.md");
#[allow(dead_code)]
pub const WORKER_VERIFY_PROMPT: &str = include_str!("worker_verify_prompt.md");
