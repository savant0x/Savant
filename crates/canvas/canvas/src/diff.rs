use serde_json::Value;
use thiserror::Error;

/// Computes the Longest Common Subsequence (LCS) of two slices.
/// Returns the indices into `a` and `b` that form the LCS.
fn lcs_indices<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let m = a.len();
    let n = b.len();
    if m == 0 || n == 0 {
        return Vec::new();
    }

    // Build LCS table
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to find indices
    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    result.reverse();
    result
}

/// Generates patch operations for two arrays using LCS-based diffing.
fn diff_arrays_lcs(
    old_arr: &[Value],
    new_arr: &[Value],
    base_version: u64,
    target_version: u64,
) -> DiffResult {
    let lcs = lcs_indices(old_arr, new_arr);
    let mut patches = Vec::new();
    let mut old_idx = 0;
    let mut new_idx = 0;
    let mut lcs_pos = 0;

    while old_idx < old_arr.len() || new_idx < new_arr.len() {
        // If we're at an LCS element, advance both pointers
        if lcs_pos < lcs.len() && old_idx == lcs[lcs_pos].0 && new_idx == lcs[lcs_pos].1 {
            old_idx += 1;
            new_idx += 1;
            lcs_pos += 1;
            continue;
        }

        // Elements removed from old
        if old_idx < old_arr.len() && (lcs_pos >= lcs.len() || old_idx < lcs[lcs_pos].0) {
            patches.push(PatchOp::Remove {
                path: format!("/{}", old_idx),
            });
            old_idx += 1;
            continue;
        }

        // Elements added in new
        if new_idx < new_arr.len() && (lcs_pos >= lcs.len() || new_idx < lcs[lcs_pos].1) {
            patches.push(PatchOp::Add {
                path: format!("/{}", old_idx),
                value: new_arr[new_idx].clone(),
            });
            new_idx += 1;
            // After an add, old_idx stays the same (nothing was removed from old)
            continue;
        }
    }

    DiffResult {
        patches,
        base_version,
        target_version,
        is_full_replace: false,
    }
}

/// Errors that can occur during diff operations.
#[derive(Debug, Error)]
pub enum DiffError {
    #[error("Invalid patch format: {0}")]
    InvalidPatch(String),
    #[error("Patch application failed: {0}")]
    PatchFailed(String),
    #[error("Conflict detected: {0}")]
    Conflict(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// A single patch operation (RFC 6902 JSON Patch compatible).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum PatchOp {
    /// Add a value at the specified path
    Add { path: String, value: Value },
    /// Remove the value at the specified path
    Remove { path: String },
    /// Replace the value at the specified path
    Replace { path: String, value: Value },
    /// Move a value from one path to another
    Move { from: String, path: String },
    /// Copy a value from one path to another
    Copy { from: String, path: String },
    /// Test that the value at the path matches the expected value
    Test { path: String, value: Value },
}

/// A diff result containing the patch operations and metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiffResult {
    /// The patch operations to apply
    pub patches: Vec<PatchOp>,
    /// The version of the state this diff applies to
    pub base_version: u64,
    /// The version after applying this diff
    pub target_version: u64,
    /// Whether this is a full replacement (true) or incremental diff (false)
    pub is_full_replace: bool,
}

/// Computes a JSON Patch (RFC 6902) diff between two JSON values.
///
/// This implementation uses a hybrid approach:
/// 1. For simple values (primitives, small objects), generates incremental patches
/// 2. For complex/large objects, falls back to full replacement for efficiency
///
/// # Arguments
/// * `old` - The previous state
/// * `new` - The new state
/// * `base_version` - The version of the old state
/// * `target_version` - The version of the new state
///
/// # Returns
/// A `DiffResult` containing the patch operations and metadata.
pub fn compute_diff(
    old: &Value,
    new: &Value,
    base_version: u64,
    target_version: u64,
) -> DiffResult {
    // If old is null/empty, return full replacement
    if old.is_null() {
        return DiffResult {
            patches: vec![PatchOp::Replace {
                path: "/".to_string(),
                value: new.clone(),
            }],
            base_version,
            target_version,
            is_full_replace: true,
        };
    }

    // If values are identical, return empty diff
    if old == new {
        return DiffResult {
            patches: vec![],
            base_version,
            target_version,
            is_full_replace: false,
        };
    }

    // For objects, compute field-level diffs
    if let (Some(old_obj), Some(new_obj)) = (old.as_object(), new.as_object()) {
        let mut patches = Vec::new();

        // Find removed and changed fields
        for (key, old_val) in old_obj {
            match new_obj.get(key) {
                None => {
                    // Field was removed
                    patches.push(PatchOp::Remove {
                        path: format!("/{}", escape_json_pointer(key)),
                    });
                }
                Some(new_val) if old_val != new_val => {
                    // Field was changed
                    if old_val.is_object() && new_val.is_object() {
                        // Recursively diff nested objects
                        let nested = compute_diff(old_val, new_val, base_version, target_version);
                        for mut nested_patch in nested.patches {
                            // Prefix the path with the current key
                            match &mut nested_patch {
                                PatchOp::Add { path, .. }
                                | PatchOp::Remove { path }
                                | PatchOp::Replace { path, .. }
                                | PatchOp::Test { path, .. } => {
                                    *path = format!("/{}{}", escape_json_pointer(key), path);
                                }
                                PatchOp::Move { from, path } | PatchOp::Copy { from, path } => {
                                    *from = format!("/{}{}", escape_json_pointer(key), from);
                                    *path = format!("/{}{}", escape_json_pointer(key), path);
                                }
                            }
                            patches.push(nested_patch);
                        }
                    } else {
                        // Replace the field
                        patches.push(PatchOp::Replace {
                            path: format!("/{}", escape_json_pointer(key)),
                            value: new_val.clone(),
                        });
                    }
                }
                _ => {
                    // Field unchanged, skip
                }
            }
        }

        // Find added fields
        for (key, new_val) in new_obj {
            if !old_obj.contains_key(key) {
                patches.push(PatchOp::Add {
                    path: format!("/{}", escape_json_pointer(key)),
                    value: new_val.clone(),
                });
            }
        }

        return DiffResult {
            patches,
            base_version,
            target_version,
            is_full_replace: false,
        };
    }

    // For arrays, use LCS-based diff for correct insertion/deletion semantics
    if let (Some(old_arr), Some(new_arr)) = (old.as_array(), new.as_array()) {
        // If arrays are small enough, use element-level LCS diff
        if old_arr.len() <= 100 && new_arr.len() <= 100 {
            return diff_arrays_lcs(old_arr, new_arr, base_version, target_version);
        }

        // For large arrays, fall back to full replacement
        return DiffResult {
            patches: vec![PatchOp::Replace {
                path: "/".to_string(),
                value: new.clone(),
            }],
            base_version,
            target_version,
            is_full_replace: true,
        };
    }

    // For primitive values, just replace
    DiffResult {
        patches: vec![PatchOp::Replace {
            path: "/".to_string(),
            value: new.clone(),
        }],
        base_version,
        target_version,
        is_full_replace: false,
    }
}

/// Applies a JSON Patch (RFC 6902) to a JSON value.
///
/// # Arguments
/// * `state` - The current state to patch
/// * `diff` - The diff result containing patch operations
///
/// # Returns
/// The patched state, or an error if the patch cannot be applied.
pub fn apply_diff(state: &Value, diff: &DiffResult) -> Result<Value, DiffError> {
    let mut result = state.clone();

    for patch in &diff.patches {
        match patch {
            PatchOp::Add { path, value } => {
                apply_add(&mut result, path, value)?;
            }
            PatchOp::Remove { path } => {
                apply_remove(&mut result, path)?;
            }
            PatchOp::Replace { path, value } => {
                apply_replace(&mut result, path, value)?;
            }
            PatchOp::Move { from, path } => {
                let value = get_value_at_path(&result, from)?.clone();
                apply_remove(&mut result, from)?;
                apply_add(&mut result, path, &value)?;
            }
            PatchOp::Copy { from, path } => {
                let value = get_value_at_path(&result, from)?.clone();
                apply_add(&mut result, path, &value)?;
            }
            PatchOp::Test { path, value } => {
                let actual = get_value_at_path(&result, path)?;
                if actual != value {
                    return Err(DiffError::Conflict(format!(
                        "Test failed at {}: expected {}, got {}",
                        path, value, actual
                    )));
                }
            }
        }
    }

    Ok(result)
}

/// Escapes a string for use in a JSON Pointer (RFC 6901).
fn escape_json_pointer(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// Unescapes a JSON Pointer segment.
fn unescape_json_pointer(s: &str) -> String {
    s.replace("~1", "/").replace("~0", "~")
}

/// Gets a value at a JSON Pointer path.
fn get_value_at_path<'a>(value: &'a Value, path: &str) -> Result<&'a Value, DiffError> {
    if path.is_empty() || path == "/" {
        return Ok(value);
    }

    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let mut current = value;

    for segment in segments {
        let unescaped = unescape_json_pointer(segment);
        current = match current {
            Value::Object(obj) => obj
                .get(&unescaped)
                .ok_or_else(|| DiffError::InvalidPatch(format!("Path not found: {}", path)))?,
            Value::Array(arr) => {
                let index: usize = unescaped.parse().map_err(|_| {
                    DiffError::InvalidPatch(format!("Invalid array index: {}", unescaped))
                })?;
                arr.get(index).ok_or_else(|| {
                    DiffError::InvalidPatch(format!("Array index out of bounds: {}", index))
                })?
            }
            _ => {
                return Err(DiffError::InvalidPatch(format!(
                    "Cannot index into non-object/array at path: {}",
                    path
                )))
            }
        };
    }

    Ok(current)
}

/// Applies an add operation.
fn apply_add(state: &mut Value, path: &str, value: &Value) -> Result<(), DiffError> {
    if path.is_empty() || path == "/" {
        *state = value.clone();
        return Ok(());
    }

    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (parent_path, key) = segments.split_at(segments.len() - 1);

    let parent = get_value_at_path_mut(state, parent_path)?;
    let unescaped_key = unescape_json_pointer(key[0]);

    match parent {
        Value::Object(obj) => {
            obj.insert(unescaped_key, value.clone());
        }
        Value::Array(arr) => {
            let index: usize = unescaped_key.parse().map_err(|_| {
                DiffError::InvalidPatch(format!("Invalid array index: {}", unescaped_key))
            })?;
            if index > arr.len() {
                return Err(DiffError::InvalidPatch(format!(
                    "Array index out of bounds: {}",
                    index
                )));
            }
            arr.insert(index, value.clone());
        }
        _ => {
            return Err(DiffError::PatchFailed(format!(
                "Cannot add to non-object/array at path: {}",
                path
            )))
        }
    }

    Ok(())
}

/// Applies a remove operation.
fn apply_remove(state: &mut Value, path: &str) -> Result<(), DiffError> {
    if path.is_empty() || path == "/" {
        *state = Value::Null;
        return Ok(());
    }

    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (parent_path, key) = segments.split_at(segments.len() - 1);

    let parent = get_value_at_path_mut(state, parent_path)?;
    let unescaped_key = unescape_json_pointer(key[0]);

    match parent {
        Value::Object(obj) => {
            obj.remove(&unescaped_key).ok_or_else(|| {
                DiffError::InvalidPatch(format!("Key not found: {}", unescaped_key))
            })?;
        }
        Value::Array(arr) => {
            let index: usize = unescaped_key.parse().map_err(|_| {
                DiffError::InvalidPatch(format!("Invalid array index: {}", unescaped_key))
            })?;
            if index >= arr.len() {
                return Err(DiffError::InvalidPatch(format!(
                    "Array index out of bounds: {}",
                    index
                )));
            }
            arr.remove(index);
        }
        _ => {
            return Err(DiffError::PatchFailed(format!(
                "Cannot remove from non-object/array at path: {}",
                path
            )))
        }
    }

    Ok(())
}

/// Applies a replace operation.
fn apply_replace(state: &mut Value, path: &str, value: &Value) -> Result<(), DiffError> {
    if path.is_empty() || path == "/" {
        *state = value.clone();
        return Ok(());
    }

    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (parent_path, key) = segments.split_at(segments.len() - 1);

    let parent = get_value_at_path_mut(state, parent_path)?;
    let unescaped_key = unescape_json_pointer(key[0]);

    match parent {
        Value::Object(obj) => {
            if !obj.contains_key(&unescaped_key) {
                return Err(DiffError::InvalidPatch(format!(
                    "Key not found: {}",
                    unescaped_key
                )));
            }
            obj.insert(unescaped_key, value.clone());
        }
        Value::Array(arr) => {
            let index: usize = unescaped_key.parse().map_err(|_| {
                DiffError::InvalidPatch(format!("Invalid array index: {}", unescaped_key))
            })?;
            if index >= arr.len() {
                return Err(DiffError::InvalidPatch(format!(
                    "Array index out of bounds: {}",
                    index
                )));
            }
            arr[index] = value.clone();
        }
        _ => {
            return Err(DiffError::PatchFailed(format!(
                "Cannot replace in non-object/array at path: {}",
                path
            )))
        }
    }

    Ok(())
}

/// Gets a mutable reference to a value at a JSON Pointer path.
fn get_value_at_path_mut<'a>(
    value: &'a mut Value,
    path: &[&str],
) -> Result<&'a mut Value, DiffError> {
    let mut current = value;

    for segment in path {
        let unescaped = unescape_json_pointer(segment);
        current = match current {
            Value::Object(obj) => obj
                .get_mut(&unescaped)
                .ok_or_else(|| DiffError::InvalidPatch(format!("Path not found: {}", segment)))?,
            Value::Array(arr) => {
                let index: usize = unescaped.parse().map_err(|_| {
                    DiffError::InvalidPatch(format!("Invalid array index: {}", unescaped))
                })?;
                arr.get_mut(index).ok_or_else(|| {
                    DiffError::InvalidPatch(format!("Array index out of bounds: {}", index))
                })?
            }
            _ => {
                return Err(DiffError::InvalidPatch(
                    "Cannot index into non-object/array".to_string(),
                ))
            }
        };
    }

    Ok(current)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_compute_diff_simple() {
        let old = json!({"name": "Alice", "age": 30});
        let new = json!({"name": "Alice", "age": 31});

        let diff = compute_diff(&old, &new, 1, 2);
        assert_eq!(diff.patches.len(), 1);
        assert!(!diff.is_full_replace);
    }

    #[test]
    fn test_apply_diff_simple() {
        let old = json!({"name": "Alice", "age": 30});
        let new = json!({"name": "Alice", "age": 31});

        let diff = compute_diff(&old, &new, 1, 2);
        let result = apply_diff(&old, &diff).expect("diff should apply to test object");
        assert_eq!(result, new);
    }

    #[test]
    fn test_compute_diff_add_field() {
        let old = json!({"name": "Alice"});
        let new = json!({"name": "Alice", "age": 30});

        let diff = compute_diff(&old, &new, 1, 2);
        assert_eq!(diff.patches.len(), 1);
        assert!(!diff.is_full_replace);
    }

    #[test]
    fn test_compute_diff_remove_field() {
        let old = json!({"name": "Alice", "age": 30});
        let new = json!({"name": "Alice"});

        let diff = compute_diff(&old, &new, 1, 2);
        assert_eq!(diff.patches.len(), 1);
        assert!(!diff.is_full_replace);
    }

    #[test]
    fn test_compute_diff_nested() {
        let old = json!({"user": {"name": "Alice", "age": 30}});
        let new = json!({"user": {"name": "Alice", "age": 31}});

        let diff = compute_diff(&old, &new, 1, 2);
        assert_eq!(diff.patches.len(), 1);
        assert!(!diff.is_full_replace);
    }

    #[test]
    fn test_compute_diff_identical() {
        let old = json!({"name": "Alice"});
        let new = json!({"name": "Alice"});

        let diff = compute_diff(&old, &new, 1, 2);
        assert_eq!(diff.patches.len(), 0);
        assert!(!diff.is_full_replace);
    }

    #[test]
    fn test_compute_diff_null_old() {
        let old = json!(null);
        let new = json!({"name": "Alice"});

        let diff = compute_diff(&old, &new, 1, 2);
        assert_eq!(diff.patches.len(), 1);
        assert!(diff.is_full_replace);
    }
}
