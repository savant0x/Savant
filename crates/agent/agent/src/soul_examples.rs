//! Soul Example Exchange Parser
//!
//! Parses `## Example Exchanges` section from SOUL.md to provide
//! few-shot examples for the LLM. Examples teach the model "what kind of
//! assistant to be" more effectively than descriptions alone.
//!
//! Format:
//! ```markdown
//! ## Example Exchanges
//!
//! ### Example 1: Debugging a crash
//! User: The agent keeps crashing on startup
//! Assistant: Let me check the logs and trace the issue.
//! ```
//!
//! If no `## Example Exchanges` section exists, returns empty (no error).

/// A parsed example exchange from SOUL.md.
#[derive(Debug, Clone)]
pub struct SoulExample {
    /// The user's message in the example.
    pub user_message: String,
    /// The assistant's response in the example.
    pub assistant_message: String,
}

/// Parse example exchanges from SOUL.md content.
///
/// Looks for `## Example Exchanges` section, then splits on
/// `### Example N:` headers. Within each example, splits on
/// `User:` and `Assistant:` prefixes.
pub fn parse_soul_examples(soul_content: &str) -> Vec<SoulExample> {
    // Find the "## Example Exchanges" section
    let section_start = match soul_content.find("## Example Exchanges") {
        Some(idx) => idx,
        None => return Vec::new(),
    };

    let section = &soul_content[section_start..];

    // Find the next ## header (end of this section)
    let section_end = section[20..]
        .find("\n## ")
        .map(|idx| idx + 20)
        .unwrap_or(section.len());
    let section = &section[..section_end];

    // Split on "### Example" headers
    let parts: Vec<&str> = section.split("### Example").collect();
    let mut examples = Vec::new();

    for part in parts.iter().skip(1) {
        // Skip the first part (header before first example)
        if let Some(example) = parse_single_example(part) {
            examples.push(example);
        }
    }

    examples
}

/// Parse a single example block into user/assistant messages.
fn parse_single_example(block: &str) -> Option<SoulExample> {
    let block = block.trim();

    // Find "User:" prefix
    let user_start = block.find("User:")?;
    let user_text = &block[user_start + 5..];

    // Find "Assistant:" prefix
    let assistant_start = user_text.find("Assistant:")?;
    let user_message = user_text[..assistant_start].trim().to_string();

    let assistant_text = &user_text[assistant_start + 10..];

    // Assistant message goes until the next "### Example" or end of block
    let assistant_end = assistant_text
        .find("### Example")
        .unwrap_or(assistant_text.len());
    let assistant_message = assistant_text[..assistant_end].trim().to_string();

    if user_message.is_empty() && assistant_message.is_empty() {
        return None;
    }

    Some(SoulExample {
        user_message,
        assistant_message,
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_examples() {
        let soul = r#"
# My Soul

I am a helpful assistant.

## Example Exchanges

### Example 1: Debugging
User: The app crashes on startup
Assistant: Let me check the logs.
I see a panic in the memory engine.

### Example 2: Uncertain
User: What's X?
Assistant: I'm not sure. Let me search.
"#;
        let examples = parse_soul_examples(soul);
        assert_eq!(examples.len(), 2);
        assert!(examples[0].user_message.contains("crashes on startup"));
        assert!(examples[0].assistant_message.contains("check the logs"));
        assert!(examples[1].user_message.contains("What's X?"));
    }

    #[test]
    fn test_no_examples_section() {
        let soul = "# My Soul\nI am helpful.\n";
        let examples = parse_soul_examples(soul);
        assert!(examples.is_empty());
    }

    #[test]
    fn test_empty_examples_section() {
        let soul = "# My Soul\n\n## Example Exchanges\n\n## Other Section\n";
        let examples = parse_soul_examples(soul);
        assert!(examples.is_empty());
    }
}
