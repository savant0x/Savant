//! Cost-Aware Model Routing
//!
//! Routes tasks to cheap or expensive models based on prompt complexity.
//! Simple tasks (summarization, formatting) use cheap models.
//! Complex tasks (reasoning, code generation) use expensive models.
//!
//! Component 1 of FID-20260525-AGENT-INTELLIGENCE.

/// Task complexity classification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskComplexity {
    Simple,
    Moderate,
    Complex,
}

/// Routes tasks to appropriate models based on complexity.
pub struct CostAwareRouter {
    pub cheap_model: String,
    pub expensive_model: String,
}

impl CostAwareRouter {
    pub fn new(cheap: &str, expensive: &str) -> Self {
        Self {
            cheap_model: cheap.to_string(),
            expensive_model: expensive.to_string(),
        }
    }

    /// Classify task complexity from prompt text.
    pub fn classify(prompt: &str) -> TaskComplexity {
        let lower = prompt.to_lowercase();
        let word_count = prompt.split_whitespace().count();

        // Complex signals (highest priority)
        if lower.contains("```")
            || lower.contains("implement")
            || lower.contains("architect")
            || lower.contains("design a")
            || lower.contains("debug")
            || lower.contains("refactor")
            || word_count > 200
        {
            return TaskComplexity::Complex;
        }

        // Moderate signals
        if lower.contains("analyze")
            || lower.contains("compare")
            || lower.contains("explain")
            || lower.contains("why")
            || lower.contains("how does")
            || word_count > 50
        {
            return TaskComplexity::Moderate;
        }

        // Simple: short prompts, formatting, extraction
        TaskComplexity::Simple
    }

    /// Select model based on complexity.
    pub fn select_model(&self, complexity: TaskComplexity) -> &str {
        match complexity {
            TaskComplexity::Simple | TaskComplexity::Moderate => &self.cheap_model,
            TaskComplexity::Complex => &self.expensive_model,
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_task_classification() {
        assert_eq!(
            CostAwareRouter::classify("Summarize this"),
            TaskComplexity::Simple
        );
        assert_eq!(
            CostAwareRouter::classify("List files"),
            TaskComplexity::Simple
        );
    }

    #[test]
    fn test_moderate_task_classification() {
        assert_eq!(
            CostAwareRouter::classify("Analyze why the build fails"),
            TaskComplexity::Moderate
        );
        assert_eq!(
            CostAwareRouter::classify("Compare X and Y approaches"),
            TaskComplexity::Moderate
        );
    }

    #[test]
    fn test_complex_task_classification() {
        assert_eq!(
            CostAwareRouter::classify("Implement a new provider"),
            TaskComplexity::Complex
        );
        assert_eq!(
            CostAwareRouter::classify("```\ncode here\n```"),
            TaskComplexity::Complex
        );
        assert_eq!(
            CostAwareRouter::classify("Debug this crash"),
            TaskComplexity::Complex
        );
    }

    #[test]
    fn test_model_selection() {
        let router = CostAwareRouter::new("cheap-model", "expensive-model");
        assert_eq!(router.select_model(TaskComplexity::Simple), "cheap-model");
        assert_eq!(router.select_model(TaskComplexity::Moderate), "cheap-model");
        assert_eq!(
            router.select_model(TaskComplexity::Complex),
            "expensive-model"
        );
    }
}
