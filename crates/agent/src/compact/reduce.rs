//! Reduction pipeline — text transformation and compaction.

use crate::compact::schema::*;
use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Instant;

/// Pipeline for applying rule-based text transformations.
pub struct ReductionPipeline;

impl ReductionPipeline {
    /// Applies a compiled rule to tool output and returns the compaction result.
    pub fn apply(rule: &CompiledRule, output: &ToolOutput) -> CompactionResult {
        let start = Instant::now();
        let original_bytes = output.raw_output.len();

        // Step 1: Passthrough check — output below threshold
        if original_bytes <= 240 {
            return CompactionResult::passthrough(&output.raw_output);
        }

        // Step 2: Apply transforms
        let mut text = Self::apply_transforms(&output.raw_output, &rule.rule.transforms);

        // Step 3: Apply filters
        text = Self::apply_filters(text, rule);

        // Step 4: Apply summarization (head/tail)
        let (text, was_truncated) = Self::apply_summarize(text, &rule.rule.summarize);

        // Step 5: Apply counters
        let counters = Self::apply_counters(&text, &rule.counter_regexes);

        // Step 6: Apply failure mode if exit code != 0
        let text = if output.exit_code != 0 {
            Self::apply_failure_mode(text, &rule.rule.failure_mode)
        } else {
            text
        };

        let compressed_bytes = text.len();
        let ratio = if original_bytes > 0 {
            compressed_bytes as f32 / original_bytes as f32
        } else {
            1.0
        };

        let processing_us = start.elapsed().as_micros() as u64;

        CompactionResult {
            output: text,
            rule_id: rule.rule.id.clone(),
            original_bytes,
            compressed_bytes,
            ratio,
            counters,
            was_truncated,
            processing_us,
        }
    }

    /// Applies text transforms (ANSI stripping, normalization, dedup).
    fn apply_transforms<'a>(input: &'a str, transforms: &Transforms) -> Cow<'a, str> {
        let mut text = Cow::Borrowed(input);

        if transforms.strip_ansi {
            text = Cow::Owned(Self::strip_ansi(&text));
        }

        if transforms.trim_empty_edges {
            text = Cow::Owned(text.trim().to_string());
        }

        if transforms.normalize_whitespace {
            text = Cow::Owned(Self::normalize_whitespace(&text));
        }

        if transforms.dedupe_adjacent_lines {
            text = Cow::Owned(Self::dedupe_adjacent(&text));
        }

        if transforms.extract_json {
            text = Cow::Owned(Self::minify_json(&text));
        }

        text
    }

    /// Applies line-level filters (skip/keep patterns).
    fn apply_filters<'a>(input: Cow<'a, str>, rule: &CompiledRule) -> Cow<'a, str> {
        let has_keep = !rule.keep_regexes.is_empty();
        let has_skip = !rule.skip_regexes.is_empty();

        if !has_keep && !has_skip {
            return input;
        }

        let lines: Vec<&str> = input.lines().collect();
        let mut filtered = Vec::with_capacity(lines.len());

        for line in &lines {
            // If keep patterns exist, only keep matching lines
            if has_keep {
                let matches = rule.keep_regexes.iter().any(|re| re.is_match(line));
                if matches {
                    filtered.push(*line);
                }
                continue;
            }

            // Skip patterns: drop matching lines
            if has_skip {
                let should_skip = rule.skip_regexes.iter().any(|re| re.is_match(line));
                if !should_skip {
                    filtered.push(*line);
                }
            }
        }

        Cow::Owned(filtered.join("\n"))
    }

    /// Applies head/tail summarization.
    fn apply_summarize(input: Cow<'_, str>, strategy: &SummarizeStrategy) -> (String, bool) {
        let lines: Vec<&str> = input.lines().collect();
        let total_lines = lines.len();

        if total_lines <= strategy.head_lines + strategy.tail_lines {
            let text = lines.join("\n");
            if text.len() <= strategy.max_chars {
                return (text, false);
            }
        }

        // Head + tail truncation
        let head = lines
            .iter()
            .take(strategy.head_lines)
            .copied()
            .collect::<Vec<_>>();
        let tail = lines
            .iter()
            .skip(total_lines.saturating_sub(strategy.tail_lines))
            .copied()
            .collect::<Vec<_>>();

        let omitted = total_lines - head.len() - tail.len();
        let result = format!(
            "{}\n\n[... {} lines omitted ...]\n\n{}",
            head.join("\n"),
            omitted,
            tail.join("\n")
        );

        // Final char clamp
        if result.len() > strategy.max_chars {
            let head_chars = (strategy.max_chars * 60) / 100;
            let tail_chars = (strategy.max_chars * 40) / 100;
            let mut head_end = head_chars.min(result.len());
            while head_end > 0 && !result.is_char_boundary(head_end) {
                head_end -= 1;
            }
            let mut tail_start = result.len().saturating_sub(tail_chars);
            while tail_start < result.len() && !result.is_char_boundary(tail_start) {
                tail_start += 1;
            }
            (
                format!(
                    "{}\n\n[... truncated ...]\n\n{}",
                    &result[..head_end],
                    &result[tail_start..]
                ),
                true,
            )
        } else {
            (result, true)
        }
    }

    /// Applies named regex counters to extract metrics from output.
    fn apply_counters(
        text: &str,
        counter_regexes: &[(String, regex::Regex)],
    ) -> HashMap<String, usize> {
        let mut counters = HashMap::new();
        for (name, regex) in counter_regexes {
            let count = regex.find_iter(text).count();
            if count > 0 {
                counters.insert(name.clone(), count);
            }
        }
        counters
    }

    /// Applies failure mode transformation.
    fn apply_failure_mode(input: String, mode: &FailureMode) -> String {
        match mode {
            FailureMode::PreserveRaw => input,
            FailureMode::AggressiveTruncate {
                head_lines,
                tail_lines,
            } => {
                let lines: Vec<&str> = input.lines().collect();
                let total = lines.len();
                if total <= head_lines + tail_lines {
                    return input;
                }
                let head = lines.iter().take(*head_lines).copied().collect::<Vec<_>>();
                let tail = lines
                    .iter()
                    .skip(total.saturating_sub(*tail_lines))
                    .copied()
                    .collect::<Vec<_>>();
                let omitted = total - head.len() - tail.len();
                format!(
                    "{}\n\n[... {} lines omitted on failure ...]\n\n{}",
                    head.join("\n"),
                    omitted,
                    tail.join("\n")
                )
            }
            FailureMode::EmitErrorMarker => {
                format!("[COMPACT: Error output truncated]\n{}\n[END]", input)
            }
        }
    }

    // ── Text utilities ──

    /// Strips ANSI escape sequences.
    fn strip_ansi(input: &str) -> String {
        // Simple ANSI escape sequence removal
        let mut result = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Skip ESC sequences: ESC [ ... m  or  ESC ] ... BEL
                if chars.peek() == Some(&'[') {
                    chars.next(); // consume '['
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch.is_ascii_alphabetic() {
                            break;
                        }
                    }
                } else if chars.peek() == Some(&']') {
                    chars.next(); // consume ']'
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch == '\x07' || (ch == '\\' && chars.peek().is_some()) {
                            break;
                        }
                    }
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    /// Normalizes whitespace (collapse multiple spaces, trim lines).
    fn normalize_whitespace(input: &str) -> String {
        input
            .lines()
            .map(|line| line.trim())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Deduplicates adjacent identical lines.
    fn dedupe_adjacent(input: &str) -> String {
        let lines: Vec<&str> = input.lines().collect();
        if lines.is_empty() {
            return String::new();
        }
        let mut result: Vec<String> = Vec::with_capacity(lines.len());
        let mut prev = lines[0];
        let mut count = 1usize;
        for &line in &lines[1..] {
            if line == prev {
                count += 1;
            } else {
                result.push(prev.to_string());
                if count > 1 {
                    result.push(format!("  [x{}]", count));
                }
                prev = line;
                count = 1;
            }
        }
        result.push(prev.to_string());
        if count > 1 {
            result.push(format!("  [x{}]", count));
        }
        result.join("\n")
    }

    /// Minifies JSON by removing unnecessary whitespace.
    fn minify_json(input: &str) -> String {
        // Simple JSON minification: remove whitespace outside strings
        let mut result = String::with_capacity(input.len());
        let mut in_string = false;
        let mut escaped = false;
        for c in input.chars() {
            if escaped {
                result.push(c);
                escaped = false;
                continue;
            }
            if c == '\\' && in_string {
                result.push(c);
                escaped = true;
                continue;
            }
            if c == '"' {
                in_string = !in_string;
                result.push(c);
                continue;
            }
            if !in_string && c.is_ascii_whitespace() {
                continue;
            }
            result.push(c);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi() {
        let input = "\x1b[32mhello\x1b[0m world";
        let result = ReductionPipeline::strip_ansi(input);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_dedupe_adjacent() {
        let input = "line1\nline1\nline1\nline2\nline2";
        let result = ReductionPipeline::dedupe_adjacent(input);
        assert!(result.contains("[x3]"));
        assert!(result.contains("[x2]"));
    }

    #[test]
    fn test_minify_json() {
        let input = r#"{ "key" : "value" , "num" : 42 }"#;
        let result = ReductionPipeline::minify_json(input);
        assert_eq!(result, r#"{"key":"value","num":42}"#);
    }

    #[test]
    fn test_passthrough_short_output() {
        let rule = crate::compact::schema::CompiledRule {
            rule: CompactRule::default(),
            skip_regexes: Vec::new(),
            keep_regexes: Vec::new(),
            heuristic_regexes: Vec::new(),
            counter_regexes: Vec::new(),
        };
        let output = ToolOutput {
            tool_name: "echo".to_string(),
            argv: vec!["echo".to_string(), "hello".to_string()],
            exit_code: 0,
            raw_output: "hello".to_string(),
            working_dir: None,
        };
        let result = ReductionPipeline::apply(&rule, &output);
        assert_eq!(result.rule_id, "passthrough");
        assert_eq!(result.ratio, 1.0);
    }

    #[test]
    fn test_apply_transforms_strip_ansi() {
        let transforms = Transforms {
            strip_ansi: true,
            ..Default::default()
        };
        let input = "\x1b[31merror\x1b[0m";
        let result = ReductionPipeline::apply_transforms(input, &transforms);
        assert_eq!(result, "error");
    }

    #[test]
    fn test_counters() {
        let regex = regex::Regex::new(r"test").unwrap();
        let counters = ReductionPipeline::apply_counters(
            "test line 1\ntest line 2\nother",
            &[("test_count".to_string(), regex)],
        );
        assert_eq!(counters.get("test_count"), Some(&2));
    }
}
