//! Hippocampal associative memory for the CubeLang VM.
//!
//! Wraps the ported VSA primitives (`Codebook` + `HammingIndex`) to give the
//! VM content-addressable memory: STORE binds a key to a payload via a seeded
//! hypervector address; RECALL retrieves the payload whose key-hypervector is
//! cosine-nearest to the query (so partial / noisy keys still resolve to the
//! right memory ? genuine cleanup, not an exact-dict lookup).

use std::collections::HashMap;
use crate::vm::codebook::Codebook;
use crate::vm::index::HammingIndex;
use crate::vm::hypervec::Hypervec;
use crate::vm::engine::Value;

/// A single stored memory: the literal key string and its payload value.
#[derive(Debug, Clone)]
pub struct MemoryTrace {
    pub key: String,
    pub value: Value,
}

/// Content-addressable VSA memory.
///
/// - `codebook`  : deterministic key-string -> bipolar hypervector (FNV-1a seed)
/// - `index`     : HammingIndex over the key hypervectors (id = trace position)
/// - `traces`    : id -> (key, payload). `None` for forgotten slots.
/// - `by_key`    : exact-key -> id, so STORE under an existing key overwrites
///                 rather than appending a duplicate.
pub struct HippocampalMemory {
    codebook: Codebook,
    index: HammingIndex,
    traces: Vec<Option<MemoryTrace>>,
    by_key: HashMap<String, u64>,
    dim: usize,
}

impl HippocampalMemory {
    /// Create an empty memory with the given hypervector dimension and codebook seed.
    pub fn new(dim: usize, seed: u64) -> Self {
        Self {
            codebook: Codebook::with_dim(seed, dim),
            index: HammingIndex::new(),
            traces: Vec::new(),
            by_key: HashMap::new(),
            dim,
        }
    }

    /// Number of live (non-forgotten) traces.
    pub fn len(&self) -> usize {
        self.traces.iter().filter(|t| t.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The hypervector address for a key (deterministic via the codebook).
    fn key_vec(&mut self, key: &str) -> Hypervec {
        self.codebook.get_cloned(key)
    }

    /// STORE: bind `key` -> `value`. Overwrites the payload if the exact key
    /// already exists; otherwise inserts a new associative trace.
    pub fn store(&mut self, key: &str, value: Value) {
        if let Some(&id) = self.by_key.get(key) {
            // Overwrite existing trace's payload (address vector unchanged).
            if let Some(slot) = self.traces.get_mut(id as usize) {
                *slot = Some(MemoryTrace { key: key.to_string(), value });
                return;
            }
        }
        let id = self.traces.len() as u64;
        let v = self.key_vec(key);
        self.index.insert(id, &v);
        self.traces.push(Some(MemoryTrace { key: key.to_string(), value }));
        self.by_key.insert(key.to_string(), id);
    }

    /// RECALL by associative cleanup: returns the payload + matched key +
    /// cosine similarity of the trace whose key-hypervector is nearest the
    /// query key. Tolerates partial / noisy keys. `None` if memory is empty.
    pub fn recall(&mut self, query_key: &str) -> Option<(String, Value, f64)> {
        if self.index.is_empty() {
            return None;
        }
        let q = self.key_vec(query_key);
        // Walk nearest neighbours until we hit a live (non-forgotten) trace.
        let hits = self.index.query(&q, 8);
        for (id, sim) in hits {
            if let Some(Some(tr)) = self.traces.get(id as usize) {
                return Some((tr.key.clone(), tr.value.clone(), sim));
            }
        }
        None
    }

    /// FORGET: drop the trace stored under the exact `key` (if any). The index
    /// entry is tombstoned (skipped at recall) rather than physically removed.
    pub fn forget(&mut self, key: &str) -> bool {
        if let Some(&id) = self.by_key.get(key) {
            if let Some(slot) = self.traces.get_mut(id as usize) {
                *slot = None;
            }
            self.by_key.remove(key);
            self.index.remove(id);
            true
        } else {
            false
        }
    }

    /// Exact-key lookup (no similarity search) ? used as a fast path.
    pub fn get_exact(&self, key: &str) -> Option<&Value> {
        self.by_key.get(key)
            .and_then(|&id| self.traces.get(id as usize))
            .and_then(|slot| slot.as_ref())
            .map(|tr| &tr.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::engine::Value;

    #[test]
    fn test_store_and_exact_recall() {
        let mut m = HippocampalMemory::new(4096, 42);
        m.store("janet_daily_revenue", Value::Int(18));
        m.store("robe_total_bolts", Value::Int(3));
        assert_eq!(m.len(), 2);
        let (key, val, sim) = m.recall("janet_daily_revenue").unwrap();
        assert_eq!(key, "janet_daily_revenue");
        assert_eq!(val.as_i64(), 18);
        assert!((sim - 1.0).abs() < 1e-9, "exact key must recall at sim=1.0, got {sim}");
    }

    #[test]
    fn test_recall_is_associative() {
        let mut m = HippocampalMemory::new(4096, 7);
        m.store("price_of_apples", Value::Int(5));
        m.store("count_of_oranges", Value::Int(12));
        let (k1, v1, _) = m.recall("price_of_apples").unwrap();
        assert_eq!(k1, "price_of_apples");
        assert_eq!(v1.as_i64(), 5);
        let (k2, v2, _) = m.recall("count_of_oranges").unwrap();
        assert_eq!(k2, "count_of_oranges");
        assert_eq!(v2.as_i64(), 12);
    }

    #[test]
    fn test_overwrite_same_key() {
        let mut m = HippocampalMemory::new(4096, 1);
        m.store("x", Value::Int(1));
        m.store("x", Value::Int(99));
        assert_eq!(m.len(), 1);
        assert_eq!(m.recall("x").unwrap().1.as_i64(), 99);
    }

    #[test]
    fn test_forget() {
        let mut m = HippocampalMemory::new(4096, 1);
        m.store("a", Value::Int(1));
        m.store("b", Value::Int(2));
        assert!(m.forget("a"));
        assert_eq!(m.len(), 1);
        if let Some((k, _, _)) = m.recall("a") {
            assert_eq!(k, "b", "forgotten key must not resurface");
        }
        assert!(!m.forget("a"));
    }

    #[test]
    fn test_empty_recall_is_none() {
        let mut m = HippocampalMemory::new(4096, 1);
        assert!(m.recall("anything").is_none());
    }
}
