//! Shell command explanation and risk assessment.

use super::parser::{RiskKind, ShellAnalysis};

/// Risk level of a shell command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Safe,
    Warning,
    Danger,
}

/// Explanation of a shell command.
#[derive(Debug, Clone)]
pub struct CommandExplanation {
    /// The original command.
    pub command: String,
    /// Human-readable explanation.
    pub explanation: String,
    /// Overall risk level.
    pub risk_level: RiskLevel,
    /// Individual risks found.
    pub risks: Vec<String>,
    /// What each subcommand does.
    pub sub_explanations: Vec<String>,
}

/// Explain a shell command based on parsed analysis.
pub fn explain(cmd: &str, analysis: &ShellAnalysis) -> CommandExplanation {
    let mut sub_explanations = Vec::new();
    let mut risk_level = RiskLevel::Safe;
    let mut risks = Vec::new();

    // Explain each command in the pipeline
    for cmd_info in &analysis.commands {
        let sub = explain_simple_command(&cmd_info.name, &cmd_info.args);
        sub_explanations.push(sub);
    }

    // Assess risks
    for risk in &analysis.risks {
        match risk.kind {
            RiskKind::ShellWrapper => {
                risk_level = RiskLevel::Danger;
                risks.push(risk.description.clone());
            }
            RiskKind::InlineEval => {
                risk_level = RiskLevel::Danger;
                risks.push(risk.description.clone());
            }
            RiskKind::Sudo => {
                risk_level = RiskLevel::Warning;
                risks.push(risk.description.clone());
            }
            RiskKind::CommandSubstitution => {
                risks.push(risk.description.clone());
            }
            RiskKind::PipeChain => {
                risks.push(risk.description.clone());
            }
            RiskKind::Subshell => {
                risks.push(risk.description.clone());
            }
            RiskKind::DynamicArgument => {
                risks.push(risk.description.clone());
            }
            RiskKind::FileRedirect => {
                if risk_level == RiskLevel::Safe {
                    risk_level = RiskLevel::Warning;
                }
                risks.push(risk.description.clone());
            }
        }
    }

    // Build overall explanation
    let explanation = if analysis.is_pipeline {
        format!(
            "This is a pipeline with {} stages: {}",
            analysis.commands.len(),
            sub_explanations.join(" → ")
        )
    } else if let Some(first) = sub_explanations.first() {
        first.clone()
    } else {
        "Unable to parse this command.".to_string()
    };

    CommandExplanation {
        command: cmd.to_string(),
        explanation,
        risk_level,
        risks,
        sub_explanations,
    }
}

fn explain_simple_command(name: &str, args: &[String]) -> String {
    match name {
        "ls" => format!("List directory contents{}", format_args(args)),
        "cd" => format!(
            "Change directory to {}",
            args.first().unwrap_or(&"~".to_string())
        ),
        "pwd" => "Print working directory".to_string(),
        "cat" => format!("Display contents of file{}", format_file_args(args)),
        "echo" => format!("Print text: {}", args.join(" ")),
        "grep" => format!(
            "Search for pattern '{}' in {}",
            args.first().unwrap_or(&"?".to_string()),
            args.get(1).unwrap_or(&"input".to_string())
        ),
        "find" => format!(
            "Search for files in {}",
            args.first().unwrap_or(&".".to_string())
        ),
        "rm" => format!(
            "Remove file{}{}",
            if args.contains(&"-r".to_string()) || args.contains(&"-rf".to_string()) {
                " (recursive)"
            } else {
                ""
            },
            format_args(args)
        ),
        "mv" => format!(
            "Move/rename {} to {}",
            args.first().unwrap_or(&"?".to_string()),
            args.get(1).unwrap_or(&"?".to_string())
        ),
        "cp" => format!(
            "Copy {} to {}",
            args.first().unwrap_or(&"?".to_string()),
            args.get(1).unwrap_or(&"?".to_string())
        ),
        "mkdir" => format!("Create directory{}", format_args(args)),
        "chmod" => format!("Change permissions: {}", args.join(" ")),
        "chown" => format!("Change ownership: {}", args.join(" ")),
        "curl" => format!(
            "Download from URL: {}",
            args.first().unwrap_or(&"?".to_string())
        ),
        "wget" => format!(
            "Download from URL: {}",
            args.first().unwrap_or(&"?".to_string())
        ),
        "git" => format!("Git command: {}", args.first().unwrap_or(&"?".to_string())),
        "cargo" => format!(
            "Cargo command: {}",
            args.first().unwrap_or(&"?".to_string())
        ),
        "npm" => format!("npm command: {}", args.first().unwrap_or(&"?".to_string())),
        "pip" => format!("pip command: {}", args.first().unwrap_or(&"?".to_string())),
        "python" | "python3" => format!(
            "Run Python script: {}",
            args.first().unwrap_or(&"REPL".to_string())
        ),
        "node" => format!(
            "Run Node.js: {}",
            args.first().unwrap_or(&"REPL".to_string())
        ),
        "docker" => format!(
            "Docker command: {}",
            args.first().unwrap_or(&"?".to_string())
        ),
        "sudo" => format!("Run with elevated privileges: {}", args.join(" ")),
        _ => format!("Run command '{}' with args: {}", name, args.join(" ")),
    }
}

fn format_args(args: &[String]) -> String {
    if args.is_empty() {
        String::new()
    } else {
        format!(" {}", args.join(" "))
    }
}

fn format_file_args(args: &[String]) -> String {
    if args.is_empty() {
        " (stdin)".to_string()
    } else {
        format!(" {}", args.join(", "))
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::shell_intel::parser::parse_command;

    #[test]
    fn test_explain_simple() {
        let analysis = parse_command("ls -la /tmp");
        let result = explain("ls -la /tmp", &analysis);
        assert_eq!(result.risk_level, RiskLevel::Safe);
        assert!(result.explanation.contains("List directory"));
    }

    #[test]
    fn test_explain_dangerous() {
        let analysis = parse_command("bash -c 'rm -rf /'");
        let result = explain("bash -c 'rm -rf /'", &analysis);
        assert_eq!(result.risk_level, RiskLevel::Danger);
        assert!(!result.risks.is_empty());
    }

    #[test]
    fn test_explain_pipeline() {
        let analysis = parse_command("cat file | grep pattern");
        let result = explain("cat file | grep pattern", &analysis);
        assert!(result.explanation.contains("pipeline"));
    }
}
