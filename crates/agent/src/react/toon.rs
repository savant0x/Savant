//! TOON — Token-Oriented Object Notation for compressing tool results.
//!
//! When a tool returns a uniform JSON array (all objects with the same keys),
//! TOON hoists the field names into a single header line and serializes rows
//! like CSV. This reduces token count by ~61% for tabular data.
//!
//! # Example
//! ```json
//! [{"name":"Alice","age":30},{"name":"Bob","age":25}]
//! ```
//! Becomes:
//! ```text
//! [2]name|age
//! Alice|30
//! Bob|25
//! ```

use serde_json::Value;

/// TOON encoder for uniform JSON arrays.
pub struct ToonEncoder;

impl ToonEncoder {
    /// Encode a JSON value. If it's a uniform array, apply TOON compression.
    /// Otherwise, return the original JSON string.
    pub fn encode(value: &Value) -> String {
        if let Some(array) = value.as_array() {
            if Self::is_uniform_array(value) && !array.is_empty() {
                return Self::encode_array(array);
            }
        }
        // Not a uniform array — return as-is
        serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
    }

    /// Check if a value is a uniform array (all objects with the same keys).
    pub fn is_uniform_array(value: &Value) -> bool {
        let array = match value.as_array() {
            Some(a) => a,
            None => return false,
        };

        if array.is_empty() {
            return false;
        }

        // Every element must be an object
        let first = match array.first() {
            Some(Value::Object(obj)) => obj,
            _ => return false,
        };

        let first_keys: Vec<&String> = first.keys().collect();
        if first_keys.is_empty() {
            return false;
        }

        array.iter().all(|elem| {
            if let Value::Object(obj) = elem {
                let keys: Vec<&String> = obj.keys().collect();
                keys == first_keys
            } else {
                false
            }
        })
    }

    /// Encode a uniform array in TOON format.
    fn encode_array(array: &[Value]) -> String {
        let first = match array.first() {
            Some(Value::Object(obj)) => obj,
            _ => return serde_json::to_string(array).unwrap_or_default(),
        };

        let keys: Vec<&String> = first.keys().collect();
        let header = keys
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join("|");

        let mut output = format!("[{}]{}", array.len(), header);

        for elem in array {
            if let Value::Object(obj) = elem {
                output.push('\n');
                let row: Vec<String> = keys
                    .iter()
                    .map(|k| obj.get(*k).map(Self::value_to_string).unwrap_or_default())
                    .collect();
                output.push_str(&row.join("|"));
            }
        }

        output
    }

    /// Convert a JSON value to a compact string for TOON rows.
    fn value_to_string(value: &Value) -> String {
        match value {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            Value::Array(_) | Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_is_uniform_array_true() {
        let val = json!([{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}]);
        assert!(ToonEncoder::is_uniform_array(&val));
    }

    #[test]
    fn test_is_uniform_array_false_mixed() {
        let val = json!([{"name": "Alice"}, "not an object"]);
        assert!(!ToonEncoder::is_uniform_array(&val));
    }

    #[test]
    fn test_is_uniform_array_false_different_keys() {
        let val = json!([{"name": "Alice"}, {"age": 25}]);
        assert!(!ToonEncoder::is_uniform_array(&val));
    }

    #[test]
    fn test_is_uniform_array_false_empty() {
        let val = json!([]);
        assert!(!ToonEncoder::is_uniform_array(&val));
    }

    #[test]
    fn test_is_uniform_array_false_not_array() {
        let val = json!({"name": "Alice"});
        assert!(!ToonEncoder::is_uniform_array(&val));
    }

    #[test]
    fn test_encode_uniform_array() {
        let val = json!([{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}]);
        let encoded = ToonEncoder::encode(&val);
        assert!(encoded.starts_with("[2]"));
        // Key order depends on serde_json internals — check both orderings
        assert!(encoded.contains("name|age") || encoded.contains("age|name"));
        assert!(encoded.contains("Alice|30") || encoded.contains("30|Alice"));
        assert!(encoded.contains("Bob|25") || encoded.contains("25|Bob"));
    }

    #[test]
    fn test_encode_non_uniform_passthrough() {
        let val = json!({"key": "value"});
        let encoded = ToonEncoder::encode(&val);
        let parsed: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(parsed, val);
    }

    #[test]
    fn test_encode_empty_array_passthrough() {
        let val = json!([]);
        let encoded = ToonEncoder::encode(&val);
        assert_eq!(encoded, "[]");
    }

    #[test]
    fn test_encode_with_null_values() {
        let val = json!([{"name": "Alice", "score": null}, {"name": "Bob", "score": 95}]);
        let encoded = ToonEncoder::encode(&val);
        assert!(encoded.contains("Alice|"));
        assert!(encoded.contains("Bob|95"));
    }

    #[test]
    fn test_encode_single_element() {
        let val = json!([{"x": 1, "y": 2}]);
        let encoded = ToonEncoder::encode(&val);
        assert!(encoded.starts_with("[1]"));
        assert!(encoded.contains("x|y"));
        assert!(encoded.contains("1|2"));
    }

    #[test]
    fn test_token_reduction() {
        // Simulate a large result set
        let array: Vec<Value> = (0..100)
            .map(|i| json!({"id": i, "name": format!("user_{}", i), "active": true}))
            .collect();
        let val = Value::Array(array);

        let json_size = serde_json::to_string(&val).unwrap().len();
        let toon_size = ToonEncoder::encode(&val).len();

        // TOON should be smaller than JSON for uniform arrays
        assert!(
            toon_size < json_size,
            "TOON ({}) should be smaller than JSON ({})",
            toon_size,
            json_size
        );
    }
}
