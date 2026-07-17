//! Core bipolar hypervector type and MAP-Bipolar VSA operations.
//!
//! Hypervectors live in {-1, +1}^D. All algebraic operations follow the
//! MAP-Bipolar (Multiply-Add-Permute) VSA family:
//!
//! - **Binding** (⊗): element-wise Hadamard product (×), result is quasi-orthogonal to inputs.
//! - **Bundling** (⊕): element-wise integer sum followed by sign/rebinarize, result is
//!   similar to each input.
//! - **Permute** (ρ): left cyclic shift by k positions, used to encode position/order.
//! - **Cosine similarity**: dot product / D, ranges in [−1, +1].
//!
//! ## Vectorization
//!
//! Bit-packed (u64 words) representations are used for the Hamming distance,
//! cosine similarity, and bundling hot paths.  Encoding: +1 → bit 1, -1 → bit 0.
//!
//! For dimension D and u64 words W = ceil(D/64):
//! - Hamming distance  = popcount(XOR(a_packed, b_packed))
//! - Cosine similarity = (D - 2 * hamming_dist) / D
//! - Bundling uses a i16 accumulator per element with LLVM auto-vectorization.

use rand::{Rng, SeedableRng};
use rand::rngs::SmallRng;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;

/// Default hypervector dimension. 8192 gives ~1229 programs PER CATEGORY slot.
/// With 9+ category slots, total system capacity is ~11K+ programs.
pub const DEFAULT_DIM: usize = 8192;

/// A bipolar hypervector with elements in {-1, +1}.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hypervec {
    /// Raw i8 storage; each element is −1 or +1.
    pub(crate) data: Vec<i8>,
}

// ---------------------------------------------------------------------------
// Bit-packed helpers (u64 words, +1→1 bit, -1→0 bit)
// ---------------------------------------------------------------------------

/// Pack a bipolar i8 slice into u64 words (+1 → bit 1, -1 → bit 0).
/// The returned Vec<u64> has ceil(len/64) words; unused high bits in the last word are 0.
#[inline]
pub fn pack_bits(data: &[i8]) -> Vec<u64> {
    let n_words = (data.len() + 63) / 64;
    let mut words = vec![0u64; n_words];
    for (i, &x) in data.iter().enumerate() {
        if x > 0 {
            words[i / 64] |= 1u64 << (i % 64);
        }
    }
    words
}

/// Compute Hamming distance between two bit-packed word slices.
/// Assumes both have the same length (same underlying dimension).
#[inline]
pub fn hamming_dist_packed(a: &[u64], b: &[u64]) -> u32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter().zip(b.iter()).map(|(&wa, &wb)| (wa ^ wb).count_ones()).sum()
}

impl Hypervec {
    /// Create a hypervector from raw data (each element must be -1 or +1).
    pub fn from_raw(data: Vec<i8>) -> Self {
        debug_assert!(data.iter().all(|&x| x == -1 || x == 1));
        Self { data }
    }

    /// Create a random bipolar hypervector of `dim` dimensions using a seeded RNG.
    pub fn random_seeded(dim: usize, seed: u64) -> Self {
        let mut rng = SmallRng::seed_from_u64(seed);
        Self::random_with_rng(dim, &mut rng)
    }

    /// Create a random bipolar hypervector using the provided RNG.
    pub fn random_with_rng<R: Rng>(dim: usize, rng: &mut R) -> Self {
        let data: Vec<i8> = (0..dim).map(|_| if rng.gen_bool(0.5) { 1i8 } else { -1i8 }).collect();
        Self { data }
    }

    /// Create the zero/neutral hypervector (all +1, identity under bundling with sign).
    pub fn ones(dim: usize) -> Self {
        Self { data: vec![1i8; dim] }
    }

    /// Create a zero-initialized accumulator vector.
    ///
    /// Unlike `from_raw`, this does **not** require bipolar (±1) values —
    /// a zero accumulator is used to collect bundle sums before re-signing.
    pub(crate) fn zeroed(dim: usize) -> Self {
        Self { data: vec![0i8; dim] }
    }

    /// Dimension (number of components).
    pub fn dim(&self) -> usize {
        self.data.len()
    }

    // -------------------------------------------------------------------------
    // Core VSA operations
    // -------------------------------------------------------------------------

    /// **Binding** (⊗): element-wise Hadamard product.
    ///
    /// For bipolar {-1,+1} vectors, multiplication reduces to XOR on the
    /// packed bit representation: bit_result = bit_a XOR bit_b XOR 1
    /// (because 1*1=1, (-1)*(-1)=1, but 1*(-1)=-1).
    /// We implement it directly on i8 for simplicity (bind is O(D)).
    ///
    /// Properties:
    /// - Self-inverse: a ⊗ a = 1 (ones vector)
    /// - a ⊗ b is quasi-orthogonal to both a and b
    /// - Commutative and associative
    pub fn bind(&self, other: &Hypervec) -> Hypervec {
        assert_eq!(self.dim(), other.dim(), "dimension mismatch in bind");
        let data: Vec<i8> = self.data.iter().zip(other.data.iter()).map(|(&a, &b)| a * b).collect();
        Hypervec { data }
    }

    /// **In-place binding**.
    pub fn bind_assign(&mut self, other: &Hypervec) {
        assert_eq!(self.dim(), other.dim(), "dimension mismatch in bind_assign");
        for (a, &b) in self.data.iter_mut().zip(other.data.iter()) {
            *a *= b;
        }
    }

    /// **Unbinding**: since bind is its own inverse, unbind(a ⊗ b, a) = b.
    pub fn unbind(&self, key: &Hypervec) -> Hypervec {
        self.bind(key)
    }

    /// **Bundling** (⊕): element-wise integer sum of multiple hypervectors, then sign-binarize.
    ///
    /// Returns a hypervector where sign(sum[i]) gives the majority vote.
    /// Ties (sum = 0) are broken by +1.
    ///
    /// Uses i16 accumulators for better LLVM auto-vectorization (avoids i32
    /// overhead while safely handling up to 128 input vectors).  For >128
    /// inputs we flush to i32 every 128 vectors.
    pub fn bundle(vecs: &[&Hypervec]) -> Hypervec {
        assert!(!vecs.is_empty(), "cannot bundle empty slice");
        let dim = vecs[0].dim();
        assert!(vecs.iter().all(|v| v.dim() == dim), "dimension mismatch in bundle");

        // Chunked accumulation: i16 is fine for up to 127 vectors before overflow
        // (127 * 1 = 127 < 32767). Flush to i32 every CHUNK vectors.
        const CHUNK: usize = 64;
        let mut acc: Vec<i32> = vec![0i32; dim];
        let mut tmp: Vec<i16> = vec![0i16; dim];

        for chunk in vecs.chunks(CHUNK) {
            // Reset tmp
            for t in tmp.iter_mut() { *t = 0; }
            for v in chunk {
                for (t, &x) in tmp.iter_mut().zip(v.data.iter()) {
                    *t += x as i16;
                }
            }
            for (a, &t) in acc.iter_mut().zip(tmp.iter()) {
                *a += t as i32;
            }
        }
        Self::sign_from_sum(acc)
    }

    /// Bundle two hypervectors (convenience overload).
    pub fn bundle2(a: &Hypervec, b: &Hypervec) -> Hypervec {
        Self::bundle(&[a, b])
    }

    /// Bundle from owned vectors (convenience).
    pub fn bundle_owned(vecs: &[Hypervec]) -> Hypervec {
        let refs: Vec<&Hypervec> = vecs.iter().collect();
        Self::bundle(&refs)
    }

    /// **Weighted bundling**: each vector contributes with a floating-point weight.
    ///
    /// Used for multi-view embedding with paper-specified weights.
    pub fn bundle_weighted(items: &[(&Hypervec, f64)]) -> Hypervec {
        assert!(!items.is_empty(), "cannot bundle_weighted empty slice");
        let dim = items[0].0.dim();
        assert!(items.iter().all(|(v, _)| v.dim() == dim), "dimension mismatch");
        let mut sum: Vec<f64> = vec![0.0; dim];
        for (v, w) in items {
            for (s, &x) in sum.iter_mut().zip(v.data.iter()) {
                *s += *w * (x as f64);
            }
        }
        // Sign-binarize
        let data: Vec<i8> = sum.iter().map(|&s| if s >= 0.0 { 1i8 } else { -1i8 }).collect();
        Hypervec { data }
    }

    /// **Sign/rebinarize**: map each element through sign (≥0 → +1, <0 → -1).
    pub fn sign(mut self) -> Hypervec {
        for x in self.data.iter_mut() {
            *x = if *x >= 0 { 1 } else { -1 };
        }
        self
    }

    /// Rebinarize a sum-accumulator (i32 vector) into a bipolar hypervector.
    pub fn sign_from_sum(sum: Vec<i32>) -> Hypervec {
        let data: Vec<i8> = sum.iter().map(|&s| if s >= 0 { 1i8 } else { -1i8 }).collect();
        Hypervec { data }
    }

    /// **Cyclic permutation** (ρ^k): left-rotate elements by `k` positions.
    ///
    /// Used to encode positional information (e.g., instruction index in a window).
    /// ρ^0 = identity, ρ^k is invertible via ρ^(D-k).
    pub fn permute(&self, k: usize) -> Hypervec {
        let dim = self.dim();
        if dim == 0 { return self.clone(); }
        let k = k % dim;
        if k == 0 { return self.clone(); }
        let mut data = Vec::with_capacity(dim);
        data.extend_from_slice(&self.data[k..]);
        data.extend_from_slice(&self.data[..k]);
        Hypervec { data }
    }

    /// Inverse permutation: right-rotate by k (equivalent to left-rotate by D-k).
    pub fn unpermute(&self, k: usize) -> Hypervec {
        let dim = self.dim();
        if dim == 0 { return self.clone(); }
        let k = k % dim;
        self.permute(dim - k)
    }

    // -------------------------------------------------------------------------
    // Similarity / distance — bit-packed fast paths
    // -------------------------------------------------------------------------

    /// **Cosine similarity** between two hypervectors: dot(a, b) / D ∈ [−1, +1].
    ///
    /// Uses the bit-packed identity:
    ///   cosine_sim = (D - 2 * hamming_dist) / D
    /// where hamming_dist is computed via XOR + popcount over packed u64 words.
    pub fn cosine_sim(&self, other: &Hypervec) -> f64 {
        let dist = self.hamming_dist(other);
        let d = self.dim() as f64;
        (d - 2.0 * dist as f64) / d
    }

    /// Hamming similarity: fraction of matching positions ∈ [0, 1].
    /// hamming_sim = (cosine_sim + 1) / 2.
    pub fn hamming_sim(&self, other: &Hypervec) -> f64 {
        let dist = self.hamming_dist(other);
        1.0 - dist as f64 / self.dim() as f64
    }

    /// Number of positions where self ≠ other (Hamming distance).
    ///
    /// Fast path: pack into u64 words then XOR + popcount.
    pub fn hamming_dist(&self, other: &Hypervec) -> usize {
        assert_eq!(self.dim(), other.dim(), "dimension mismatch in hamming_dist");
        let a = pack_bits(&self.data);
        let b = pack_bits(&other.data);
        hamming_dist_packed(&a, &b) as usize
    }

    /// Pack the hypervector into u64 bit words for fast distance computation.
    /// +1 maps to bit 1, -1 maps to bit 0.
    pub fn pack(&self) -> PackedHypervec {
        PackedHypervec {
            words: pack_bits(&self.data),
            dim: self.data.len(),
        }
    }

    /// Flip `n` random bits (for noise testing).
    pub fn flip_bits_seeded(&self, n: usize, seed: u64) -> Hypervec {
        let mut data = self.data.clone();
        let mut rng = SmallRng::seed_from_u64(seed);
        let dim = data.len();
        // Reservoir sample n distinct indices
        let mut indices: Vec<usize> = (0..dim).collect();
        for i in 0..n.min(dim) {
            let j = rng.gen_range(i..dim);
            indices.swap(i, j);
        }
        for &idx in &indices[..n.min(dim)] {
            data[idx] = -data[idx];
        }
        Hypervec { data }
    }

    /// Return the raw data slice.
    pub fn as_slice(&self) -> &[i8] {
        &self.data
    }

    /// Return a mutable reference to the raw data slice.
    pub(crate) fn as_slice_mut(&mut self) -> &mut [i8] {
        self.data.as_mut_slice()
    }

    // -------------------------------------------------------------------------
    // Serialization helpers
    // -------------------------------------------------------------------------

    /// Save this hypervector to a JSON file.
    pub fn save_json<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let json = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut f = std::fs::File::create(path)?;
        f.write_all(json.as_bytes())
    }

    /// Load a hypervector from a JSON file.
    pub fn load_json<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;
        serde_json::from_str(&buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Save this hypervector to a binary (bincode) file.
    pub fn save_bin<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let bytes = bincode::serialize(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut f = std::fs::File::create(path)?;
        f.write_all(&bytes)
    }

    /// Load a hypervector from a binary (bincode) file.
    pub fn load_bin<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        bincode::deserialize(&buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

impl std::ops::Mul for &Hypervec {
    type Output = Hypervec;
    /// Element-wise multiplication (binding).
    fn mul(self, rhs: &Hypervec) -> Hypervec {
        self.bind(rhs)
    }
}

// ---------------------------------------------------------------------------
// PackedHypervec — bit-packed representation for fast distance queries
// ---------------------------------------------------------------------------

/// A bit-packed view of a [`Hypervec`] for fast Hamming distance queries.
///
/// Encoding: +1 → bit 1, -1 → bit 0.
/// Hamming distance = popcount(XOR(a.words, b.words)).
/// Cosine similarity = (dim - 2 * hamming) / dim.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackedHypervec {
    pub(crate) words: Vec<u64>,
    pub(crate) dim: usize,
}

impl PackedHypervec {
    /// Hamming distance to another packed hypervector.
    pub fn hamming_dist(&self, other: &PackedHypervec) -> usize {
        debug_assert_eq!(self.dim, other.dim);
        hamming_dist_packed(&self.words, &other.words) as usize
    }

    /// Cosine similarity to another packed hypervector ∈ [−1, +1].
    pub fn cosine_sim(&self, other: &PackedHypervec) -> f64 {
        let dist = self.hamming_dist(other);
        (self.dim as f64 - 2.0 * dist as f64) / self.dim as f64
    }

    /// Number of dimensions.
    pub fn dim(&self) -> usize {
        self.dim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bind_self_inverse() {
        let a = Hypervec::random_seeded(64, 1);
        let aa = a.bind(&a);
        // a ⊗ a should be all +1
        assert!(aa.data.iter().all(|&x| x == 1), "a ⊗ a should be ones");
    }

    #[test]
    fn test_bind_commutativity() {
        let a = Hypervec::random_seeded(64, 2);
        let b = Hypervec::random_seeded(64, 3);
        assert_eq!(a.bind(&b), b.bind(&a));
    }

    #[test]
    fn test_bind_associativity() {
        let a = Hypervec::random_seeded(32, 10);
        let b = Hypervec::random_seeded(32, 11);
        let c = Hypervec::random_seeded(32, 12);
        let lhs = a.bind(&b).bind(&c);
        let rhs = a.bind(&b.bind(&c));
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn test_unbind_recovers_value() {
        let a = Hypervec::random_seeded(64, 5);
        let b = Hypervec::random_seeded(64, 6);
        let bound = a.bind(&b);
        let recovered = bound.unbind(&a);
        assert_eq!(recovered, b);
    }

    #[test]
    fn test_bundle_similar_to_inputs() {
        let a = Hypervec::random_seeded(1024, 7);
        let b = Hypervec::random_seeded(1024, 8);
        let bundled = Hypervec::bundle2(&a, &b);
        // bundled should be more similar to a and b than to a random vector
        let sim_a = bundled.cosine_sim(&a);
        let sim_b = bundled.cosine_sim(&b);
        let random = Hypervec::random_seeded(1024, 99);
        let sim_rand = bundled.cosine_sim(&random);
        assert!(sim_a > sim_rand + 0.1, "bundled should be close to a (sim={sim_a:.3} vs rand={sim_rand:.3})");
        assert!(sim_b > sim_rand + 0.1, "bundled should be close to b (sim={sim_b:.3} vs rand={sim_rand:.3})");
    }

    #[test]
    fn test_permute_invertible() {
        let a = Hypervec::random_seeded(128, 20);
        for k in [0, 1, 5, 127, 128] {
            let permuted = a.permute(k);
            let recovered = permuted.unpermute(k);
            assert_eq!(recovered, a, "permute/unpermute failed for k={k}");
        }
    }

    #[test]
    fn test_permute_changes_vector() {
        let a = Hypervec::random_seeded(1024, 21);
        let p = a.permute(1);
        let sim = a.cosine_sim(&p);
        // Should be quasi-orthogonal (near zero similarity)
        assert!(sim.abs() < 0.1, "permute by 1 should be near-orthogonal, got sim={sim:.4}");
    }

    #[test]
    fn test_cosine_sim_identical() {
        let a = Hypervec::random_seeded(256, 30);
        assert!((a.cosine_sim(&a) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_cosine_sim_random_near_zero() {
        let a = Hypervec::random_seeded(4096, 31);
        let b = Hypervec::random_seeded(4096, 32);
        let sim = a.cosine_sim(&b);
        // Expected ~0, within 3σ ≈ 3/sqrt(4096) ≈ 0.047
        assert!(sim.abs() < 0.1, "random vectors should be near-orthogonal, got {sim:.4}");
    }

    #[test]
    fn test_bind_quasi_orthogonal_to_inputs() {
        let a = Hypervec::random_seeded(4096, 40);
        let b = Hypervec::random_seeded(4096, 41);
        let bound = a.bind(&b);
        let sim_a = bound.cosine_sim(&a);
        let sim_b = bound.cosine_sim(&b);
        assert!(sim_a.abs() < 0.1, "a⊗b quasi-orthogonal to a (sim={sim_a:.4})");
        assert!(sim_b.abs() < 0.1, "a⊗b quasi-orthogonal to b (sim={sim_b:.4})");
    }

    #[test]
    fn test_bundle_majority_vote() {
        // With 3 identical + 1 different, bundled should equal the majority
        let a = Hypervec::random_seeded(4096, 50);
        let b = Hypervec::random_seeded(4096, 51);
        let bundled = Hypervec::bundle(&[&a, &a, &a, &b]);
        let sim_a = bundled.cosine_sim(&a);
        let sim_b = bundled.cosine_sim(&b);
        assert!(sim_a > 0.5, "3:1 majority should be close to a (sim={sim_a:.4})");
        assert!(sim_a > sim_b, "majority bundle should be more similar to a than b");
    }

    #[test]
    fn test_determinism() {
        let a = Hypervec::random_seeded(256, 42);
        let b = Hypervec::random_seeded(256, 42);
        assert_eq!(a, b, "same seed must produce same vector");
    }

    #[test]
    fn test_different_seeds_orthogonal() {
        let a = Hypervec::random_seeded(4096, 100);
        let b = Hypervec::random_seeded(4096, 200);
        let sim = a.cosine_sim(&b);
        assert!(sim.abs() < 0.1, "different seeds should be near-orthogonal (sim={sim:.4})");
    }

    #[test]
    fn test_weighted_bundle() {
        let a = Hypervec::random_seeded(4096, 60);
        let b = Hypervec::random_seeded(4096, 61);
        // Weight a heavily
        let wb = Hypervec::bundle_weighted(&[(&a, 3.0), (&b, 1.0)]);
        let sim_a = wb.cosine_sim(&a);
        let sim_b = wb.cosine_sim(&b);
        assert!(sim_a > sim_b, "higher weight should mean more similar");
    }

    // ── Bit-packed fast-path tests ────────────────────────────────────────

    #[test]
    fn test_hamming_dist_packed_agrees_with_naive() {
        let a = Hypervec::random_seeded(4096, 77);
        let b = Hypervec::random_seeded(4096, 78);
        let naive: usize = a.data.iter().zip(b.data.iter()).filter(|(&x, &y)| x != y).count();
        let fast = a.hamming_dist(&b);
        assert_eq!(naive, fast, "bit-packed hamming_dist must match naive count");
    }

    #[test]
    fn test_cosine_sim_packed_agrees_with_dot() {
        let a = Hypervec::random_seeded(4096, 79);
        let b = Hypervec::random_seeded(4096, 80);
        let dot_sim = {
            let dot: i64 = a.data.iter().zip(b.data.iter())
                .map(|(&x, &y)| (x as i64) * (y as i64))
                .sum();
            dot as f64 / 4096.0
        };
        let fast_sim = a.cosine_sim(&b);
        assert!((dot_sim - fast_sim).abs() < 1e-10,
            "bit-packed cosine_sim {fast_sim:.6} must match dot-product sim {dot_sim:.6}");
    }

    #[test]
    fn test_packed_self_dist_is_zero() {
        let a = Hypervec::random_seeded(512, 81);
        assert_eq!(a.hamming_dist(&a), 0, "self hamming distance must be zero");
    }

    #[test]
    fn test_pack_round_trip() {
        let a = Hypervec::random_seeded(512, 82);
        let pa = a.pack();
        let b = Hypervec::random_seeded(512, 83);
        let pb = b.pack();
        let packed_dist = pa.hamming_dist(&pb);
        let direct_dist = a.hamming_dist(&b);
        assert_eq!(packed_dist, direct_dist, "packed and direct hamming_dist must agree");
    }

    #[test]
    fn test_bundle_chunked_matches_naive() {
        // Verify chunked accumulation in bundle gives same result as naive i32
        let vecs: Vec<Hypervec> = (0..200).map(|i| Hypervec::random_seeded(128, i)).collect();
        let refs: Vec<&Hypervec> = vecs.iter().collect();
        let chunked = Hypervec::bundle(&refs);

        // Naive i32 accumulation
        let mut naive_sum = vec![0i32; 128];
        for v in &vecs {
            for (s, &x) in naive_sum.iter_mut().zip(v.data.iter()) {
                *s += x as i32;
            }
        }
        let naive = Hypervec::sign_from_sum(naive_sum);
        assert_eq!(chunked, naive, "chunked bundle must match naive i32 accumulation");
    }

    // ── Serde / save-load tests ───────────────────────────────────────────

    #[test]
    fn test_serde_json_roundtrip() {
        let v = Hypervec::random_seeded(256, 99);
        let json = serde_json::to_string(&v).expect("serialize");
        let v2: Hypervec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(v, v2, "JSON round-trip must be lossless");
    }

    #[test]
    fn test_serde_bincode_roundtrip() {
        let v = Hypervec::random_seeded(256, 100);
        let bytes = bincode::serialize(&v).expect("bincode serialize");
        let v2: Hypervec = bincode::deserialize(&bytes).expect("bincode deserialize");
        assert_eq!(v, v2, "bincode round-trip must be lossless");
    }

    #[test]
    fn test_save_load_json() {
        let v = Hypervec::random_seeded(128, 101);
        let path = std::env::temp_dir().join("opcode_vsa_test_hv.json");
        v.save_json(&path).expect("save json");
        let v2 = Hypervec::load_json(&path).expect("load json");
        assert_eq!(v, v2, "save/load JSON must be lossless");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_save_load_bin() {
        let v = Hypervec::random_seeded(128, 102);
        let path = std::env::temp_dir().join("opcode_vsa_test_hv.bin");
        v.save_bin(&path).expect("save bin");
        let v2 = Hypervec::load_bin(&path).expect("load bin");
        assert_eq!(v, v2, "save/load binary must be lossless");
        let _ = std::fs::remove_file(&path);
    }
}
