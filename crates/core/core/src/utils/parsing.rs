use bytes::Bytes;
use regex::Regex;
use std::sync::OnceLock;

/// Extracts text from a stream chunk, handling potential UTF-8 issues.
pub fn bytes_to_string(bytes: &Bytes) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// Scrub known secrets from output text to prevent logging to user or LLM traces.
#[allow(clippy::disallowed_methods)]
pub fn scrub_secrets(text: &str) -> String {
    static SECRETS_RE: OnceLock<Regex> = OnceLock::new();
    let re = SECRETS_RE.get_or_init(|| {
        Regex::new(r"sk-ant-[0-9a-zA-Z\-_]{40,}|sk-[0-9a-zA-Z\-_]{40,}|ghp_[0-9a-zA-Z]{36}|gho_[0-9a-zA-Z]{36}|glpat-[0-9a-zA-Z\-_]{20}|xox[baprs]-[0-9a-zA-Z\-]{10,}|Bearer\s+[0-9a-zA-Z\-_.]+|eyJ[a-zA-Z0-9_-]+\.eyJ[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+").expect("SECRETS_RE: hardcoded regex must compile")
    });
    re.replace_all(text, "[REDACTED]").to_string()
}

/// Maps common LLM-generated tool names to actual registered tool names
pub fn alias_tool_name(name: &str) -> &str {
    match name {
        "bash" | "sh" | "exec" | "command" | "cmd" => "shell",
        "fileread" | "readfile" | "read_file" | "file" | "list_dir" | "glob" | "search_files" => {
            "foundation"
        }
        "filewrite" | "writefile" | "write_file" => "foundation",
        "fileedit" | "editfile" | "edit_file" | "replace_in_file" => "file_atomic_edit",
        "filemove" | "move" | "rename" => "file_move",
        "filedelete" | "delete" | "rm" => "file_delete",
        "filecreate" | "create" | "touch" | "write_to_file" => "file_create",
        "memory" | "memory_search" | "recall" => "memory_search",
        "memory_write" | "store" | "remember" | "memory_append" => "memory_append",
        "shell" => "shell",
        "foundation" => "foundation",
        _ => name,
    }
}

/// Parses multiple "Action: name[args]" OR emergent XML-like "<tool_call>" patterns from LLM text.
#[allow(clippy::disallowed_methods)]
pub fn parse_actions(text: &str) -> Vec<(String, String)> {
    static LEGACY_RE: OnceLock<Regex> = OnceLock::new();
    static TOOL_CALL_RE: OnceLock<Regex> = OnceLock::new();
    static FN_RE: OnceLock<Regex> = OnceLock::new();
    static PARAM_RE: OnceLock<Regex> = OnceLock::new();

    // Format C
    static INVOKE_RE: OnceLock<Regex> = OnceLock::new();
    static INVOKE_PARAM_RE: OnceLock<Regex> = OnceLock::new();

    // Format D
    static USE_MCP_RE: OnceLock<Regex> = OnceLock::new();
    static MCP_TOOL_NAME_RE: OnceLock<Regex> = OnceLock::new();
    static MCP_ARGUMENTS_RE: OnceLock<Regex> = OnceLock::new();

    // Format E
    static FN_CALL_RE: OnceLock<Regex> = OnceLock::new();

    // OpenRouter native <name>/<arguments> format (moved out of loop)
    static OPENROUTER_NAME_RE: OnceLock<Regex> = OnceLock::new();
    static OPENROUTER_ARGS_RE: OnceLock<Regex> = OnceLock::new();

    let mut actions: Vec<(String, String)> = Vec::new();

    // 1. Legacy/Standard Parser: Action: name[args]
    let legacy_re = LEGACY_RE.get_or_init(|| {
        Regex::new(r"Action:\s*(\w+)\[(.*?)\]").expect("hardcoded regex must compile")
    });
    for cap in legacy_re.captures_iter(text) {
        actions.push((alias_tool_name(&cap[1]).to_string(), cap[2].to_string()));
    }

    // 1b. JSON-format Parser: Action: name{"key": "value"}
    if actions.is_empty() {
        static JSON_ACTION_RE: OnceLock<Regex> = OnceLock::new();
        let json_re = JSON_ACTION_RE.get_or_init(|| {
            Regex::new(r#"Action:\s*(\w+)(\{.*\})"#).expect("hardcoded regex must compile")
        });
        for cap in json_re.captures_iter(text) {
            actions.push((alias_tool_name(&cap[1]).to_string(), cap[2].to_string()));
        }
    }

    // 2. Emergent Substrate Parser (XML-like): <tool_call><function=name>...
    if text.contains("<tool_call>") {
        let tool_call_re = TOOL_CALL_RE.get_or_init(|| {
            Regex::new(r"(?s)<tool_call>.*?</tool_call>").expect("hardcoded regex must compile")
        });
        let fn_re = FN_RE.get_or_init(|| {
            Regex::new(r"<function=([\w_]+)>").expect("hardcoded regex must compile")
        });
        let param_re = PARAM_RE.get_or_init(|| {
            Regex::new(r"(?s)<parameter=([\w_]+)>(.*?)</parameter>")
                .expect("hardcoded regex must compile")
        });

        let name_re = OPENROUTER_NAME_RE.get_or_init(|| {
            Regex::new(r"(?s)<name>\s*([^<]+)\s*</name>").expect("hardcoded regex must compile")
        });
        let args_re = OPENROUTER_ARGS_RE.get_or_init(|| {
            Regex::new(r"(?s)<arguments>(.*?)</arguments>").expect("hardcoded regex must compile")
        });

        for tc_match in tool_call_re.find_iter(text) {
            let tc_text = tc_match.as_str();
            if let Some(fn_cap) = fn_re.captures(tc_text) {
                let fn_name = alias_tool_name(&fn_cap[1]).to_string();
                let mut params = serde_json::Map::new();
                for p_cap in param_re.captures_iter(tc_text) {
                    let key = p_cap[1].to_string();
                    let val = p_cap[2].trim().to_string();
                    params.insert(key, serde_json::Value::String(val));
                }
                let args_json = serde_json::Value::Object(params).to_string();
                actions.push((fn_name, args_json));
            } else {
                // Check for generic native <name> / <arguments> format injected by OpenRouter
                if let (Some(name_cap), Some(args_cap)) =
                    (name_re.captures(tc_text), args_re.captures(tc_text))
                {
                    let fn_name = alias_tool_name(name_cap[1].trim()).to_string();
                    let args_str = args_cap[1].trim();
                    let args = if let Ok(val) = serde_json::from_str::<serde_json::Value>(args_str)
                    {
                        val.to_string()
                    } else {
                        #[allow(clippy::disallowed_methods)]
                        serde_json::json!({ "payload": args_str }).to_string()
                    };
                    actions.push((fn_name, args));
                }
            }
        }
    }

    // 3. Format C - Attribute-style XML (<invoke name="x">)
    if text.contains("<invoke ") {
        let invoke_re = INVOKE_RE.get_or_init(|| {
            Regex::new(r#"(?s)<invoke\s+name=["']([^"']+)["']>(.*?)</invoke>"#)
                .expect("hardcoded regex must compile")
        });
        let param_re = INVOKE_PARAM_RE.get_or_init(|| {
            Regex::new(r#"(?s)<parameter\s+name=["']([^"']+)["']\s+value=["']([^"']+)["']\s*/?>"#)
                .expect("hardcoded regex must compile")
        });
        for invoke_cap in invoke_re.captures_iter(text) {
            let fn_name = alias_tool_name(&invoke_cap[1]).to_string();
            let invoke_body = &invoke_cap[2];
            let mut params = serde_json::Map::new();
            for p_cap in param_re.captures_iter(invoke_body) {
                params.insert(
                    p_cap[1].to_string(),
                    serde_json::Value::String(p_cap[2].to_string()),
                );
            }
            actions.push((fn_name, serde_json::Value::Object(params).to_string()));
        }
    }

    // 4. Format D - <use_mcp_tool> XML
    if text.contains("<use_mcp_tool>") {
        let use_mcp_re = USE_MCP_RE.get_or_init(|| {
            Regex::new(r"(?s)<use_mcp_tool>.*?</use_mcp_tool>")
                .expect("hardcoded regex must compile")
        });
        let tool_name_re = MCP_TOOL_NAME_RE.get_or_init(|| {
            Regex::new(r"<tool_name>([^<]+)</tool_name>").expect("hardcoded regex must compile")
        });
        let args_re = MCP_ARGUMENTS_RE.get_or_init(|| {
            Regex::new(r"(?s)<arguments>(.*?)</arguments>").expect("hardcoded regex must compile")
        });

        for mcp_match in use_mcp_re.find_iter(text) {
            let mcp_text = mcp_match.as_str();
            if let (Some(name_cap), Some(args_cap)) =
                (tool_name_re.captures(mcp_text), args_re.captures(mcp_text))
            {
                let fn_name = alias_tool_name(name_cap[1].trim()).to_string();
                let args_str = args_cap[1].trim();
                let args = if let Ok(val) = serde_json::from_str::<serde_json::Value>(args_str) {
                    val.to_string()
                } else {
                    #[allow(clippy::disallowed_methods)]
                    serde_json::json!({ "payload": args_str }).to_string()
                };
                actions.push((fn_name, args));
            }
        }
    }

    // 5. Format E - <function_call name="..." arguments="..."/>
    if text.contains("<function_call ") {
        let fn_call_re = FN_CALL_RE.get_or_init(|| {
            Regex::new(
                r#"(?s)<function_call\s+name=["']([^"']+)["']\s+arguments=["']([^"']+)["']\s*/?>"#,
            )
            .expect("hardcoded regex must compile")
        });
        for cap in fn_call_re.captures_iter(text) {
            let fn_name = alias_tool_name(&cap[1]).to_string();
            // Try to parse arguments string which might have escaped quotes
            let args_str = cap[2]
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&#34;", "\"");
            actions.push((fn_name, args_str));
        }
    }

    actions
}

/// Parses a simple "Action: name[args]" pattern from LLM text (legacy helper).
pub fn parse_action(text: &str) -> Option<(String, String)> {
    parse_actions(text).into_iter().next()
}

/// Consolidates common error logging with agent context.
pub fn log_agent_error(agent_name: &str, context: &str, error: impl std::fmt::Display) {
    tracing::error!("[{}] {}: {}", agent_name, context, error);
}
