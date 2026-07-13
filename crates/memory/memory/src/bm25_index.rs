//! BM25 Keyword Search Index (MEM-04)
//!
//! Provides keyword-based search as an alternative/complement to HNSW semantic search.
//! Uses the BM25 ranking function with a Porter stemmer and synonym expansion.
//!
//! The index is stored in-memory and can be persisted to CortexaDB for crash recovery.

use std::collections::HashMap;

/// Default BM25 parameters.
const DEFAULT_BM25_K1: f32 = 1.2;
const DEFAULT_BM25_B: f32 = 0.75;
/// Default maximum number of documents in the BM25 index.
const DEFAULT_MAX_BM25_DOCUMENTS: usize = 50_000;

/// A single document in the BM25 index.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Bm25Document {
    /// Tokenized and stemmed terms.
    terms: Vec<String>,
    /// Document length in terms.
    doc_len: usize,
}

/// Posting list entry: document ID + term frequency.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Posting {
    doc_id: u64,
    term_freq: u32,
}

/// Serializable snapshot of BM25 state for CortexaDB persistence.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Bm25Snapshot {
    inverted_index: HashMap<String, Vec<Posting>>,
    documents: HashMap<u64, Bm25Document>,
    doc_count: usize,
    avg_doc_len: f32,
    total_doc_len: usize,
    doc_order: Vec<u64>,
    k1: f32,
    b: f32,
}

/// BM25 keyword search index.
///
/// Supports:
/// - Porter stemming for English
/// - Synonym expansion (45+ dev term groups)
/// - BM25 ranking with configurable k1 and b parameters
pub struct Bm25Index {
    /// Inverted index: term -> posting list.
    index: HashMap<String, Vec<Posting>>,
    /// Document store: doc_id -> document metadata.
    documents: HashMap<u64, Bm25Document>,
    /// Total number of documents.
    doc_count: usize,
    /// Average document length.
    avg_doc_len: f32,
    /// Total length of all documents (for computing avg_doc_len).
    total_doc_len: usize,
    /// Insertion order for LRU eviction.
    doc_order: Vec<u64>,
    /// BM25 term frequency saturation parameter (MEM-21).
    k1: f32,
    /// BM25 document length normalization parameter (MEM-22).
    b: f32,
    /// Maximum documents before LRU eviction (MEM-23).
    max_documents: usize,
}

impl Bm25Index {
    pub fn new() -> Self {
        Self {
            index: HashMap::new(),
            documents: HashMap::new(),
            doc_count: 0,
            avg_doc_len: 0.0,
            total_doc_len: 0,
            doc_order: Vec::new(),
            k1: DEFAULT_BM25_K1,
            b: DEFAULT_BM25_B,
            max_documents: DEFAULT_MAX_BM25_DOCUMENTS,
        }
    }

    /// Creates a new BM25 index with custom parameters from MemoryConfig.
    pub fn with_config(k1: f32, b: f32, max_documents: usize) -> Self {
        Self {
            index: HashMap::new(),
            documents: HashMap::new(),
            doc_count: 0,
            avg_doc_len: 0.0,
            total_doc_len: 0,
            doc_order: Vec::new(),
            k1,
            b,
            max_documents,
        }
    }

    /// Adds a document to the index. Evicts the oldest document if at capacity.
    pub fn add_document(&mut self, doc_id: u64, content: &str) {
        // RC-07: Evict oldest document if at capacity
        if self.documents.len() >= self.max_documents && !self.documents.contains_key(&doc_id) {
            // Find the oldest document that is not the one being inserted
            while let Some(oldest_id) = self.doc_order.first().cloned() {
                if oldest_id != doc_id && self.documents.contains_key(&oldest_id) {
                    self.remove_document(oldest_id);
                    break;
                } else {
                    self.doc_order.remove(0);
                }
            }
        }
        let mut terms = tokenize(content);
        // Apply synonym expansion
        let expanded = expand_synonyms(&terms);
        terms.extend(expanded);
        // Apply stemming
        let stemmed: Vec<String> = terms.iter().map(|t| porter_stem(t)).collect();

        let doc_len = stemmed.len();
        let mut term_freqs: HashMap<String, u32> = HashMap::new();

        for term in &stemmed {
            *term_freqs.entry(term.clone()).or_insert(0) += 1;
        }

        // Add to inverted index
        for (term, freq) in &term_freqs {
            self.index.entry(term.clone()).or_default().push(Posting {
                doc_id,
                term_freq: *freq,
            });
        }

        // Store document
        let is_reinsert = self.documents.contains_key(&doc_id);
        if is_reinsert {
            // Remove old document's terms from the inverted index first
            if let Some(old_doc) = self.documents.get(&doc_id) {
                for term in &old_doc.terms {
                    if let Some(postings) = self.index.get_mut(term) {
                        postings.retain(|p| p.doc_id != doc_id);
                        if postings.is_empty() {
                            self.index.remove(term);
                        }
                    }
                }
                self.total_doc_len -= old_doc.doc_len;
            }
        }

        self.documents.insert(
            doc_id,
            Bm25Document {
                terms: stemmed,
                doc_len,
            },
        );

        if !is_reinsert {
            self.doc_order.push(doc_id);
            self.doc_count += 1;
        }
        self.total_doc_len += doc_len;
        self.avg_doc_len = if self.doc_count > 0 {
            self.total_doc_len as f32 / self.doc_count as f32
        } else {
            0.0
        };
    }

    /// Removes a document from the index.
    pub fn remove_document(&mut self, doc_id: u64) {
        if let Some(doc) = self.documents.remove(&doc_id) {
            // Remove from posting lists
            for term in &doc.terms {
                if let Some(postings) = self.index.get_mut(term) {
                    postings.retain(|p| p.doc_id != doc_id);
                    if postings.is_empty() {
                        self.index.remove(term);
                    }
                }
            }
            // Remove from insertion order
            self.doc_order.retain(|id| *id != doc_id);
            self.doc_count -= 1;
            self.total_doc_len -= doc.doc_len;
            if self.doc_count > 0 {
                self.avg_doc_len = self.total_doc_len as f32 / self.doc_count as f32;
            } else {
                self.avg_doc_len = 0.0;
            }
        }
    }

    /// Searches the index and returns (doc_id, score) pairs sorted by descending score.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(u64, f32)> {
        // Guard: empty index or zero avg doc length would cause division by zero
        if self.avg_doc_len == 0.0 || self.doc_count == 0 {
            return Vec::new();
        }

        let mut query_terms = tokenize(query);
        let expanded = expand_synonyms(&query_terms);
        query_terms.extend(expanded);
        let stemmed: Vec<String> = query_terms.iter().map(|t| porter_stem(t)).collect();

        let mut scores: HashMap<u64, f32> = HashMap::new();

        for term in &stemmed {
            if let Some(postings) = self.index.get(term) {
                let df = postings.len() as f32;
                let idf = ((self.doc_count as f32 - df + 0.5) / (df + 0.5) + 1.0).ln();

                for posting in postings {
                    let tf = posting.term_freq as f32;
                    let doc_len = self
                        .documents
                        .get(&posting.doc_id)
                        .map(|d| d.doc_len as f32)
                        .unwrap_or(0.0);

                    let score = idf * (tf * (self.k1 + 1.0))
                        / (tf + self.k1 * (1.0 - self.b + self.b * doc_len / self.avg_doc_len));

                    *scores.entry(posting.doc_id).or_insert(0.0) += score;
                }
            }
        }

        let mut results: Vec<(u64, f32)> = scores.into_iter().collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        results
    }

    /// Returns the number of indexed documents.
    pub fn doc_count(&self) -> usize {
        self.doc_count
    }

    /// Returns the number of unique terms in the index.
    pub fn term_count(&self) -> usize {
        self.index.len()
    }

    /// Serializes the BM25 state to JSON bytes for CortexaDB persistence.
    pub fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        let snapshot = Bm25Snapshot {
            inverted_index: self.index.clone(),
            documents: self.documents.clone(),
            doc_count: self.doc_count,
            avg_doc_len: self.avg_doc_len,
            total_doc_len: self.total_doc_len,
            doc_order: self.doc_order.clone(),
            k1: self.k1,
            b: self.b,
        };
        serde_json::to_vec(&snapshot).map_err(|e| e.to_string())
    }

    /// Restores BM25 state from JSON bytes (from CortexaDB).
    pub fn load_snapshot(bytes: &[u8]) -> Result<Self, String> {
        let snapshot: Bm25Snapshot = serde_json::from_slice(bytes)
            .map_err(|e| format!("BM25 snapshot deserialization failed: {}", e))?;
        Ok(Self {
            index: snapshot.inverted_index,
            documents: snapshot.documents,
            doc_count: snapshot.doc_count,
            avg_doc_len: snapshot.avg_doc_len,
            total_doc_len: snapshot.total_doc_len,
            doc_order: snapshot.doc_order,
            k1: snapshot.k1,
            b: snapshot.b,
            max_documents: DEFAULT_MAX_BM25_DOCUMENTS,
        })
    }
}

impl Default for Bm25Index {
    fn default() -> Self {
        Self::new()
    }
}

/// Tokenizes text into lowercase alphanumeric tokens.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty() && s.len() > 1)
        .map(|s| s.to_lowercase())
        .collect()
}

/// Suffix list for the Porter stemmer. Ordered longest-first to prefer
/// the most specific suffix match.
const STEM_SUFFIXES: &[(&str, usize)] = &[
    ("tion", 5),
    ("ness", 5),
    ("ment", 5),
    ("able", 5),
    ("ible", 5),
    ("less", 5),
    ("ally", 6),
    ("ing", 5),
    ("ize", 5),
    ("ise", 5),
    ("ous", 5),
    ("ive", 5),
    ("ful", 5),
    ("est", 5),
    ("ly", 4),
    ("ed", 4),
    ("er", 4),
    ("al", 4),
];

/// Porter stemmer — simplified implementation for English.
///
/// Handles common suffixes using a data-driven suffix table.
fn porter_stem(word: &str) -> String {
    let word = word.to_lowercase();
    if word.len() <= 3 {
        return word;
    }

    for &(suffix, min_len) in STEM_SUFFIXES {
        if word.ends_with(suffix) && word.len() > min_len {
            return word[..word.len() - suffix.len()].to_string();
        }
    }

    word
}

/// Expands tokens with synonyms from dev-term groups.
///
/// Returns additional tokens that should be added to the index/query.
fn expand_synonyms(tokens: &[String]) -> Vec<String> {
    let mut expanded = Vec::new();
    for token in tokens {
        if let Some(synonyms) = SYNONYM_MAP.get(token.as_str()) {
            for syn in *synonyms {
                expanded.push(syn.to_string());
            }
        }
    }
    expanded
}

/// Synonym map: 45+ dev-term groups for technical vocabulary expansion.
/// Uses `LazyLock` for thread-safe lazy initialization.
use std::sync::LazyLock;

static SYNONYM_MAP: LazyLock<HashMap<&'static str, &'static [&'static str]>> =
    LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert(
            "function",
            &["method", "procedure", "fn", "func"] as &[&str],
        );
        m.insert("method", &["function", "procedure", "fn"] as &[&str]);
        m.insert("variable", &["var", "binding", "let"] as &[&str]);
        m.insert("constant", &["const", "static", "final"] as &[&str]);
        m.insert("class", &["struct", "type", "object"] as &[&str]);
        m.insert("struct", &["class", "record", "type"] as &[&str]);
        m.insert("interface", &["trait", "protocol", "contract"] as &[&str]);
        m.insert("trait", &["interface", "protocol", "contract"] as &[&str]);
        m.insert("enum", &["enumeration", "union", "variant"] as &[&str]);
        m.insert("array", &["list", "vec", "vector", "sequence"] as &[&str]);
        m.insert(
            "map",
            &["dict", "hashmap", "dictionary", "table"] as &[&str],
        );
        m.insert("string", &["str", "text", "slice"] as &[&str]);
        m.insert(
            "error",
            &["err", "failure", "fault", "exception"] as &[&str],
        );
        m.insert("result", &["outcome", "response", "return"] as &[&str]);
        m.insert("option", &["optional", "maybe", "nullable"] as &[&str]);
        m.insert("null", &["nil", "none", "nothing"] as &[&str]);
        m.insert(
            "async",
            &["asynchronous", "concurrent", "non-blocking"] as &[&str],
        );
        m.insert(
            "sync",
            &["synchronous", "blocking", "sequential"] as &[&str],
        );
        m.insert("thread", &["process", "task", "worker"] as &[&str]);
        m.insert(
            "lock",
            &["mutex", "semaphore", "guard", "rwlock"] as &[&str],
        );
        m.insert(
            "channel",
            &["pipe", "queue", "sender", "receiver"] as &[&str],
        );
        m.insert("stream", &["iterator", "sequence", "flow"] as &[&str]);
        m.insert("buffer", &["cache", "pool", "queue"] as &[&str]);
        m.insert("file", &["path", "directory", "filesystem"] as &[&str]);
        m.insert(
            "network",
            &["socket", "connection", "endpoint", "tcp", "udp"] as &[&str],
        );
        m.insert("http", &["request", "response", "api", "rest"] as &[&str]);
        m.insert(
            "database",
            &["db", "store", "persistence", "storage"] as &[&str],
        );
        m.insert("query", &["search", "find", "lookup", "filter"] as &[&str]);
        m.insert("index", &["indexing", "catalog", "registry"] as &[&str]);
        m.insert("key", &["id", "identifier", "primary"] as &[&str]);
        m.insert("value", &["data", "payload", "content"] as &[&str]);
        m.insert(
            "config",
            &["configuration", "settings", "options"] as &[&str],
        );
        m.insert("log", &["logging", "trace", "debug"] as &[&str]);
        m.insert(
            "test",
            &["testing", "spec", "assertion", "check"] as &[&str],
        );
        m.insert("build", &["compile", "make", "assemble"] as &[&str]);
        m.insert("deploy", &["release", "publish", "ship"] as &[&str]);
        m.insert(
            "security",
            &["auth", "authentication", "authorization", "crypto"] as &[&str],
        );
        m.insert("memory", &["heap", "stack", "allocation", "gc"] as &[&str]);
        m.insert(
            "performance",
            &["latency", "throughput", "benchmark", "profile"] as &[&str],
        );
        m.insert("crash", &["panic", "fault", "segfault", "abort"] as &[&str]);
        m.insert("bug", &["defect", "issue", "regression", "fix"] as &[&str]);
        m.insert(
            "feature",
            &["capability", "functionality", "enhancement"] as &[&str],
        );
        m.insert(
            "refactor",
            &["restructure", "reorganize", "rewrite"] as &[&str],
        );
        m.insert("api", &["interface", "endpoint", "surface"] as &[&str]);
        m.insert("version", &["release", "tag", "semver"] as &[&str]);
        m.insert(
            "dependency",
            &["dep", "import", "require", "crate"] as &[&str],
        );
        m.insert("module", &["package", "crate", "library"] as &[&str]);
        m.insert("binary", &["executable", "bin", "program"] as &[&str]);
        m
    });

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("Hello World! This is a test.");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        assert!(tokens.contains(&"test".to_string()));
        assert!(!tokens.contains(&"a".to_string())); // too short
    }

    #[test]
    fn test_porter_stem_basic() {
        assert_eq!(porter_stem("running"), "runn");
        assert_eq!(porter_stem("testing"), "test");
        assert_eq!(porter_stem("connection"), "connec");
        assert_eq!(porter_stem("happiness"), "happi");
        assert_eq!(porter_stem("beautiful"), "beauti");
        assert_eq!(porter_stem("quickly"), "quick");
    }

    #[test]
    fn test_porter_stem_short_word() {
        assert_eq!(porter_stem("go"), "go");
        assert_eq!(porter_stem("is"), "is");
    }

    #[test]
    fn test_bm25_basic_search() {
        let mut index = Bm25Index::new();
        index.add_document(1, "the quick brown fox");
        index.add_document(2, "the lazy brown dog");
        index.add_document(3, "the quick brown dog");

        let results = index.search("quick fox", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, 1); // doc 1 should be top result
    }

    #[test]
    fn test_bm25_no_results() {
        let mut index = Bm25Index::new();
        index.add_document(1, "hello world");

        let results = index.search("nonexistent", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_bm25_remove_document() {
        let mut index = Bm25Index::new();
        index.add_document(1, "hello world");
        index.add_document(2, "hello rust");
        assert_eq!(index.doc_count(), 2);

        index.remove_document(1);
        assert_eq!(index.doc_count(), 1);

        let results = index.search("hello", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 2);
    }

    #[test]
    fn test_bm25_synonym_expansion() {
        let mut index = Bm25Index::new();
        index.add_document(1, "the function returns a result");
        index.add_document(2, "the method returns an outcome");

        // Searching for "function" should also match "method" via synonyms
        let results = index.search("function", 10);
        assert_eq!(results.len(), 2); // both docs should match
    }

    #[test]
    fn test_bm25_limit_results() {
        let mut index = Bm25Index::new();
        for i in 0..100 {
            index.add_document(i, &format!("document {} about testing", i));
        }

        let results = index.search("testing", 10);
        assert!(results.len() <= 10);
    }

    #[test]
    fn test_bm25_doc_count_and_term_count() {
        let mut index = Bm25Index::new();
        index.add_document(1, "hello world");
        index.add_document(2, "foo bar baz");

        assert_eq!(index.doc_count(), 2);
        assert!(index.term_count() > 0);
    }
}
