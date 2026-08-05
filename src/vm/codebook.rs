//! Symbol codebook: deterministic mapping from symbol names to bipolar hypervectors.
//!
//! A `Codebook` acts as the "random projector" for the VSA: each distinct symbol
//! (opcode mnemonic, register name, abstract operand class, role name, …) maps to a
//! unique quasi-orthogonal hypervector. The same symbol always maps to the same
//! vector as long as the seed is the same (determinism).
//!
//! ## Symbol derivation
//! We derive a stable per-symbol seed by hashing the symbol string together with
//! the global seed, then generate the vector. This avoids storing a growing table
//! in situations where the symbol set is fixed but large (e.g., all x86 mnemonics).
//! For flexibility the codebook also has a runtime cache so repeated lookups are O(1).

use std::collections::HashMap;
use crate::vm::hypervec::{Hypervec, DEFAULT_DIM};
use rand::rngs::SmallRng;
use rand::SeedableRng;

/// A deterministic, seeded symbol → hypervector codebook.
///
/// Built on top of a 64-bit seed. Any string key is hashed to a reproducible
/// per-symbol seed using a simple FNV-1a mix.
#[derive(Debug)]
pub struct Codebook {
    /// Global seed mixed into per-symbol seeds.
    pub(crate) seed: u64,
    /// Hypervector dimension.
    pub(crate) dim: usize,
    /// Cached vectors.
    pub(crate) cache: HashMap<String, Hypervec>,
}

impl Codebook {
    /// Create a codebook with the given seed and default dimension (8192,
    /// `hypervec::DEFAULT_DIM`).
    pub fn new(seed: u64) -> Self {
        Self { seed, dim: DEFAULT_DIM, cache: HashMap::new() }
    }

    /// Create a codebook with a custom dimension.
    pub fn with_dim(seed: u64, dim: usize) -> Self {
        assert!(dim > 0 && dim % 2 == 0, "dim must be a positive even number");
        Self { seed, dim, cache: HashMap::new() }
    }

    /// Look up (or generate) the hypervector for `symbol`.
    pub fn get(&mut self, symbol: &str) -> &Hypervec {
        let seed = self.seed;
        let dim = self.dim;
        self.cache.entry(symbol.to_string()).or_insert_with(|| {
            let sym_seed = fnv_mix(seed, symbol);
            let mut rng = SmallRng::seed_from_u64(sym_seed);
            Hypervec::random_with_rng(dim, &mut rng)
        })
    }

    /// Return a clone of the vector for a symbol (useful when borrowing issues arise).
    pub fn get_cloned(&mut self, symbol: &str) -> Hypervec {
        self.get(symbol).clone()
    }

    /// The dimension this codebook generates vectors at.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// How many symbols are cached.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Is the cache empty?
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Insert a pre-computed hypervector under `key`, bypassing the FNV seed derivation.
    ///
    /// This is used to pin the eight
    /// universal-role vectors to their fixed seeds from the Python `_ROLE_SEEDS` dict.
    pub fn insert_precomputed(&mut self, key: impl Into<String>, vec: Hypervec) {
        self.cache.insert(key.into(), vec);
    }
}

/// FNV-1a inspired mix: combine a 64-bit base seed with a string to produce a
/// stable per-symbol seed.
fn fnv_mix(base_seed: u64, s: &str) -> u64 {
    const FNV_PRIME: u64 = 0x100000001b3;
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    let mut h = FNV_OFFSET.wrapping_add(base_seed.wrapping_mul(FNV_PRIME));
    for byte in s.bytes() {
        h ^= byte as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determinism() {
        let mut cb1 = Codebook::new(42);
        let mut cb2 = Codebook::new(42);
        let v1 = cb1.get_cloned("add");
        let v2 = cb2.get_cloned("add");
        assert_eq!(v1, v2, "same seed + symbol must produce same vector");
    }

    #[test]
    fn test_different_symbols_orthogonal() {
        let mut cb = Codebook::new(42);
        let add = cb.get_cloned("add");
        let sub = cb.get_cloned("sub");
        let mul = cb.get_cloned("mul");
        let sim_as = add.cosine_sim(&sub);
        let sim_am = add.cosine_sim(&mul);
        assert!(sim_as.abs() < 0.1, "add vs sub should be near-orthogonal: {sim_as:.4}");
        assert!(sim_am.abs() < 0.1, "add vs mul should be near-orthogonal: {sim_am:.4}");
    }

    #[test]
    fn test_different_seeds_give_different_vectors() {
        let mut cb1 = Codebook::new(1);
        let mut cb2 = Codebook::new(2);
        let v1 = cb1.get_cloned("add");
        let v2 = cb2.get_cloned("add");
        assert_ne!(v1, v2, "different seeds should give different vectors");
    }

    #[test]
    fn test_cache_consistency() {
        let mut cb = Codebook::new(7);
        let v1 = cb.get_cloned("mov");
        let v2 = cb.get_cloned("mov");
        assert_eq!(v1, v2, "same key must return same vector");
    }

    #[test]
    fn test_many_symbols_quasi_orthogonal() {
        let mnemonics = ["add","sub","mul","div","load","store","jump","call","ret",
                         "cmp","and","or","xor","not","shl","shr","push","pop"];
        let mut cb = Codebook::new(123);
        let vecs: Vec<Hypervec> = mnemonics.iter().map(|s| cb.get_cloned(s)).collect();
        for i in 0..vecs.len() {
            for j in (i+1)..vecs.len() {
                let sim = vecs[i].cosine_sim(&vecs[j]);
                assert!(sim.abs() < 0.15, 
                    "mnemonics {} vs {} not quasi-orthogonal: {sim:.4}",
                    mnemonics[i], mnemonics[j]);
            }
        }
    }
}
