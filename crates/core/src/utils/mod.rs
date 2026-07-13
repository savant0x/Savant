pub mod embeddings;
pub mod io;
pub mod ollama_embeddings;
pub mod ollama_vision;
pub mod parsing;
pub mod time;

/// Token count utility.
///
/// Returns the number of tokens in a string using tiktoken cl100k_base encoding.
/// Falls back to a word/character heuristic if tiktoken initialization fails.
pub fn token_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    // Try tiktoken first (accurate BPE encoding)
    if let Ok(bpe) = tiktoken_rs::cl100k_base() {
        return bpe.encode_with_special_tokens(text).len();
    }

    // Fallback heuristic: roughly 4 characters or 1 whitespace-delimited word per token
    let words = text.split_whitespace().count();
    let chars = text.len() / 4;
    std::cmp::max(words, chars).max(1)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod benches {
    // criterion benchmark stub
}
