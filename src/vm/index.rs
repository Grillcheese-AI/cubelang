//! Retrieval indices for bipolar hypervectors.
//!
//! Two backends are provided:
//!
//! ## [`HammingIndex`] — exact bit-packed linear scan
//!
//! Stores hypervectors as packed u64 word arrays (bit 1 = +1, bit 0 = -1).
//! Distance computation is XOR + `count_ones()` over 64-element word groups,
//! giving a ~64× reduction in memory bandwidth vs byte-level iteration.
//!
//! Suitable for collections up to a few thousand vectors and/or offline
//! (non-latency-critical) lookups.
//!
//! ```ignore
//! use opcode_vsa_rs::index::HammingIndex;
//! use opcode_vsa_rs::Hypervec;
//!
//! let mut idx = HammingIndex::new();
//! let a = Hypervec::random_seeded(4096, 1);
//! idx.insert(0, &a);
//! let hits = idx.query(&a, 1);
//! assert_eq!(hits[0].0, 0);
//! ```
//!
//! ## [`LshIndex`] — multi-table LSH for approximate nearest-neighbour search
//!
//! Uses `L` independent hash tables, each built from `K` random bipolar
//! projection vectors.  A query hashes into all `L` tables and collects
//! candidates; the exact Hamming distance is then re-computed on the small
//! candidate set.
//!
//! Provides sub-linear query time for large collections with high recall for
//! vectors within moderate Hamming distance.
//!
//! ```ignore
//! use opcode_vsa_rs::index::LshIndex;
//! use opcode_vsa_rs::Hypervec;
//!
//! let mut idx = LshIndex::new(4096, 8, 10, 42);  // dim, K, L, seed
//! let a = Hypervec::random_seeded(4096, 1);
//! idx.insert(0, &a);
//! let hits = idx.query(&a, 1);
//! assert_eq!(hits[0].0, 0);
//! ```

use crate::vm::hypervec::{pack_bits, hamming_dist_packed, Hypervec, PackedHypervec};

/// Scalar Hamming distance over packed u64 words. (Ported without the SIMD
/// dispatcher from opcode-vsa-rs — the scalar path auto-vectorises under -O3.)
#[inline]
fn hamming_dist_words(a: &[u64], b: &[u64]) -> u32 {
    hamming_dist_packed(a, b)
}
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

// ---------------------------------------------------------------------------
// QueryResult — a (id, cosine_similarity) pair
// ---------------------------------------------------------------------------

/// A retrieval result: (item id, cosine similarity ∈ [−1, +1]).
pub type QueryResult = (u64, f64);

// ---------------------------------------------------------------------------
// HammingIndex — exact linear scan over bit-packed vectors
// ---------------------------------------------------------------------------

/// Exact nearest-neighbour index for bipolar hypervectors using bit-packed
/// Hamming distance (XOR + popcount over u64 words).
///
/// **Complexity**: O(N·D/64) per query, O(N·D/64) storage.
/// **Accuracy**: exact — no false negatives.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HammingIndex {
    /// Stored entries: (id, packed_words).
    entries: Vec<(u64, PackedHypervec)>,
    /// Dimensionality of stored vectors.
    dim: usize,
}

impl HammingIndex {
    /// Create an empty index (dimension inferred from the first insertion).
    pub fn new() -> Self {
        Self { entries: Vec::new(), dim: 0 }
    }

    /// Create an index pre-allocated for `capacity` vectors.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { entries: Vec::with_capacity(capacity), dim: 0 }
    }

    /// Number of vectors in the index.
    pub fn len(&self) -> usize { self.entries.len() }

    /// True if the index is empty.
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Dimensionality of stored vectors (0 if empty).
    pub fn dim(&self) -> usize { self.dim }

    /// Insert a hypervector with the given `id`.
    ///
    /// Panics if the vector's dimension is inconsistent with previously
    /// inserted vectors.
    pub fn insert(&mut self, id: u64, vec: &Hypervec) {
        if self.dim == 0 {
            self.dim = vec.dim();
        } else {
            assert_eq!(vec.dim(), self.dim,
                "HammingIndex dimension mismatch: expected {}, got {}", self.dim, vec.dim());
        }
        self.entries.push((id, vec.pack()));
    }

    /// Remove all entries with the given id (may be O(N)).
    pub fn remove(&mut self, id: u64) {
        self.entries.retain(|(i, _)| *i != id);
    }

    /// Query for the `k` nearest neighbours by Hamming distance.
    ///
    /// Returns up to `k` results as `(id, cosine_sim)` pairs sorted by
    /// **decreasing** similarity.
    pub fn query(&self, query: &Hypervec, k: usize) -> Vec<QueryResult> {
        if self.entries.is_empty() { return Vec::new(); }
        assert_eq!(query.dim(), self.dim,
            "query dimension {} != index dimension {}", query.dim(), self.dim);
        let qpacked = query.pack();
        let d = self.dim as f64;

        // Compute distances and collect — uses SIMD dispatcher when simd feature enabled
        let mut scored: Vec<(u64, u32)> = self.entries.iter()
            .map(|(id, pv)| (*id, hamming_dist_words(&qpacked.words, &pv.words)))
            .collect();

        // Partial sort: only need top k (smallest distance = highest similarity)
        scored.sort_unstable_by_key(|&(_, dist)| dist);

        scored.into_iter()
            .take(k)
            .map(|(id, dist)| {
                let sim = (d - 2.0 * dist as f64) / d;
                (id, sim)
            })
            .collect()
    }

    /// Query returning the single nearest neighbour (convenience wrapper).
    pub fn query_one(&self, query: &Hypervec) -> Option<QueryResult> {
        self.query(query, 1).into_iter().next()
    }

    // -----------------------------------------------------------------------
    // Serialization helpers
    // -----------------------------------------------------------------------

    /// Save index to a JSON file.
    pub fn save_json<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let json = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut f = std::fs::File::create(path)?;
        f.write_all(json.as_bytes())
    }

    /// Load index from a JSON file.
    pub fn load_json<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;
        serde_json::from_str(&buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Save index to a binary (bincode) file.
    pub fn save_bin<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let bytes = bincode::serialize(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut f = std::fs::File::create(path)?;
        f.write_all(&bytes)
    }

    /// Load index from a binary (bincode) file.
    pub fn load_bin<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        bincode::deserialize(&buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

impl Default for HammingIndex {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// LshIndex — multi-table approximate nearest-neighbour search
// ---------------------------------------------------------------------------

/// Multi-table locality-sensitive hashing index for bipolar hypervectors.
///
/// ## Algorithm
///
/// Offline (build phase):
/// 1. Generate `L` hash tables, each with `K` random bipolar projection vectors
///    drawn from the codebook seed.
/// 2. For each inserted vector `v`, compute hash `h_t(v) = sign(P_t · v)`
///    reduced to a `K`-bit integer, and store `v` in bucket `h_t` of table `t`.
///
/// Online (query phase):
/// 1. Hash the query into all `L` tables and collect the union of all matching
///    bucket entries as candidates.
/// 2. Re-score candidates by exact Hamming distance and return the top-k.
///
/// ## Parameters
///
/// - `K` — number of bits per hash (more bits → fewer collisions, faster but
///   lower recall for distant vectors).  Typical range: 8–20.
/// - `L` — number of tables (more tables → higher recall, more memory).
///   Typical range: 4–16.
/// - `seed` — PRNG seed for reproducible projection generation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LshIndex {
    /// Dimension of stored vectors.
    dim: usize,
    /// Number of hash bits per table.
    k: usize,
    /// Number of hash tables.
    l: usize,
    /// Packed projection vectors, grouped by table: `projections[t][b]` is the
    /// b-th projection for table t (stored as packed u64 words).
    projections: Vec<Vec<Vec<u64>>>,  // [table][bit][word]
    /// Hash tables: `tables[t]` maps a K-bit hash bucket → list of entry indices.
    tables: Vec<HashMap<u64, Vec<usize>>>,
    /// All stored entries: (id, packed_words).
    entries: Vec<(u64, Vec<u64>)>,
}

impl LshIndex {
    /// Create a new LSH index.
    ///
    /// - `dim`  — hypervector dimension
    /// - `k`    — hash bits per table (4–20 is typical)
    /// - `l`    — number of tables (4–16 is typical)
    /// - `seed` — PRNG seed for projection generation
    pub fn new(dim: usize, k: usize, l: usize, seed: u64) -> Self {
        assert!(k <= 63, "k must be ≤ 63 (fits in u64 hash key)");
        assert!(l > 0 && k > 0 && dim > 0);

        // Generate L*K projection vectors deterministically
        let n_words = (dim + 63) / 64;
        let mut projections = Vec::with_capacity(l);
        for t in 0..l {
            let mut table_proj = Vec::with_capacity(k);
            for b in 0..k {
                // Use a deterministic seed per (table, bit): mix seed with table/bit indices
                let proj_seed = seed
                    .wrapping_add((t as u64).wrapping_mul(0x9e3779b97f4a7c15))
                    .wrapping_add((b as u64).wrapping_mul(0x6c62272e07bb0142))
                    .wrapping_add(0xb5a4bcae5f6a3c1d);
                let pv = Hypervec::random_seeded(dim, proj_seed);
                let packed = pack_bits(pv.as_slice());
                assert_eq!(packed.len(), n_words);
                table_proj.push(packed);
            }
            projections.push(table_proj);
        }

        let tables = vec![HashMap::new(); l];
        Self { dim, k, l, projections, tables, entries: Vec::new() }
    }

    /// Number of vectors in the index.
    pub fn len(&self) -> usize { self.entries.len() }

    /// True if the index is empty.
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Dimension of stored vectors.
    pub fn dim(&self) -> usize { self.dim }

    /// Number of hash tables.
    pub fn num_tables(&self) -> usize { self.l }

    /// Hash bits per table.
    pub fn hash_bits(&self) -> usize { self.k }

    /// Compute the K-bit LSH bucket key for vector `packed_words` in table `t`.
    ///
    /// Each bit b of the key is the sign of dot(projection[t][b], v):
    ///   sign = popcount(NOT XOR(p,v)) > D/2  ↔  agreement > half the bits
    ///        equivalently: popcount(XNOR) > D/2
    ///        equivalently: hamming_dist(p,v) < D/2
    fn hash_key(&self, t: usize, packed: &[u64]) -> u64 {
        let half = self.dim / 2;
        let mut key: u64 = 0;
        for b in 0..self.k {
            // Use SIMD dispatcher (avx2/sse2/scalar) for projection distance
            let dist = hamming_dist_words(&self.projections[t][b], packed) as usize;
            if dist < half {
                key |= 1u64 << b;
            }
        }
        key
    }

    /// Insert a hypervector with the given `id`.
    ///
    /// Panics if the vector's dimension is inconsistent.
    pub fn insert(&mut self, id: u64, vec: &Hypervec) {
        assert_eq!(vec.dim(), self.dim,
            "LshIndex dimension mismatch: expected {}, got {}", self.dim, vec.dim());
        let packed = pack_bits(vec.as_slice());
        let entry_idx = self.entries.len();
        // Add to all L hash tables
        for t in 0..self.l {
            let key = self.hash_key(t, &packed);
            self.tables[t].entry(key).or_default().push(entry_idx);
        }
        self.entries.push((id, packed));
    }

    /// Query for approximate `k` nearest neighbours.
    ///
    /// Collects candidates from all `L` tables, deduplicates, re-scores by
    /// exact Hamming distance, and returns the top-k sorted by **decreasing**
    /// similarity.
    pub fn query(&self, query: &Hypervec, k: usize) -> Vec<QueryResult> {
        if self.entries.is_empty() { return Vec::new(); }
        assert_eq!(query.dim(), self.dim,
            "query dimension {} != index dimension {}", query.dim(), self.dim);
        let qpacked = pack_bits(query.as_slice());
        let d = self.dim as f64;

        // Collect candidate entry indices from all tables
        let mut candidate_indices: Vec<usize> = Vec::new();
        for t in 0..self.l {
            let key = self.hash_key(t, &qpacked);
            if let Some(bucket) = self.tables[t].get(&key) {
                candidate_indices.extend_from_slice(bucket);
            }
        }

        // Deduplicate
        candidate_indices.sort_unstable();
        candidate_indices.dedup();

        if candidate_indices.is_empty() {
            // Fallback: if no candidates found (unlikely with good params),
            // fall back to a small linear scan of up to 100 random entries
            let n = self.entries.len().min(100);
            candidate_indices = (0..n).collect();
        }

        // Re-score candidates by exact Hamming distance (SIMD dispatcher)
        let mut scored: Vec<(u64, u32)> = candidate_indices.iter()
            .map(|&idx| {
                let (id, ref packed) = self.entries[idx];
                let dist = hamming_dist_words(&qpacked, packed);
                (id, dist)
            })
            .collect();

        scored.sort_unstable_by_key(|&(_, dist)| dist);
        scored.dedup_by_key(|&mut (id, _)| id);

        scored.into_iter()
            .take(k)
            .map(|(id, dist)| {
                let sim = (d - 2.0 * dist as f64) / d;
                (id, sim)
            })
            .collect()
    }

    /// Query returning the single nearest neighbour (convenience wrapper).
    pub fn query_one(&self, query: &Hypervec) -> Option<QueryResult> {
        self.query(query, 1).into_iter().next()
    }

    // -----------------------------------------------------------------------
    // Serialization helpers
    // -----------------------------------------------------------------------

    /// Save index to a JSON file.
    pub fn save_json<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let json = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut f = std::fs::File::create(path)?;
        f.write_all(json.as_bytes())
    }

    /// Load index from a JSON file.
    pub fn load_json<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;
        serde_json::from_str(&buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Save index to a binary (bincode) file.
    pub fn save_bin<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let bytes = bincode::serialize(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut f = std::fs::File::create(path)?;
        f.write_all(&bytes)
    }

    /// Load index from a binary (bincode) file.
    pub fn load_bin<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        bincode::deserialize(&buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

// ---------------------------------------------------------------------------
// Tests

