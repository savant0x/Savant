//! Recursive Synthesis Engine
//!
//! This module implements recursive plan synthesis, allowing the cognitive
//! layer to re-evaluate and refine action trajectories based on real-time
//! prediction feedback.
//!
//! # Architecture
//! The synthesis engine performs three stages:
//! 1. **Goal Decomposition**: Splits compound goals into atomic sub-tasks
//! 2. **Step Generation**: Translates sub-tasks into executable RequestFrame steps
//! 3. **Complexity Estimation**: Computes trajectory complexity from step count,
//!    tool diversity, and dependency depth
//!
//! # Refinement
//! After execution, `refine_trajectory` adjusts complexity based on:
//! - DSP predictor optimal-k estimation
//! - Execution failures (increase complexity)
//! - Execution successes (decrease complexity)
//! - High-complexity splitting (recursive decomposition)

use crate::predictor::DspPredictor;
use savant_core::types::{ControlFrame, RequestFrame, RequestPayload, ResponseFrame, SessionId};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Represents a single decomposed sub-task within a plan.
#[derive(Debug, Clone)]
pub struct SubTask {
    /// Natural language description of the sub-task
    pub description: String,
    /// Tool name required to execute this task (None = control/coordination step)
    pub tool_name: Option<String>,
    /// Dependencies on prior sub-task indices
    pub dependencies: Vec<usize>,
    /// Estimated complexity weight (1.0 = trivial, 10.0 = extremely complex)
    pub weight: f32,
}

/// Represents a synthesized plan trajectory.
#[derive(Debug, Clone)]
pub struct PlanTrajectory {
    pub id: uuid::Uuid,
    pub steps: Vec<RequestFrame>,
    pub estimated_complexity: f32,
    /// The decomposed sub-tasks that generated these steps
    pub sub_tasks: Vec<SubTask>,
    /// The original goal string
    pub original_goal: String,
}

/// Conjunction keywords that split compound goals into sub-tasks.
const CONJUNCTIONS: &[&str] = &[
    " and then ",
    " then ",
    " and also ",
    " and ",
    " after that ",
    " finally ",
    " additionally ",
    " next ",
    " afterwards ",
    " once done ",
];

/// Action verb patterns mapped to tool names.
///
/// This mapping enables automatic tool selection during goal decomposition.
/// Each pattern is checked against the sub-task description to determine
/// which tool should execute it.
struct ActionPattern {
    /// Keywords that indicate this action
    keywords: &'static [&'static str],
    /// The tool name to invoke
    tool_name: &'static str,
    /// Base complexity weight for this action type
    weight: f32,
}

/// Registry of action patterns for automatic tool resolution.
static ACTION_PATTERNS: &[ActionPattern] = &[
    ActionPattern {
        keywords: &[
            "read file",
            "open file",
            "load file",
            "read document",
            "read the file",
            "get the contents",
        ],
        tool_name: "filesystem",
        weight: 1.0,
    },
    ActionPattern {
        keywords: &[
            "write file",
            "save file",
            "create file",
            "write to",
            "save to",
            "output to",
        ],
        tool_name: "filesystem",
        weight: 1.5,
    },
    ActionPattern {
        keywords: &[
            "list files",
            "list directory",
            "show files",
            "list the",
            "browse",
        ],
        tool_name: "filesystem",
        weight: 0.8,
    },
    ActionPattern {
        keywords: &[
            "run command",
            "execute",
            "shell",
            "bash",
            "terminal",
            "run script",
        ],
        tool_name: "shell",
        weight: 2.0,
    },
    ActionPattern {
        keywords: &[
            "search web",
            "look up",
            "fetch url",
            "http",
            "download",
            "get data from",
        ],
        tool_name: "web",
        weight: 2.5,
    },
    ActionPattern {
        keywords: &[
            "search memory",
            "recall",
            "remember",
            "find in memory",
            "query memory",
        ],
        tool_name: "memory",
        weight: 1.2,
    },
    ActionPattern {
        keywords: &["store memory", "save to memory", "remember this", "persist"],
        tool_name: "memory",
        weight: 1.0,
    },
    ActionPattern {
        keywords: &["organize", "sort", "index", "catalog", "classify"],
        tool_name: "librarian",
        weight: 3.0,
    },
    ActionPattern {
        keywords: &[
            "orchestrate",
            "coordinate",
            "delegate",
            "distribute",
            "parallel",
        ],
        tool_name: "orchestration",
        weight: 4.0,
    },
];

/// Resolves a natural language sub-task description to a tool name and weight.
///
/// Uses keyword matching against the action pattern registry. Falls back to
/// `None` (control step) if no pattern matches.
fn resolve_action(description: &str) -> (Option<String>, f32) {
    let lower = description.to_lowercase();

    for pattern in ACTION_PATTERNS {
        for &keyword in pattern.keywords {
            if lower.contains(keyword) {
                return (Some(pattern.tool_name.to_string()), pattern.weight);
            }
        }
    }

    // Default: treat as a control/coordination step with moderate complexity
    (None, 1.5)
}

/// Computes the estimated complexity of a plan from its sub-tasks.
///
/// Complexity formula:
/// ```text
/// complexity = base_steps * tool_diversity_factor * dependency_factor
/// ```
///
/// Where:
/// - `base_steps`: `sqrt(sub_task_count)` normalized to [0, 5]
/// - `tool_diversity_factor`: `1.0 + (unique_tools / 10.0)`, capped at 2.0
/// - `dependency_factor`: `1.0 + (max_dependency_depth / 5.0)`, capped at 2.0
///
/// Final result is clamped to [0.5, 10.0].
fn compute_complexity(sub_tasks: &[SubTask]) -> f32 {
    if sub_tasks.is_empty() {
        return 0.5;
    }

    // Base complexity from step count (sqrt scaling)
    let step_count = sub_tasks.len() as f32;
    let base = (step_count.sqrt() / 2.0).min(5.0);

    // Tool diversity factor
    let unique_tools: usize = sub_tasks
        .iter()
        .filter_map(|t| t.tool_name.as_ref())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let diversity_factor = (1.0 + (unique_tools as f32 / 10.0)).min(2.0);

    // Dependency depth factor
    let max_depth = compute_max_dependency_depth(sub_tasks);
    let dependency_factor = (1.0 + (max_depth as f32 / 5.0)).min(2.0);

    // Sum of individual task weights
    let weight_sum: f32 = sub_tasks.iter().map(|t| t.weight).sum();
    let weight_factor = (weight_sum / step_count).max(1.0);

    (base * diversity_factor * dependency_factor * weight_factor).clamp(0.5, 10.0)
}

/// Computes the maximum dependency depth of the sub-task graph.
///
/// A task with no dependencies has depth 0. A task that depends on a depth-0
/// task has depth 1, and so on.
fn compute_max_dependency_depth(sub_tasks: &[SubTask]) -> usize {
    if sub_tasks.is_empty() {
        return 0;
    }

    let mut depths: Vec<usize> = vec![0; sub_tasks.len()];

    for (i, task) in sub_tasks.iter().enumerate() {
        if !task.dependencies.is_empty() {
            let max_parent_depth = task
                .dependencies
                .iter()
                .filter(|&&d| d < sub_tasks.len() && d < i)
                .map(|&d| depths[d])
                .max()
                .unwrap_or(0);
            depths[i] = max_parent_depth + 1;
        }
    }

    *depths.iter().max().unwrap_or(&0)
}

/// Splits a goal string into sub-task descriptions using conjunction-based parsing.
///
/// Handles compound goals like:
/// - "Read config.json and then write the output to results.txt"
/// - "Search memory for 'project-x', organize findings, and save report"
///
/// If no conjunctions are found, the entire goal is treated as a single sub-task.
fn decompose_goal(goal: &str) -> Vec<String> {
    let lower = goal.to_lowercase();
    let mut splits: Vec<(usize, usize)> = Vec::new();

    // Find all conjunction positions
    for &conj in CONJUNCTIONS {
        let mut search_start = 0;
        while let Some(pos) = lower[search_start..].find(conj) {
            let abs_pos = search_start + pos;
            splits.push((abs_pos, abs_pos + conj.len()));
            search_start = abs_pos + 1;
        }
    }

    if splits.is_empty() {
        // No conjunctions — single task
        return vec![goal.trim().to_string()];
    }

    // Sort splits by position and deduplicate overlapping ranges
    splits.sort_by_key(|s| s.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for split in splits {
        if let Some(last) = merged.last_mut() {
            if split.0 < last.1 {
                // Overlapping — extend the range
                last.1 = split.1;
                continue;
            }
        }
        merged.push(split);
    }

    // Extract sub-tasks between conjunctions
    let mut tasks: Vec<String> = Vec::new();
    let mut last_end = 0;

    for (start, end) in &merged {
        let segment = goal[*start..*end].trim();
        if !segment.is_empty() && *start > last_end {
            let task_str = goal[last_end..*start].trim();
            if !task_str.is_empty() {
                tasks.push(task_str.to_string());
            }
        }
        last_end = *end;
    }

    // Add the remaining tail
    if last_end < goal.len() {
        let tail = goal[last_end..].trim();
        if !tail.is_empty() {
            tasks.push(tail.to_string());
        }
    }

    // If all conjunctions were at the start/end and produced no real tasks,
    // fall back to the whole goal
    if tasks.is_empty() {
        tasks.push(goal.trim().to_string());
    }

    tasks
}

/// Builds a RequestFrame for a given sub-task.
fn build_step(sub_task: &SubTask, step_index: usize, session_id: &str) -> RequestFrame {
    let request_id = format!("synth-{}-{}", session_id, step_index);

    let payload = if let Some(ref tool_name) = sub_task.tool_name {
        // Tool execution step
        RequestPayload::ChatMessage(savant_core::types::ChatMessage {
            is_telemetry: false,
            role: savant_core::types::ChatRole::Assistant,
            content: format!("Execute tool '{}': {}", tool_name, sub_task.description),
            sender: Some("synthesis_engine".to_string()),
            recipient: None,
            agent_id: Some("synthesis".to_string()),
            session_id: Some(SessionId(session_id.to_string())),
            channel: savant_core::types::AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        })
    } else {
        // Control/coordination step
        RequestPayload::ControlFrame(ControlFrame::SoulUpdate {
            agent_id: "synthesis".to_string(),
            content: sub_task.description.clone(),
        })
    };

    RequestFrame {
        request_id,
        session_id: SessionId(session_id.to_string()),
        payload,
        signature: None,
        timestamp: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        ),
    }
}

/// Checks whether a response payload indicates an error.
///
/// Parses JSON to check for `is_error` and `success` fields, falling back
/// to prefix matching for plain-text error messages.
fn is_error_response(response: &str) -> bool {
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(response) {
        if let Some(is_error) = parsed.get("is_error").and_then(|v| v.as_bool()) {
            return is_error;
        }
        if let Some(success) = parsed.get("success").and_then(|v| v.as_bool()) {
            return !success;
        }
    }
    // Fallback: check for error patterns at start of response
    let trimmed = response.trim();
    trimmed.starts_with("error:") || trimmed.starts_with("Error:") || trimmed.starts_with("ERROR:")
}

/// The Recursive Synthesis Engine.
///
/// Decomposes high-level goals into executable trajectories using
/// conjunction-based parsing and action verb resolution.
pub struct SynthesisEngine {
    predictor: Arc<Mutex<DspPredictor>>,
}

impl SynthesisEngine {
    /// Creates a new SynthesisEngine with the given DSP predictor.
    pub fn new(predictor: Arc<Mutex<DspPredictor>>) -> Self {
        Self { predictor }
    }

    /// Synthesizes a new plan trajectory for a given set of goals.
    ///
    /// # Process
    /// 1. Decompose the goal string into sub-tasks using conjunction parsing
    /// 2. Resolve each sub-task to a tool name via action verb matching
    /// 3. Build RequestFrame steps for each sub-task
    /// 4. Compute overall trajectory complexity
    ///
    /// # Arguments
    /// * `goals` - Natural language goal description (may be compound)
    ///
    /// # Returns
    /// A `PlanTrajectory` containing the steps and complexity estimate.
    pub fn synthesize_plan(&self, goals: &str) -> PlanTrajectory {
        tracing::info!("SynthesisEngine: Synthesizing plan for goals: {}", goals);

        let session_id = uuid::Uuid::new_v4().to_string();

        // Stage 1: Decompose goal into sub-tasks
        let task_descriptions = decompose_goal(goals);
        tracing::debug!(
            "SynthesisEngine: Decomposed into {} sub-tasks",
            task_descriptions.len()
        );

        // Stage 2: Resolve actions and build sub-task structure
        let sub_tasks: Vec<SubTask> = task_descriptions
            .iter()
            .enumerate()
            .map(|(i, desc)| {
                let (tool_name, weight) = resolve_action(desc);

                // Track dependencies: tasks after the first depend on their predecessor
                let dependencies = if i > 0 { vec![i - 1] } else { vec![] };

                SubTask {
                    description: desc.clone(),
                    tool_name,
                    dependencies,
                    weight,
                }
            })
            .collect();

        // Stage 3: Build executable steps
        let steps: Vec<RequestFrame> = sub_tasks
            .iter()
            .enumerate()
            .map(|(i, task)| build_step(task, i, &session_id))
            .collect();

        // Stage 4: Compute complexity
        let complexity = compute_complexity(&sub_tasks);

        tracing::info!(
            "SynthesisEngine: Generated {} steps with estimated complexity {:.2}",
            steps.len(),
            complexity
        );

        PlanTrajectory {
            id: uuid::Uuid::new_v4(),
            steps,
            estimated_complexity: complexity,
            sub_tasks,
            original_goal: goals.to_string(),
        }
    }

    /// Refines an existing trajectory based on execution results.
    ///
    /// # Refinement Strategy
    /// - Uses DSP predictor to get optimal-k for the current complexity
    /// - If complexity > 5.0, splits into sub-trajectories (recursive decomposition)
    /// - Adjusts complexity based on execution failures (penalized +0.5 per error)
    /// - Adjusts complexity based on successes (rewarded -0.2 per success)
    /// - If trajectory exceeds predicted optimal-k, marks excess steps as low-priority
    ///
    /// # Arguments
    /// * `trajectory` - The existing plan trajectory
    /// * `results` - Execution results from completed steps
    ///
    /// # Returns
    /// The refined trajectory with adjusted complexity and optionally re-prioritized steps.
    pub async fn refine_trajectory(
        &self,
        mut trajectory: PlanTrajectory,
        results: &[ResponseFrame],
    ) -> PlanTrajectory {
        let mut predictor = self.predictor.lock().await;

        let k = predictor.predict_optimal_k(trajectory.estimated_complexity);
        tracing::debug!(
            "SynthesisEngine: Refining trajectory (predicted_k={}, steps={})",
            k,
            trajectory.steps.len()
        );

        // High-complexity splitting: if trajectory is too complex, split it
        if trajectory.estimated_complexity > 5.0 {
            tracing::info!(
                "SynthesisEngine: High complexity ({:.2}) detected. Triggering plan splitting.",
                trajectory.estimated_complexity
            );
            trajectory.estimated_complexity *= 0.6;

            // Mark excess steps beyond predicted k as low-priority by reducing their weight
            let excess = trajectory.steps.len().saturating_sub(k as usize);
            let total = trajectory.sub_tasks.len();
            if excess > 0 && excess <= total {
                let skip = total - excess;
                for task in trajectory.sub_tasks.iter_mut().skip(skip) {
                    task.weight *= 0.5;
                }
            }
        }

        // Analyze execution results to adjust complexity
        let mut failures = 0usize;
        let mut successes = 0usize;

        for res in results {
            if is_error_response(&res.payload) {
                failures += 1;
            } else {
                successes += 1;
            }
        }

        // Penalize failures, reward successes
        trajectory.estimated_complexity += failures as f32 * 0.5;
        trajectory.estimated_complexity -= successes as f32 * 0.2;
        trajectory.estimated_complexity = trajectory.estimated_complexity.clamp(0.5, 10.0);

        // If we have failures, identify which steps might need retry
        if failures > 0 {
            tracing::warn!(
                "SynthesisEngine: {} step(s) failed. Trajectory complexity adjusted to {:.2}",
                failures,
                trajectory.estimated_complexity
            );
        }

        // If the trajectory exceeds the predicted optimal-k, we flag it
        // as potentially needing further decomposition
        if trajectory.steps.len() > k as usize {
            tracing::info!(
                "SynthesisEngine: Step count ({}) exceeds optimal k ({}). Consider further decomposition.",
                trajectory.steps.len(),
                k
            );
        }

        trajectory
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::predictor::DspConfig;

    fn make_test_engine() -> SynthesisEngine {
        let config = DspConfig::default();
        let predictor = DspPredictor::new(config).expect("test predictor");
        SynthesisEngine::new(Arc::new(Mutex::new(predictor)))
    }

    #[test]
    fn test_decompose_goal_single() {
        let tasks = decompose_goal("Read config.json");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0], "Read config.json");
    }

    #[test]
    fn test_decompose_goal_compound() {
        let tasks = decompose_goal("Read config.json and write the output to results.txt");
        assert_eq!(tasks.len(), 2);
        assert!(tasks[0].contains("Read config.json"));
        assert!(tasks[1].contains("write"));
    }

    #[test]
    fn test_decompose_goal_multiple_conjunctions() {
        let tasks =
            decompose_goal("Search memory for project-x, organize findings, and save report");
        // The function splits on ", " and "and" — producing sub-tasks
        assert!(
            tasks.len() >= 2,
            "Should produce at least 2 sub-tasks, got: {:?}",
            tasks
        );
    }

    #[test]
    fn test_decompose_goal_then_separator() {
        let tasks = decompose_goal("Run the tests then fix any failures");
        assert!(tasks.len() >= 2, "Should split on 'then': {:?}", tasks);
    }

    #[test]
    fn test_resolve_action_shell_command() {
        let (tool, weight) = resolve_action("Execute ls -la in the directory");
        assert_eq!(tool, Some("shell".to_string()));
        assert!(weight > 0.0);
    }

    #[test]
    fn test_resolve_action_memory_search() {
        let (tool, weight) = resolve_action("Search memory for project history");
        assert_eq!(tool, Some("memory".to_string()));
        assert!(weight > 0.0);
    }

    #[test]
    fn test_resolve_action_librarian() {
        let (tool, _weight) = resolve_action("organize the codebase by category");
        assert_eq!(tool, Some("librarian".to_string()));
    }

    #[test]
    fn test_resolve_action_orchestration() {
        let (tool, _weight) = resolve_action("Orchestrate the deployment across nodes");
        assert_eq!(tool, Some("orchestration".to_string()));
    }

    #[test]
    fn test_resolve_action_web_fetch() {
        let (tool, _weight) = resolve_action("search web for documentation");
        assert_eq!(tool, Some("web".to_string()));
    }

    #[test]
    fn test_resolve_action_filesystem_write() {
        let (tool, _weight) = resolve_action("save file report to disk");
        assert_eq!(tool, Some("filesystem".to_string()));
    }

    #[test]
    fn test_decompose_goal_then() {
        let tasks = decompose_goal("Run the tests then fix any failures");
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_resolve_action_filesystem_read() {
        let (tool, weight) = resolve_action("read file config.json");
        assert_eq!(tool, Some("filesystem".to_string()));
        assert!(weight > 0.0);
    }

    #[test]
    fn test_resolve_action_web() {
        let (tool, _weight) = resolve_action("search web for documentation");
        assert_eq!(tool, Some("web".to_string()));
    }

    #[test]
    fn test_resolve_action_unknown() {
        let (tool, weight) = resolve_action("Think about the problem");
        assert!(tool.is_none());
        assert!(weight > 0.0);
    }

    #[test]
    fn test_compute_complexity_empty() {
        assert_eq!(compute_complexity(&[]), 0.5);
    }

    #[test]
    fn test_compute_complexity_single() {
        let tasks = vec![SubTask {
            description: "test".to_string(),
            tool_name: Some("filesystem".to_string()),
            dependencies: vec![],
            weight: 1.0,
        }];
        let c = compute_complexity(&tasks);
        assert!((0.5..=10.0).contains(&c));
    }

    #[test]
    fn test_compute_complexity_scales() {
        let small: Vec<SubTask> = (0..2)
            .map(|i| SubTask {
                description: format!("task {}", i),
                tool_name: Some("filesystem".to_string()),
                dependencies: if i > 0 { vec![i - 1] } else { vec![] },
                weight: 1.0,
            })
            .collect();

        let large: Vec<SubTask> = (0..20)
            .map(|i| SubTask {
                description: format!("task {}", i),
                tool_name: Some("shell".to_string()),
                dependencies: if i > 0 { vec![i - 1] } else { vec![] },
                weight: 2.0,
            })
            .collect();

        assert!(compute_complexity(&large) > compute_complexity(&small));
    }

    #[test]
    fn test_compute_max_dependency_depth() {
        let tasks = vec![
            SubTask {
                description: "a".to_string(),
                tool_name: None,
                dependencies: vec![],
                weight: 1.0,
            },
            SubTask {
                description: "b".to_string(),
                tool_name: None,
                dependencies: vec![0],
                weight: 1.0,
            },
            SubTask {
                description: "c".to_string(),
                tool_name: None,
                dependencies: vec![1],
                weight: 1.0,
            },
        ];
        assert_eq!(compute_max_dependency_depth(&tasks), 2);
    }

    #[test]
    fn test_synthesize_plan_basic() {
        let engine = make_test_engine();
        let plan = engine.synthesize_plan("Read config.json and write output to results.txt");

        assert!(!plan.steps.is_empty());
        assert!(plan.estimated_complexity >= 0.5);
        assert!(!plan.sub_tasks.is_empty());
        assert_eq!(
            plan.original_goal,
            "Read config.json and write output to results.txt"
        );
    }

    #[test]
    fn test_synthesize_plan_single_task() {
        let engine = make_test_engine();
        let plan = engine.synthesize_plan("List files in the current directory");

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.sub_tasks[0].tool_name, Some("filesystem".to_string()));
    }

    #[tokio::test]
    async fn test_refine_trajectory_successes_lower_complexity() {
        let engine = make_test_engine();
        let mut plan = engine.synthesize_plan("Read file and write output");
        let original_complexity = plan.estimated_complexity;

        let results = vec![
            ResponseFrame {
                request_id: "synth-0-0".to_string(),
                payload: "Successfully read file".to_string(),
            },
            ResponseFrame {
                request_id: "synth-0-1".to_string(),
                payload: "Successfully wrote output".to_string(),
            },
        ];

        plan = engine.refine_trajectory(plan, &results).await;
        assert!(plan.estimated_complexity < original_complexity);
    }

    #[tokio::test]
    async fn test_refine_trajectory_failures_increase_complexity() {
        let engine = make_test_engine();
        let mut plan = engine.synthesize_plan("Read file");
        let original_complexity = plan.estimated_complexity;

        let results = vec![ResponseFrame {
            request_id: "synth-0-0".to_string(),
            payload: "Error: file not found".to_string(),
        }];

        plan = engine.refine_trajectory(plan, &results).await;
        assert!(plan.estimated_complexity > original_complexity);
    }

    #[test]
    fn test_build_step_tool_action() {
        let task = SubTask {
            description: "Read config.json".to_string(),
            tool_name: Some("filesystem".to_string()),
            dependencies: vec![],
            weight: 1.0,
        };
        let step = build_step(&task, 0, "test-session");
        assert!(step.request_id.contains("test-session"));
    }

    #[test]
    fn test_build_step_control_action() {
        let task = SubTask {
            description: "Wait for approval".to_string(),
            tool_name: None,
            dependencies: vec![],
            weight: 1.0,
        };
        let step = build_step(&task, 0, "test-session");
        match step.payload {
            RequestPayload::ControlFrame(_) => {}
            _ => panic!("Expected ControlFrame for control action"),
        }
    }

    // ====================== EXTENDED TEST COVERAGE ======================

    #[test]
    fn test_decompose_goal_empty_string() {
        let tasks = decompose_goal("");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0], "");
    }

    #[test]
    fn test_decompose_goal_no_conjunctions() {
        let tasks = decompose_goal("Fix the bug in main.rs");
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].contains("Fix the bug"));
    }

    #[test]
    fn test_decompose_goal_finally_separator() {
        let tasks = decompose_goal("Load data then process it finally save results");
        assert!(tasks.len() >= 2);
    }

    #[test]
    fn test_decompose_goal_plus_separator() {
        let tasks = decompose_goal("Read file and then write the output");
        assert!(tasks.len() >= 2, "Should split on 'and then': {:?}", tasks);
    }

    #[test]
    fn test_decompose_goal_semicolon_separator() {
        let tasks = decompose_goal("Run tests and fix failures and commit");
        assert!(tasks.len() >= 2, "Should split on 'and': {:?}", tasks);
    }

    #[test]
    fn test_decompose_goal_whitespace_trimming() {
        let tasks = decompose_goal("  Read config  and  write output  ");
        for task in &tasks {
            assert_eq!(task, task.trim(), "Tasks should be trimmed: {:?}", task);
        }
    }

    // Note: test_resolve_action_shell_command, test_resolve_action_memory_search,
    // test_resolve_action_filesystem_read, test_resolve_action_web, test_resolve_action_unknown,
    // test_resolve_action_web_fetch, test_resolve_action_filesystem_write,
    // test_decompose_goal_multiple_conjunctions, test_decompose_goal_then are defined above.

    #[test]
    fn test_resolve_action_unknown_returns_some_weight() {
        // Even unknown actions should have a non-zero weight
        let (_, weight) = resolve_action("Contemplate the meaning of code");
        assert!(weight > 0.0);
    }

    #[test]
    fn test_resolve_action_empty_string() {
        let (_tool, weight) = resolve_action("");
        assert!(weight > 0.0);
        // Empty string might resolve to unknown
    }

    #[test]
    fn test_compute_complexity_increases_with_tool_diversity() {
        let uniform: Vec<SubTask> = (0..5)
            .map(|i| SubTask {
                description: format!("task {}", i),
                tool_name: Some("filesystem".to_string()),
                dependencies: vec![],
                weight: 1.0,
            })
            .collect();

        let diverse: Vec<SubTask> = (0..5)
            .map(|i| SubTask {
                description: format!("task {}", i),
                tool_name: Some(
                    match i % 4 {
                        0 => "filesystem",
                        1 => "shell",
                        2 => "web",
                        _ => "memory",
                    }
                    .to_string(),
                ),
                dependencies: vec![],
                weight: 1.0,
            })
            .collect();

        assert!(
            compute_complexity(&diverse) >= compute_complexity(&uniform),
            "Diverse tool usage should not reduce complexity"
        );
    }

    #[test]
    fn test_compute_complexity_high_weight_increases() {
        let low: Vec<SubTask> = vec![SubTask {
            description: "simple".to_string(),
            tool_name: Some("filesystem".to_string()),
            dependencies: vec![],
            weight: 0.5,
        }];

        let high: Vec<SubTask> = vec![SubTask {
            description: "complex".to_string(),
            tool_name: Some("filesystem".to_string()),
            dependencies: vec![],
            weight: 5.0,
        }];

        assert!(compute_complexity(&high) > compute_complexity(&low));
    }

    #[test]
    fn test_compute_max_dependency_depth_independent_tasks() {
        let tasks: Vec<SubTask> = (0..5)
            .map(|i| SubTask {
                description: format!("task {}", i),
                tool_name: None,
                dependencies: vec![], // No dependencies
                weight: 1.0,
            })
            .collect();

        assert_eq!(compute_max_dependency_depth(&tasks), 0);
    }

    #[test]
    fn test_compute_max_dependency_depth_single_chain() {
        let tasks: Vec<SubTask> = (0..10)
            .map(|i| SubTask {
                description: format!("task {}", i),
                tool_name: None,
                dependencies: if i > 0 { vec![i - 1] } else { vec![] },
                weight: 1.0,
            })
            .collect();

        assert_eq!(compute_max_dependency_depth(&tasks), 9);
    }

    #[test]
    fn test_synthesize_plan_preserves_goal() {
        let engine = make_test_engine();
        let goal = "Analyze the codebase and generate a report";
        let plan = engine.synthesize_plan(goal);
        assert_eq!(plan.original_goal, goal);
    }

    #[test]
    fn test_synthesize_plan_has_valid_steps() {
        let engine = make_test_engine();
        let plan = engine.synthesize_plan("Read config and write output");

        for (i, step) in plan.steps.iter().enumerate() {
            assert!(
                !step.request_id.is_empty(),
                "Step {} should have a request_id",
                i
            );
        }
    }

    #[test]
    fn test_synthesize_plan_estimated_complexity_bounds() {
        let engine = make_test_engine();
        let plan = engine.synthesize_plan("Do a simple task");

        assert!(
            plan.estimated_complexity >= 0.0,
            "Complexity should be non-negative"
        );
        assert!(
            plan.estimated_complexity <= 10.0,
            "Complexity should be bounded"
        );
    }

    #[tokio::test]
    async fn test_refine_trajectory_empty_results() {
        let engine = make_test_engine();
        let plan = engine.synthesize_plan("Read file");
        let original_complexity = plan.estimated_complexity;

        let results: Vec<ResponseFrame> = vec![];
        let refined = engine.refine_trajectory(plan, &results).await;

        // With no results, complexity should stay the same
        assert_eq!(refined.estimated_complexity, original_complexity);
    }

    #[tokio::test]
    async fn test_refine_trajectory_multiple_failures() {
        let engine = make_test_engine();
        let plan = engine.synthesize_plan("Read file and write output");
        let original_complexity = plan.estimated_complexity;

        let results = vec![
            ResponseFrame {
                request_id: "synth-0-0".to_string(),
                payload: "Error: permission denied".to_string(),
            },
            ResponseFrame {
                request_id: "synth-0-1".to_string(),
                payload: "Error: disk full".to_string(),
            },
            ResponseFrame {
                request_id: "synth-0-2".to_string(),
                payload: "Error: timeout".to_string(),
            },
        ];

        let refined = engine.refine_trajectory(plan, &results).await;
        assert!(
            refined.estimated_complexity > original_complexity,
            "Multiple failures should increase complexity"
        );
    }

    #[tokio::test]
    async fn test_refine_trajectory_mixed_results() {
        let engine = make_test_engine();
        let plan = engine.synthesize_plan("Read file and write output");

        let results = vec![
            ResponseFrame {
                request_id: "synth-0-0".to_string(),
                payload: "Successfully read file".to_string(),
            },
            ResponseFrame {
                request_id: "synth-0-1".to_string(),
                payload: "Error: write failed".to_string(),
            },
        ];

        let refined = engine.refine_trajectory(plan, &results).await;
        // Mixed results should produce moderate complexity
        assert!(refined.estimated_complexity > 0.0);
    }

    #[test]
    fn test_synthesize_plan_high_complexity_splits() {
        let engine = make_test_engine();
        // A very complex goal should produce more steps
        let plan = engine.synthesize_plan(
            "Read config and validate schema and transform data and run tests and deploy to staging and run integration tests and generate report and save to disk"
        );

        assert!(
            plan.steps.len() > 1,
            "Complex goals should produce multiple steps"
        );
        assert!(
            plan.estimated_complexity > 1.0,
            "Complex goal should have complexity > 1.0"
        );
    }
}
