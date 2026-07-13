//! Shell command parsing using tree-sitter-bash.

use tree_sitter::{Node, Parser};

/// Parsed shell command analysis.
#[derive(Debug, Clone)]
pub struct ShellAnalysis {
    /// Simple commands found in the pipeline.
    pub commands: Vec<ShellCommand>,
    /// Detected risk patterns.
    pub risks: Vec<RiskPattern>,
    /// Whether the command contains a pipeline.
    pub is_pipeline: bool,
    /// Whether the command uses subshells.
    pub has_subshell: bool,
}

/// A simple command extracted from a shell pipeline.
#[derive(Debug, Clone)]
pub struct ShellCommand {
    /// The command name (first word).
    pub name: String,
    /// Arguments (everything after the command name).
    pub args: Vec<String>,
    /// Full text of this command.
    pub text: String,
}

/// A detected risk pattern in a shell command.
#[derive(Debug, Clone)]
pub struct RiskPattern {
    /// The kind of risk.
    pub kind: RiskKind,
    /// Human-readable description of the risk.
    pub description: String,
}

/// Kinds of risk in shell commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskKind {
    /// Shell wrapper (e.g., `bash -c`, `sh -c`).
    ShellWrapper,
    /// Inline eval (e.g., `eval`, `exec`, `source`).
    InlineEval,
    /// Command substitution (e.g., `$(...)`, backticks).
    CommandSubstitution,
    /// Dynamic argument (variable in command position).
    DynamicArgument,
    /// Pipe chain (multi-command pipeline).
    PipeChain,
    /// Subshell (e.g., `(...)`).
    Subshell,
    /// Sudo command.
    Sudo,
    /// Shell redirection to file.
    FileRedirect,
}

/// Parse a shell command string and analyze it.
pub fn parse_command(cmd: &str) -> ShellAnalysis {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        return basic_analysis(cmd);
    }

    let source = cmd.as_bytes();
    let tree = match parser.parse(source, None) {
        Some(tree) => tree,
        None => return basic_analysis(cmd),
    };

    let root = tree.root_node();
    let mut commands = Vec::new();
    let mut risks = Vec::new();
    let mut is_pipeline = false;
    let mut has_subshell = false;

    analyze_node(
        root,
        source,
        &mut commands,
        &mut risks,
        &mut is_pipeline,
        &mut has_subshell,
    );

    if commands.is_empty() {
        return basic_analysis(cmd);
    }

    ShellAnalysis {
        commands,
        risks,
        is_pipeline,
        has_subshell,
    }
}

fn analyze_node(
    node: Node,
    source: &[u8],
    commands: &mut Vec<ShellCommand>,
    risks: &mut Vec<RiskPattern>,
    is_pipeline: &mut bool,
    has_subshell: &mut bool,
) {
    match node.kind() {
        "command" => {
            if let Some(cmd) = extract_command(node, source) {
                // Check for shell wrappers and dangerous commands
                let lower = cmd.name.to_lowercase();
                if (lower == "bash" || lower == "sh" || lower == "zsh")
                    && cmd.args.iter().any(|a| a == "-c")
                {
                    risks.push(RiskPattern {
                        kind: RiskKind::ShellWrapper,
                        description: "Shell wrapper detected — command is passed as a string to another shell.".to_string(),
                    });
                }
                if lower == "eval" || lower == "exec" || lower == "source" {
                    risks.push(RiskPattern {
                        kind: RiskKind::InlineEval,
                        description: "Inline eval/exec/source detected — executes dynamically constructed commands.".to_string(),
                    });
                }
                if lower == "sudo" {
                    risks.push(RiskPattern {
                        kind: RiskKind::Sudo,
                        description: "sudo detected — command runs with elevated privileges."
                            .to_string(),
                    });
                }
                commands.push(cmd);
            }
        }
        "pipeline" => {
            *is_pipeline = true;
            risks.push(RiskPattern {
                kind: RiskKind::PipeChain,
                description: "Command uses a pipeline — each command runs with the output of the previous one.".to_string(),
            });
        }
        "subshell" => {
            *has_subshell = true;
            risks.push(RiskPattern {
                kind: RiskKind::Subshell,
                description:
                    "Command runs in a subshell — changes inside don't affect the parent shell."
                        .to_string(),
            });
        }
        "command_substitution" => {
            risks.push(RiskPattern {
                kind: RiskKind::CommandSubstitution,
                description: "Command substitution detected — inner command output is used as part of the outer command.".to_string(),
            });
        }
        "redirect" => {
            risks.push(RiskPattern {
                kind: RiskKind::FileRedirect,
                description: "File redirection detected — output may be written to a file."
                    .to_string(),
            });
        }
        "variable_assignment" => {
            if let Ok(text) = node.utf8_text(source) {
                if text.contains('$') {
                    risks.push(RiskPattern {
                        kind: RiskKind::DynamicArgument,
                        description: format!(
                            "Variable '{}' used in assignment — value depends on runtime state.",
                            text
                        ),
                    });
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        analyze_node(child, source, commands, risks, is_pipeline, has_subshell);
    }
}

fn extract_command(node: Node, source: &[u8]) -> Option<ShellCommand> {
    let mut name = String::new();
    let mut args = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = child.utf8_text(source).ok()?.to_string();
        match child.kind() {
            "command_name" => name = text,
            "word" | "string" | "raw_string" => {
                if !name.is_empty() {
                    args.push(text);
                }
            }
            _ => {}
        }
    }

    if name.is_empty() {
        return None;
    }

    let text = node.utf8_text(source).unwrap_or("").to_string();
    Some(ShellCommand { name, args, text })
}

/// Basic fallback analysis without tree-sitter (splits on pipes and spaces).
fn basic_analysis(cmd: &str) -> ShellAnalysis {
    let mut commands = Vec::new();
    let mut risks = Vec::new();

    let parts: Vec<&str> = cmd.split('|').collect();
    let is_pipeline = parts.len() > 1;

    if is_pipeline {
        risks.push(RiskPattern {
            kind: RiskKind::PipeChain,
            description: "Command uses a pipeline.".to_string(),
        });
    }

    for part in &parts {
        let words: Vec<&str> = part.split_whitespace().collect();
        if let Some(&name) = words.first() {
            let args: Vec<String> = words[1..].iter().map(|s| s.to_string()).collect();
            let lower = name.to_lowercase();
            let has_c_flag = args.iter().any(|a| a == "-c");
            commands.push(ShellCommand {
                name: name.to_string(),
                args,
                text: part.trim().to_string(),
            });
            if (lower == "bash" || lower == "sh" || lower == "zsh") && has_c_flag {
                risks.push(RiskPattern {
                    kind: RiskKind::ShellWrapper,
                    description: "Shell wrapper detected.".to_string(),
                });
            }
            if lower == "eval" || lower == "exec" || lower == "source" {
                risks.push(RiskPattern {
                    kind: RiskKind::InlineEval,
                    description: "Inline eval/exec/source detected.".to_string(),
                });
            }
            if lower == "sudo" {
                risks.push(RiskPattern {
                    kind: RiskKind::Sudo,
                    description: "sudo detected.".to_string(),
                });
            }
        }
    }

    let has_subshell = cmd.contains("$(") || cmd.starts_with('(');

    ShellAnalysis {
        commands,
        risks,
        is_pipeline,
        has_subshell,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_command() {
        let analysis = parse_command("ls -la /tmp");
        assert_eq!(analysis.commands.len(), 1);
        assert_eq!(analysis.commands[0].name, "ls");
        assert_eq!(analysis.commands[0].args, vec!["-la", "/tmp"]);
        assert!(!analysis.is_pipeline);
    }

    #[test]
    fn test_pipeline() {
        let analysis = parse_command("cat file.txt | grep pattern | wc -l");
        assert!(analysis.is_pipeline);
        assert!(analysis.risks.iter().any(|r| r.kind == RiskKind::PipeChain));
    }

    #[test]
    fn test_shell_wrapper() {
        let analysis = parse_command(r#"bash -c "echo hello""#);
        assert!(analysis
            .risks
            .iter()
            .any(|r| r.kind == RiskKind::ShellWrapper));
    }

    #[test]
    fn test_sudo() {
        let analysis = parse_command("sudo rm -rf /tmp/test");
        assert!(analysis.risks.iter().any(|r| r.kind == RiskKind::Sudo));
    }

    #[test]
    fn test_eval() {
        let analysis = parse_command("eval $DANGEROUS_CMD");
        assert!(analysis
            .risks
            .iter()
            .any(|r| r.kind == RiskKind::InlineEval));
    }

    #[test]
    fn test_basic_fallback() {
        let analysis = basic_analysis("echo hello | grep h");
        assert!(analysis.is_pipeline);
        assert!(!analysis.commands.is_empty());
    }
}
