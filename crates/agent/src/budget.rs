/// Token budget manager for context window management.
///
/// Allocates tokens across priority tiers:
/// - System prompt: 20%
/// - Recent messages: 50%
/// - Semantic memories: 20%
/// - Old transcripts: 10%
#[derive(Debug, Clone)]
pub struct TokenBudget {
    pub limit: usize,
    pub used: usize,
}

impl TokenBudget {
    /// Creates a new TokenBudget constraint.
    pub fn new(limit: usize) -> Self {
        Self { limit, used: 0 }
    }

    /// Creates a budget with default LLM limit (8192 tokens).
    pub fn default_limit() -> Self {
        Self::new(8192)
    }

    /// Deducts a number of tokens, returning true if budget limit is reached.
    pub fn deduct(&mut self, amount: usize) -> bool {
        self.used += amount;
        self.used >= self.limit
    }

    /// Returns remaining tokens.
    pub fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.used)
    }

    /// Returns usage percentage (0-100).
    pub fn usage_percent(&self) -> usize {
        if self.limit == 0 {
            return 100;
        }
        (self.used * 100) / self.limit
    }

    /// Evaluates if context summarization should occur to preserve budget.
    /// Triggered at 80% usage.
    #[must_use]
    pub fn should_summarize(&self) -> bool {
        self.used > (self.limit * 80) / 100
    }

    /// Returns the allocation for each tier.
    pub fn allocations(&self) -> BudgetAllocations {
        BudgetAllocations {
            system_prompt: (self.limit * 20) / 100,
            recent_messages: (self.limit * 50) / 100,
            semantic_memories: (self.limit * 20) / 100,
            old_transcripts: (self.limit * 10) / 100,
        }
    }

    /// Estimates token count from text (rough: 4 chars ≈ 1 token).
    pub fn estimate_tokens(text: &str) -> usize {
        text.len().div_ceil(4)
    }

    /// Resets the budget to zero usage.
    pub fn reset(&mut self) {
        self.used = 0;
    }
}

/// Token allocation per tier.
#[derive(Debug, Clone)]
pub struct BudgetAllocations {
    pub system_prompt: usize,
    pub recent_messages: usize,
    pub semantic_memories: usize,
    pub old_transcripts: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_creation() {
        let budget = TokenBudget::new(1000);
        assert_eq!(budget.limit, 1000);
        assert_eq!(budget.used, 0);
    }

    #[test]
    fn test_budget_deduct() {
        let mut budget = TokenBudget::new(1000);
        assert!(!budget.deduct(500));
        assert_eq!(budget.used, 500);
        assert!(budget.deduct(500));
        assert_eq!(budget.used, 1000);
    }

    #[test]
    fn test_budget_remaining() {
        let mut budget = TokenBudget::new(1000);
        budget.deduct(300);
        assert_eq!(budget.remaining(), 700);
    }

    #[test]
    fn test_budget_usage_percent() {
        let mut budget = TokenBudget::new(1000);
        budget.deduct(250);
        assert_eq!(budget.usage_percent(), 25);
    }

    #[test]
    fn test_should_summarize() {
        let mut budget = TokenBudget::new(1000);
        budget.deduct(790);
        assert!(!budget.should_summarize());
        budget.deduct(20);
        assert!(budget.should_summarize());
    }

    #[test]
    fn test_allocations() {
        let budget = TokenBudget::new(10000);
        let alloc = budget.allocations();
        assert_eq!(alloc.system_prompt, 2000);
        assert_eq!(alloc.recent_messages, 5000);
        assert_eq!(alloc.semantic_memories, 2000);
        assert_eq!(alloc.old_transcripts, 1000);
    }

    #[test]
    fn test_estimate_tokens() {
        let text = "Hello world this is a test";
        let tokens = TokenBudget::estimate_tokens(text);
        assert!(tokens > 0);
        assert!(tokens < text.len());
    }

    #[test]
    fn test_reset() {
        let mut budget = TokenBudget::new(1000);
        budget.deduct(500);
        budget.reset();
        assert_eq!(budget.used, 0);
        assert_eq!(budget.remaining(), 1000);
    }
}
