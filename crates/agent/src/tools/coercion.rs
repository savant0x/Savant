//! Tool Argument Coercion — handles LLM output format mismatches.
//!
//! LLMs often return tool arguments in slightly wrong formats:
//! - String where integer expected: `{"count": "5"}` → `{"count": 5}`
//! - Empty string for optional fields: `{"name": ""}` → `{"name": null}`
//! - Stringified arrays/objects: `{"tags": "[\"a\"]"}` → `{"tags": ["a"]}`
//!
//! This module coerces arguments against the tool's JSON Schema before execution.

use serde_json::Value;

/// Maximum recursion depth for $ref resolution.
const MAX_REF_DEPTH: u32 = 16;

/// Main entry point — coerce tool arguments against a JSON Schema.
pub fn prepare_tool_params(args: &Value, schema: &Value) -> Value {
    let resolved_schema = resolve_refs(schema, schema, 0);
    coerce_value(args, &resolved_schema)
}

/// Resolve $ref pointers inline.
/// Supports both draft-07 (`#/definitions/`) and 2020-12 (`#/$defs/`).
/// `root` is the original root schema for JSON pointer resolution.
fn resolve_refs(schema: &Value, root: &Value, depth: u32) -> Value {
    if depth > MAX_REF_DEPTH {
        return schema.clone();
    }
    match schema.get("$ref") {
        Some(Value::String(ref_str)) => {
            let resolved = resolve_json_pointer(root, ref_str);
            resolve_refs(&resolved, root, depth + 1)
        }
        _ => {
            // Recurse into object/array properties
            if let Some(obj) = schema.as_object() {
                let mut result = serde_json::Map::new();
                for (k, v) in obj {
                    result.insert(k.clone(), resolve_refs(v, root, depth + 1));
                }
                Value::Object(result)
            } else if let Some(arr) = schema.as_array() {
                Value::Array(
                    arr.iter()
                        .map(|v| resolve_refs(v, root, depth + 1))
                        .collect(),
                )
            } else {
                schema.clone()
            }
        }
    }
}

/// Resolve a JSON pointer like `#/definitions/Foo` or `#/$defs/Bar`.
fn resolve_json_pointer(root: &Value, pointer: &str) -> Value {
    let pointer = pointer.trim_start_matches('#');
    if pointer.is_empty() {
        return root.clone();
    }
    let parts: Vec<&str> = pointer.trim_start_matches('/').split('/').collect();
    let mut current = root;
    for part in parts {
        // Unescape ~1 and ~0 per RFC 6901
        let unescaped = part.replace("~1", "/").replace("~0", "~");
        match current.get(&unescaped) {
            Some(next) => current = next,
            None => return Value::Null,
        }
    }
    current.clone()
}

/// Core recursive coercion.
fn coerce_value(value: &Value, schema: &Value) -> Value {
    let schema_type = schema.get("type").and_then(|t| t.as_str());

    match (value, schema_type) {
        // Empty string → null (when schema allows null)
        (Value::String(s), _) if s.is_empty() && schema_allows_null(schema) => Value::Null,

        // String → typed coercion
        (Value::String(s), Some("integer")) => s
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| value.clone()),
        (Value::String(s), Some("number")) => s
            .parse::<f64>()
            .map(Value::from)
            .unwrap_or_else(|_| value.clone()),
        (Value::String(s), Some("boolean")) => s
            .parse::<bool>()
            .map(Value::from)
            .unwrap_or_else(|_| value.clone()),
        (Value::String(s), Some("array")) => {
            serde_json::from_str(s).unwrap_or_else(|_| value.clone())
        }
        (Value::String(s), Some("object")) => {
            serde_json::from_str(s).unwrap_or_else(|_| value.clone())
        }

        // Object coercion — coerce each property against its schema
        (Value::Object(map), Some("object")) => {
            let props = schema.get("properties").and_then(|p| p.as_object());
            let new_map: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| {
                    let prop_schema = props
                        .and_then(|p| p.get(k.as_str()))
                        .unwrap_or(&Value::Null);
                    (k.clone(), coerce_value(v, prop_schema))
                })
                .collect();
            Value::Object(new_map)
        }

        // Array coercion — coerce each element
        (Value::Array(arr), Some("array")) => {
            let items_schema = schema.get("items").unwrap_or(&Value::Null);
            Value::Array(arr.iter().map(|v| coerce_value(v, items_schema)).collect())
        }

        // oneOf/anyOf discriminator matching
        (_, _) if schema.get("oneOf").is_some() || schema.get("anyOf").is_some() => {
            if let Some(variants) = schema.get("oneOf").or_else(|| schema.get("anyOf")) {
                find_discriminated_variant(value, variants)
                    .map(|v_schema| coerce_value(value, v_schema))
                    .unwrap_or_else(|| value.clone())
            } else {
                value.clone()
            }
        }

        // No coercion needed
        _ => value.clone(),
    }
}

/// Check if schema allows null type.
fn schema_allows_null(schema: &Value) -> bool {
    if let Some(types) = schema.get("type").and_then(|t| t.as_array()) {
        types.iter().any(|t| t.as_str() == Some("null"))
    } else {
        // No type constraint → allow null
        schema.get("type").is_none()
    }
}

/// Find matching variant in oneOf/anyOf via discriminator.
fn find_discriminated_variant<'a>(value: &'a Value, variants: &'a Value) -> Option<&'a Value> {
    let arr = variants.as_array()?;
    let obj = value.as_object()?;

    for variant in arr {
        // Check const discriminator
        if let Some(const_val) = variant.get("const") {
            if let Some(const_obj) = const_val.as_object() {
                if const_obj.iter().all(|(k, v)| obj.get(k) == Some(v)) {
                    return Some(variant);
                }
            }
        }
        // Check single-element enum discriminator
        if let Some(enums) = variant.get("enum").and_then(|e| e.as_array()) {
            if enums.len() == 1 && obj.values().any(|v| v == &enums[0]) {
                return Some(variant);
            }
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn schema(props: &[(&str, &str)]) -> Value {
        let mut properties = serde_json::Map::new();
        for (name, typ) in props {
            properties.insert(name.to_string(), serde_json::json!({"type": typ}));
        }
        serde_json::json!({
            "type": "object",
            "properties": properties
        })
    }

    #[test]
    fn test_string_to_integer() {
        let args = serde_json::json!({"count": "5"});
        let s = schema(&[("count", "integer")]);
        let result = prepare_tool_params(&args, &s);
        assert_eq!(result["count"], 5);
    }

    #[test]
    fn test_string_to_number() {
        let args = serde_json::json!({"temperature": "0.7"});
        let s = schema(&[("temperature", "number")]);
        let result = prepare_tool_params(&args, &s);
        assert_eq!(result["temperature"], 0.7);
    }

    #[test]
    fn test_string_to_boolean() {
        let args = serde_json::json!({"enabled": "true"});
        let s = schema(&[("enabled", "boolean")]);
        let result = prepare_tool_params(&args, &s);
        assert_eq!(result["enabled"], true);
    }

    #[test]
    fn test_empty_string_to_null() {
        let args = serde_json::json!({"name": ""});
        let s = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": ["string", "null"]}
            }
        });
        let result = prepare_tool_params(&args, &s);
        assert!(result["name"].is_null());
    }

    #[test]
    fn test_stringified_array() {
        let args = serde_json::json!({"tags": "[\"a\",\"b\"]"});
        let s = serde_json::json!({
            "type": "object",
            "properties": {
                "tags": {"type": "array", "items": {"type": "string"}}
            }
        });
        let result = prepare_tool_params(&args, &s);
        assert!(result["tags"].is_array());
        assert_eq!(result["tags"][0], "a");
    }

    #[test]
    fn test_nested_object_coercion() {
        let args = serde_json::json!({"config": {"port": "8080", "debug": "true"}});
        let s = serde_json::json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "properties": {
                        "port": {"type": "integer"},
                        "debug": {"type": "boolean"}
                    }
                }
            }
        });
        let result = prepare_tool_params(&args, &s);
        assert_eq!(result["config"]["port"], 8080);
        assert_eq!(result["config"]["debug"], true);
    }

    #[test]
    fn test_no_coercion_needed() {
        let args = serde_json::json!({"command": "ls -la"});
        let s = schema(&[("command", "string")]);
        let result = prepare_tool_params(&args, &s);
        assert_eq!(result["command"], "ls -la");
    }

    #[test]
    fn test_ref_resolution() {
        let args = serde_json::json!({"port": "3000"});
        let s = serde_json::json!({
            "type": "object",
            "properties": {
                "port": {"$ref": "#/definitions/Port"}
            },
            "definitions": {
                "Port": {"type": "integer"}
            }
        });
        let result = prepare_tool_params(&args, &s);
        assert_eq!(result["port"], 3000);
    }
}
