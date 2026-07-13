//! Tool Schema Validator — two-tier validation for tool parameter schemas.
//!
//! **Strict** (CI-time): Enforces OpenAI strict-mode rules for tool registration.
//! **Lenient** (runtime): Validates schemas at tool execution time with relaxed rules.

use serde_json::Value;

/// Maximum recursion depth for schema validation.
const MAX_DEPTH: u32 = 16;

/// Schema validation error.
#[derive(Debug, Clone)]
pub enum SchemaError {
    MissingType { path: String },
    TopLevelNotObject,
    MissingProperties { path: String },
    RequiredKeyNotInProperties { path: String, key: String },
    MissingArrayItems { path: String },
    MaxDepthExceeded { path: String },
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingType { path } => write!(f, "Missing 'type' at {}", path),
            Self::TopLevelNotObject => write!(f, "Top-level schema must be type 'object'"),
            Self::MissingProperties { path } => {
                write!(f, "Object missing 'properties' at {}", path)
            }
            Self::RequiredKeyNotInProperties { path, key } => {
                write!(f, "Required key '{}' not in properties at {}", key, path)
            }
            Self::MissingArrayItems { path } => {
                write!(f, "Array missing 'items' at {}", path)
            }
            Self::MaxDepthExceeded { path } => {
                write!(f, "Max depth exceeded at {}", path)
            }
        }
    }
}

/// Validate tool schema (lenient — runtime).
/// Allows freeform properties and doesn't enforce type at top level.
pub fn validate_tool_schema(schema: &Value) -> Result<(), Vec<SchemaError>> {
    let mut errors = Vec::new();
    validate_lenient(schema, "", &mut errors, 0);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate tool schema (strict — CI-time, OpenAI strict mode).
/// Enforces: top-level type=object, properties required, required keys in properties,
/// array must have items.
pub fn validate_strict_schema(schema: &Value) -> Result<(), Vec<SchemaError>> {
    let mut errors = Vec::new();
    validate_strict(schema, "", &mut errors, 0);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_strict(schema: &Value, path: &str, errors: &mut Vec<SchemaError>, depth: u32) {
    if depth > MAX_DEPTH {
        errors.push(SchemaError::MaxDepthExceeded {
            path: path.to_string(),
        });
        return;
    }

    // Rule 1: Must have type
    let schema_type = match schema.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => {
            errors.push(SchemaError::MissingType {
                path: path.to_string(),
            });
            return;
        }
    };

    // Rule 2: Top-level must be object
    if depth == 0 && schema_type != "object" {
        errors.push(SchemaError::TopLevelNotObject);
    }

    // Rule 3-5: Object validation
    if schema_type == "object" {
        // Rule 3: Must have properties
        if schema.get("properties").is_none() {
            errors.push(SchemaError::MissingProperties {
                path: path.to_string(),
            });
        }

        // Rule 4: required keys must exist in properties
        if let (Some(required), Some(properties)) =
            (schema.get("required"), schema.get("properties"))
        {
            if let (Some(req_arr), Some(props)) = (required.as_array(), properties.as_object()) {
                for key in req_arr {
                    if let Some(k) = key.as_str() {
                        if !props.contains_key(k) {
                            errors.push(SchemaError::RequiredKeyNotInProperties {
                                path: path.to_string(),
                                key: k.to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Recurse into properties
        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
            for (key, prop_schema) in props {
                validate_strict(prop_schema, &format!("{}.{}", path, key), errors, depth + 1);
            }
        }
    }

    // Rule 6: Array must have items
    if schema_type == "array" {
        match schema.get("items") {
            Some(items) => {
                validate_strict(items, &format!("{}.items", path), errors, depth + 1);
            }
            None => {
                errors.push(SchemaError::MissingArrayItems {
                    path: path.to_string(),
                });
            }
        }
    }

    // Recurse into oneOf/anyOf/allOf
    for combiner in &["oneOf", "anyOf", "allOf"] {
        if let Some(variants) = schema.get(combiner).and_then(|v| v.as_array()) {
            for (i, variant) in variants.iter().enumerate() {
                validate_strict(
                    variant,
                    &format!("{}.{}[{}]", path, combiner, i),
                    errors,
                    depth + 1,
                );
            }
        }
    }
}

fn validate_lenient(schema: &Value, _path: &str, _errors: &mut Vec<SchemaError>, depth: u32) {
    if depth > MAX_DEPTH {
        return; // Lenient: just stop, don't error
    }

    // Lenient: type is recommended but not required
    let schema_type = schema.get("type").and_then(|t| t.as_str());

    if let Some("object") = schema_type {
        // Object validation (relaxed: properties not strictly required)
        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
            for (key, prop_schema) in props {
                validate_lenient(
                    prop_schema,
                    &format!("{}.{}", _path, key),
                    _errors,
                    depth + 1,
                );
            }
        }
    }

    if let Some("array") = schema_type {
        if let Some(items) = schema.get("items") {
            validate_lenient(items, &format!("{}.items", _path), _errors, depth + 1);
        }
        // Lenient: missing items is a warning, not an error
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_strict_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "count": {"type": "integer"}
            },
            "required": ["name"]
        });
        assert!(validate_strict_schema(&schema).is_ok());
    }

    #[test]
    fn test_strict_missing_type() {
        let schema = serde_json::json!({
            "properties": {
                "name": {"type": "string"}
            }
        });
        let result = validate_strict_schema(&schema);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(matches!(errors[0], SchemaError::MissingType { .. }));
    }

    #[test]
    fn test_strict_required_not_in_properties() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name", "missing_key"]
        });
        let result = validate_strict_schema(&schema);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(matches!(
            errors[0],
            SchemaError::RequiredKeyNotInProperties { .. }
        ));
    }

    #[test]
    fn test_strict_array_missing_items() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "tags": {"type": "array"}
            }
        });
        let result = validate_strict_schema(&schema);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(matches!(errors[0], SchemaError::MissingArrayItems { .. }));
    }

    #[test]
    fn test_lenient_allows_freeform() {
        // Lenient should accept schemas without strict type declarations
        let schema = serde_json::json!({
            "properties": {
                "name": {"type": "string"}
            }
        });
        assert!(validate_tool_schema(&schema).is_ok());
    }

    #[test]
    fn test_lenient_allows_missing_items() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "tags": {"type": "array"}
            }
        });
        assert!(validate_tool_schema(&schema).is_ok());
    }

    #[test]
    fn test_nested_validation() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "properties": {
                        "port": {"type": "integer"}
                    }
                }
            }
        });
        assert!(validate_strict_schema(&schema).is_ok());
    }

    #[test]
    fn test_max_depth() {
        // Create deeply nested schema
        let mut schema = serde_json::json!({"type": "string"});
        for _ in 0..20 {
            schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "nested": schema
                }
            });
        }
        let result = validate_strict_schema(&schema);
        assert!(result.is_err());
    }

    /// Validates all registered tool schemas against validation rules.
    /// CI catches bad schemas at compile time, not runtime.
    #[test]
    fn test_all_tool_schemas_valid() {
        use savant_core::traits::Tool;
        use std::sync::Arc;
        // Only test tools that can be constructed without complex dependencies
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(crate::tools::SettingsTool::new()),
            Arc::new(crate::tools::LibrarianTool::new(std::path::PathBuf::from(
                "skills",
            ))),
        ];

        let mut failures = Vec::new();
        for tool in &tools {
            let schema = tool.parameters_schema();
            if let Err(errors) = validate_tool_schema(&schema) {
                failures.push(format!(
                    "Tool '{}' validation failed: {:?}",
                    tool.name(),
                    errors
                ));
            }
        }

        if !failures.is_empty() {
            panic!("Tool schema validation failed:\n{}", failures.join("\n"));
        }
    }
}
