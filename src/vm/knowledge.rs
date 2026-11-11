//! In-VM knowledge store — exact-key grounding with provenance.
//!
//! `QUERY` used to be `trace_structural`: it read a register, wrote a log line,
//! and retrieved nothing. The SPEC has always typed it as
//! `rag { chunks: array<{text, score, source}> }` — provenance sitting in the
//! type, waiting for an implementation.
//!
//! ## Why a hash map and not an embedding index
//!
//! A canonical title *is* a key. DBpedia's 4.6M entities normalize to 4,628,421
//! unique titles with 0.2% collisions — so `"printing press"` →
//! `<dbpedia:Printing_press>` is a dict lookup: O(1), exact, and free.
//! Embedding a query costs a transformer forward pass *per lookup*, which is a
//! linear marginal cost — precisely the economics this architecture exists to
//! avoid. Aliases (Wikipedia redirects) convert paraphrase into *more keys*,
//! not more math.
//!
//! `HammingIndex` remains available for a fuzzy tier over binary codes. It is
//! deliberately not wired into `QUERY`: an approximate match is a guess, and a
//! guess must never wear the same confidence as an exact hit.
//!
//! ## Abstention is a result, not an error
//!
//! A miss returns an EMPTY chunk array. Not `Error` — nothing failed. Not a
//! nearest neighbour — that is how you get "the capital of France is Kyiv" at
//! 0.699, or the metalworking profession for a question about Gutenberg.
//! Zero chunks is the program's cue to say it does not know.
//!
//! ## Layers (2026-10-24)
//!
//! The store was one hash map, and every request (every branch) got a full
//! copy of it: memory and time linear in the store, per request. It is now
//! layered, as a log-structured store is:
//!
//! - **segments**, immutable and shared (`segment.rs`): sorted keys in a
//!   finite-state transducer, facts in a blob. A fork clones `Arc`s.
//! - **a top layer**, small and per-store: facts inserted since the last
//!   freeze (a branch's assumptions), and the keys it masks (a branch's
//!   exclusions).
//!
//! `freeze` turns the top layer into a new segment; `compact` folds every
//! layer into one. Lookups keep their meaning exactly: a key's facts are its
//! facts in every segment, oldest first (a later segment that replaced the
//! key starts the list over), then the top layer's; a masked key shows only
//! the top layer's.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use fst::{IntoStreamer, Streamer};

use crate::vm::segment::{self, Bytes};

/// One grounded fact. `source` is not optional: an unattributed fact is the
/// confabulation this store exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fact {
    pub text: String,
    pub source: String,
}

/// Fold a surface form to a matchable key: lowercase, punctuation to spaces,
/// whitespace collapsed. Deliberately conservative — this is exact alias
/// matching, not fuzzy retrieval.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true; // trims leading space
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// A key's value in a segment: bit 63 says the key REPLACES what older
/// segments hold for it (a masked or re-aliased key); the rest is the offset
/// of its record in the blob. Offset 0 is the empty record, so a key that
/// replaces with nothing is a tombstone.
const REPLACE: u64 = 1 << 63;

/// One frozen layer: normalized key -> record (the key's facts, and every key
/// sharing that record: its aliases).
#[derive(Clone)]
pub struct KnowledgeSegment {
    map: fst::Map<Bytes>,
    blob: Bytes,
}

impl std::fmt::Debug for KnowledgeSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KnowledgeSegment({} keys, {} bytes)", self.map.len(), self.size_bytes())
    }
}

impl KnowledgeSegment {
    /// The segment's own index (key -> record), for the typo index's merge.
    pub(crate) fn index(&self) -> &fst::Map<Bytes> {
        &self.map
    }

    /// Keys in this segment (live or tombstoned).
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The bytes the segment occupies (index + blob), shared by every fork.
    pub fn size_bytes(&self) -> usize {
        self.map.as_fst().as_bytes().len() + self.blob.len()
    }

    /// Open a segment file (read once; share the result).
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let f = segment::read_file(path, segment::KIND_KNOWLEDGE)?;
        Self::from_parts(f.fst, f.blob)
    }

    /// A segment from a whole file's bytes.
    pub fn from_file_bytes(buf: Arc<[u8]>) -> Result<Self, String> {
        let f = segment::decode(buf, segment::KIND_KNOWLEDGE)?;
        Self::from_parts(f.fst, f.blob)
    }

    fn from_parts(fst: Bytes, blob: Bytes) -> Result<Self, String> {
        let map = fst::Map::new(fst).map_err(|e| format!("knowledge segment index: {}", e))?;
        Ok(KnowledgeSegment { map, blob })
    }

    /// The segment as a file.
    pub fn to_file_bytes(&self) -> Vec<u8> {
        segment::encode(segment::KIND_KNOWLEDGE, self.map.as_fst().as_bytes(), self.blob.as_ref())
    }

    /// Write the segment file (atomically).
    pub fn write(&self, path: &std::path::Path) -> std::io::Result<()> {
        segment::write_atomic(path, &self.to_file_bytes())
    }

    /// (replaces older layers, facts) for a normalized key held here.
    fn get(&self, key: &str) -> Option<(bool, Vec<Fact>)> {
        let v = self.map.get(key)?;
        Some((v & REPLACE != 0, self.facts_at((v & !REPLACE) as usize)))
    }

    fn facts_at(&self, offset: usize) -> Vec<Fact> {
        let blob = self.blob.as_ref();
        let mut at = offset;
        let n = segment::get_uvarint(blob, &mut at).unwrap_or(0) as usize;
        let mut out = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            match (segment::get_str(blob, &mut at), segment::get_str(blob, &mut at)) {
                (Some(text), Some(source)) => out.push(Fact { text: text.to_string(), source: source.to_string() }),
                _ => break,
            }
        }
        out
    }

    /// Every key sharing `key`'s record (its aliases), `key` included.
    fn record_keys(&self, key: &str) -> Vec<String> {
        let Some(v) = self.map.get(key) else { return Vec::new() };
        let blob = self.blob.as_ref();
        let mut at = (v & !REPLACE) as usize;
        let n = segment::get_uvarint(blob, &mut at).unwrap_or(0);
        for _ in 0..n {
            let _ = segment::get_str(blob, &mut at);
            let _ = segment::get_str(blob, &mut at);
        }
        let k = segment::get_uvarint(blob, &mut at).unwrap_or(0);
        (0..k).filter_map(|_| segment::get_str(blob, &mut at).map(str::to_string)).collect()
    }

    /// Every key, in order.
    fn keys(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.map.len());
        let mut s = self.map.keys();
        while let Some(k) = s.next() {
            out.push(String::from_utf8_lossy(k).into_owned());
        }
        out
    }

    /// Build a segment: each key's (replaces, facts). Keys with the same facts share one record,
    /// which lists them all (so an alias costs a key in the index, not a copy of its facts).
    fn build(entries: BTreeMap<String, (bool, Vec<Fact>)>) -> Result<Self, String> {
        let mut blob: Vec<u8> = Vec::new();
        segment::put_uvarint(&mut blob, 0); // the empty record at offset 0: no facts...
        segment::put_uvarint(&mut blob, 0); // ...and no keys
        // group the keys by their facts, in key order (so the layout is deterministic)
        let mut group_of: HashMap<&[Fact], usize> = HashMap::with_capacity(entries.len());
        let mut groups: Vec<(&[Fact], Vec<&str>)> = Vec::new();
        let mut key_group: Vec<Option<usize>> = Vec::with_capacity(entries.len());
        for (k, (_, facts)) in &entries {
            if facts.is_empty() {
                key_group.push(None);
                continue;
            }
            let g = *group_of.entry(facts.as_slice()).or_insert_with(|| {
                groups.push((facts.as_slice(), Vec::new()));
                groups.len() - 1
            });
            groups[g].1.push(k.as_str());
            key_group.push(Some(g));
        }
        let mut offsets: Vec<u64> = Vec::with_capacity(groups.len());
        for (facts, keys) in &groups {
            offsets.push(blob.len() as u64);
            segment::put_uvarint(&mut blob, facts.len() as u64);
            for f in *facts {
                segment::put_bytes(&mut blob, f.text.as_bytes());
                segment::put_bytes(&mut blob, f.source.as_bytes());
            }
            segment::put_uvarint(&mut blob, keys.len() as u64);
            for k in keys {
                segment::put_bytes(&mut blob, k.as_bytes());
            }
        }
        if blob.len() as u64 >= REPLACE {
            return Err("knowledge segment over 8 EiB".into());
        }
        let mut b = fst::MapBuilder::memory();
        for ((k, (replace, _)), g) in entries.iter().zip(&key_group) {
            let off = g.map_or(0, |g| offsets[g]);
            b.insert(k.as_bytes(), off | if *replace { REPLACE } else { 0 })
                .map_err(|e| format!("knowledge segment index: {}", e))?;
        }
        let fst_bytes = b.into_inner().map_err(|e| format!("knowledge segment index: {}", e))?;
        Self::from_parts(Bytes::new(Arc::from(fst_bytes.into_boxed_slice())),
                         Bytes::new(Arc::from(blob.into_boxed_slice())))
    }
}

#[derive(Debug, Default, Clone)]
pub struct KnowledgeStore {
    /// The frozen layers, oldest first, shared by every fork.
    segments: Vec<Arc<KnowledgeSegment>>,
    /// normalized key -> the facts carrying that name, added since the last
    /// freeze.
    ///
    /// A Vec, not a single Fact: ambiguity is real (`Mercury` is a planet, an
    /// element and a god). Several facts under one key is not a failure — it is
    /// the program's cue to ASK, which is the only honest move when only the
    /// caller knows which was meant.
    top: HashMap<String, Vec<Fact>>,
    /// Keys whose segment facts this layer took out (a branch's exclusions,
    /// a re-pointed alias). Their top-layer facts still show.
    masked: HashSet<String>,
    /// One typo index over the first layers (a store opened from shards; `vm::typo_index`), shared by forks.
    typo: Option<Arc<crate::vm::typo_index::TypoIndex>>,
}

impl KnowledgeStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store over one segment file.
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        Ok(Self::from_segment(KnowledgeSegment::open(path)?))
    }

    pub fn from_segment(seg: KnowledgeSegment) -> Self {
        KnowledgeStore { segments: vec![Arc::new(seg)], ..Default::default() }
    }

    /// A store over several segments, as its layers in the order given (oldest first). CubbyLLM order 0046,
    /// R1b (2026-11-04): a names index of ~10^8 keys is built as shards, each small enough to build in memory,
    /// and opened together; a lookup reads each shard's own index, as it reads a frozen layer's.
    pub fn from_segments(segs: Vec<KnowledgeSegment>) -> Self {
        KnowledgeStore { segments: segs.into_iter().map(Arc::new).collect(), ..Default::default() }
    }

    /// A store over several segment files (see `from_segments`).
    pub fn open_layers(paths: &[std::path::PathBuf]) -> Result<Self, String> {
        let segs = paths.iter().map(|p| KnowledgeSegment::open(p)).collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_segments(segs))
    }

    /// Use `t` for typo lookups over the layers it covers (the first `t.covers`).
    pub fn set_typo_index(&mut self, t: crate::vm::typo_index::TypoIndex) {
        assert!(t.covers <= self.segments.len(), "a typo index over more layers than the store has");
        self.typo = Some(Arc::new(t));
    }

    /// Whether typo lookups read one typo index over the shards.
    pub fn has_typo_index(&self) -> bool {
        self.typo.is_some()
    }

    /// The live keys (every layer folded). O(store): for reports and tests.
    pub fn len(&self) -> usize {
        self.keys().len()
    }

    pub fn is_empty(&self) -> bool {
        if self.masked.is_empty() {
            self.top.is_empty() && self.segments.iter().all(|s| s.is_empty())
        } else {
            self.len() == 0
        }
    }

    /// How many frozen layers, and the bytes they share.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn segment_bytes(&self) -> usize {
        self.segments.iter().map(|s| s.size_bytes()).sum()
    }

    /// Keys in the mutable top layer (what a fork copies).
    pub fn top_len(&self) -> usize {
        self.top.len() + self.masked.len()
    }

    /// Register a fact under a surface form. Repeated keys accumulate.
    pub fn insert(&mut self, key: &str, text: impl Into<String>, source: impl Into<String>) {
        let k = normalize(key);
        if k.is_empty() {
            return;
        }
        self.top.entry(k).or_default().push(Fact {
            text: text.into(),
            source: source.into(),
        });
    }

    /// An alias is just another key pointing at the same facts. This is how
    /// paraphrase gets handled without a model: `"movable type press"` and
    /// `"printing press"` are two entries, not two embeddings.
    pub fn alias(&mut self, alias: &str, canonical: &str) -> bool {
        let c = normalize(canonical);
        let a = normalize(alias);
        if a.is_empty() || a == c {
            return false;
        }
        let facts = self.lookup_key(&c);
        if facts.is_empty() {
            return false;
        }
        // the alias now carries exactly the canonical's facts, whatever it carried before
        self.mask(&a);
        self.top.insert(a, facts);
        true
    }

    /// Branch support (2026-09-28, the counterfactual fork): take a key OUT of
    /// this store, and every alias that carried the same facts, so a QUERY for
    /// it abstains (an empty chunk array) exactly as it would for a name the
    /// store never held. Returns how many keys were removed. Used on the CLONE
    /// a `run-proto` request gets, never on the cached original: "X did not
    /// happen" lives in one branch's VM and nowhere else. (2026-10-24: the
    /// clone shares the segments; the removal is a mask in its top layer.)
    pub fn remove(&mut self, key: &str) -> usize {
        let k = normalize(key);
        let gone = self.lookup_key(&k);
        if gone.is_empty() {
            return 0;
        }
        let mut candidates: HashSet<String> = HashSet::new();
        for s in &self.segments {
            candidates.extend(s.record_keys(&k));
        }
        candidates.extend(self.top.keys().cloned());
        candidates.remove(&k);
        let aliases: Vec<String> = candidates.into_iter()
            .filter(|c| self.lookup_key(c) == gone)
            .collect();
        self.drop_key(&k);
        for a in &aliases {
            self.drop_key(a);
        }
        1 + aliases.len()
    }

    fn mask(&mut self, k: &str) {
        if self.segments.iter().any(|s| s.map.contains_key(k)) {
            self.masked.insert(k.to_string());
        }
    }

    fn drop_key(&mut self, k: &str) {
        self.top.remove(k);
        self.mask(k);
    }

    /// Every normalized key the store holds, sorted (for the branch report and tests).
    pub fn keys(&self) -> Vec<String> {
        let mut all: std::collections::BTreeSet<String> = self.top.keys().cloned().collect();
        for s in &self.segments {
            all.extend(s.keys());
        }
        all.into_iter().filter(|k| !self.lookup_key(k).is_empty()).collect()
    }

    /// Exact lookup. An empty result means "no fact carries this name" — the
    /// honest gap. Never a nearest neighbour.
    pub fn lookup(&self, mention: &str) -> Vec<Fact> {
        self.lookup_key(&normalize(mention))
    }

    fn lookup_key(&self, k: &str) -> Vec<Fact> {
        let mut out = Vec::new();
        if !self.masked.contains(k) {
            for s in &self.segments {
                if let Some((replace, facts)) = s.get(k) {
                    if replace {
                        out.clear();
                    }
                    out.extend(facts);
                }
            }
        }
        if let Some(t) = self.top.get(k) {
            out.extend(t.iter().cloned());
        }
        out
    }

    /// The top layer becomes a new segment (frozen, shared); the top is empty after.
    pub fn freeze(&mut self) -> Result<(), String> {
        if self.top.is_empty() && self.masked.is_empty() {
            return Ok(());
        }
        let mut entries: BTreeMap<String, (bool, Vec<Fact>)> = BTreeMap::new();
        for k in &self.masked {
            entries.insert(k.clone(), (true, Vec::new()));
        }
        for (k, facts) in &self.top {
            let e = entries.entry(k.clone()).or_insert((false, Vec::new()));
            e.1 = facts.clone();
        }
        let seg = KnowledgeSegment::build(entries)?;
        self.segments.push(Arc::new(seg));
        // new maps, not cleared ones: a cleared map keeps its capacity, and every fork would copy it
        self.top = HashMap::new();
        self.masked = HashSet::new();
        Ok(())
    }

    /// Every layer folded into one segment: masks applied, emptied keys dropped.
    pub fn compact(&mut self) -> Result<(), String> {
        if self.segments.len() <= 1 && self.top.is_empty() && self.masked.is_empty()
            && self.segments.iter().all(|s| !has_flags(s)) {
            return Ok(());
        }
        let entries: BTreeMap<String, (bool, Vec<Fact>)> = if self.segments.is_empty() {
            // the common case, a store built from facts: the top layer is the whole store
            std::mem::take(&mut self.top).into_iter().map(|(k, v)| (k, (false, v))).collect()
        } else {
            self.keys().into_iter().map(|k| {
                let facts = self.lookup_key(&k);
                (k, (false, facts))
            }).collect()
        };
        let seg = KnowledgeSegment::build(entries)?;
        self.segments = vec![Arc::new(seg)];
        self.typo = None; // its layers are gone
        self.top = HashMap::new();
        self.masked = HashSet::new();
        Ok(())
    }

    /// The store as one segment (compacting a copy; this store is untouched).
    pub fn to_segment(&self) -> Result<KnowledgeSegment, String> {
        let mut c = self.clone();
        c.compact()?;
        match c.segments.pop() {
            Some(s) => Ok((*s).clone()),
            None => KnowledgeSegment::build(BTreeMap::new()),
        }
    }

    /// Load newline-delimited JSON: `{"key": ..., "text": ..., "source": ...}`
    /// plus optional `{"alias": ..., "of": ...}` records.
    /// Returns (facts_loaded, aliases_loaded).
    pub fn load_jsonl(&mut self, data: &str) -> Result<(usize, usize), String> {
        let (mut facts, mut aliases) = (0usize, 0usize);
        for (i, line) in data.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| format!("knowledge line {}: {}", i + 1, e))?;

            if let (Some(a), Some(of)) = (v.get("alias").and_then(|x| x.as_str()),
                                          v.get("of").and_then(|x| x.as_str())) {
                if self.alias(a, of) {
                    aliases += 1;
                }
                continue;
            }
            let key = v.get("key").and_then(|x| x.as_str())
                .ok_or_else(|| format!("knowledge line {}: missing \"key\"", i + 1))?;
            let text = v.get("text").and_then(|x| x.as_str())
                .ok_or_else(|| format!("knowledge line {}: missing \"text\"", i + 1))?;
            // A fact without a source is exactly what this store refuses to hold.
            let source = v.get("source").and_then(|x| x.as_str())
                .ok_or_else(|| format!(
                    "knowledge line {}: missing \"source\" -- an unattributed fact \
                     is a confabulation with better formatting", i + 1))?;
            self.insert(key, text, source);
            facts += 1;
        }
        Ok((facts, aliases))
    }
}

/// Does a segment carry replace flags (masks)? A lone base segment without them needs no compaction.
fn has_flags(s: &KnowledgeSegment) -> bool {
    let mut st = s.map.stream();
    while let Some((_, v)) = st.next() {
        if v & REPLACE != 0 {
            return true;
        }
    }
    false
}

/// Keys in a store starting with `prefix` (normalized), sorted, at most `limit`: for tools that
/// browse a store. QUERY stays exact.
pub fn keys_with_prefix(store: &KnowledgeStore, prefix: &str, limit: usize) -> Vec<String> {
    let p = normalize(prefix);
    let mut all: std::collections::BTreeSet<String> = store.top.keys().filter(|k| k.starts_with(&p)).cloned().collect();
    for s in &store.segments {
        let aut = fst::Automaton::starts_with(fst::automaton::Str::new(&p));
        let mut st = s.map.search(aut).into_stream();
        while let Some((k, _)) = st.next() {
            all.insert(String::from_utf8_lossy(k).into_owned());
        }
    }
    all.into_iter().filter(|k| !store.lookup_key(k).is_empty()).take(limit).collect()
}

/// The most edits a typo lookup allows: one or two, as the history's fuzzy search.
pub const MAX_EDITS: u32 = 2;

/// One key within reach of a mention: the key, how many edits away, its facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyHit {
    pub key: String,
    pub edits: u32,
    pub facts: Vec<Fact>,
}

/// Typo-tolerant lookup (order 0046, 2026-10-31): every live key at most `edits` edits (Levenshtein, on
/// Unicode characters; capped at `MAX_EDITS`) from the normalized mention, nearest first, then by key, at
/// most `limit`, each with its facts. Through each segment's own index (`vm::lev`, a Levenshtein row walked
/// down its keys), never a read of every key; pinned to answer as a scan of every key does.
///
/// Not QUERY: QUERY stays exact. A hit here is a candidate carrying its distance, never a fact at
/// confidence 1.0 -- the program decides what an edit away is worth (and that 0 edits is the exact hit).
pub fn fuzzy_lookup(store: &KnowledgeStore, mention: &str, edits: u32, limit: usize) -> Vec<FuzzyHit> {
    let q = normalize(mention);
    let d = edits.min(MAX_EDITS);
    if q.is_empty() || limit == 0 {
        return Vec::new();
    }
    let near = |k: &str| crate::vm::history::edit_distance(&q, k) <= d as usize;
    let mut keys: std::collections::BTreeSet<String> = store.top.keys().filter(|k| near(k)).cloned().collect();
    // the layers one typo index covers are walked once, through it; any later layer through its own index
    let skip = match &store.typo {
        Some(t) => {
            let mut st = t.set.search(crate::vm::lev::Lev::new(&q, d)).into_stream();
            while let Some(k) = st.next() {
                keys.insert(String::from_utf8_lossy(k).into_owned());
            }
            t.covers
        }
        None => 0,
    };
    for s in &store.segments[skip..] {
        let mut st = s.map.search(crate::vm::lev::Lev::new(&q, d)).into_stream();
        while let Some((k, _)) = st.next() {
            keys.insert(String::from_utf8_lossy(k).into_owned());
        }
    }
    // nearest first, then by key; facts are read only until `limit` live keys are found (a short mention
    // reaches thousands of keys at two edits, and each read visits every layer)
    let mut near: Vec<(u32, String)> = keys.into_iter()
        .map(|k| (crate::vm::history::edit_distance(&q, &k) as u32, k)).collect();
    near.sort();
    let mut hits = Vec::new();
    for (e, k) in near {
        if hits.len() >= limit {
            break;
        }
        let facts = store.lookup_key(&k);
        if !facts.is_empty() {
            hits.push(FuzzyHit { key: k, edits: e, facts });
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_folds_case_and_punctuation() {
        assert_eq!(normalize("Printing Press"), "printing press");
        assert_eq!(normalize("  the  Printing-Press! "), "the printing press");
        assert_eq!(normalize("D"), "d");
        assert_eq!(normalize("!!!"), "");
    }

    #[test]
    fn lookup_miss_returns_empty_not_a_neighbour() {
        let mut ks = KnowledgeStore::new();
        ks.insert("printing press", "Gutenberg, c. 1440", "dbpedia:Printing_press");
        assert!(ks.lookup("goldsmith").is_empty());
        assert_eq!(ks.lookup("Printing Press").len(), 1);
    }

    #[test]
    fn aliases_are_extra_keys_not_extra_math() {
        let mut ks = KnowledgeStore::new();
        ks.insert("printing press", "Gutenberg, c. 1440", "dbpedia:Printing_press");
        assert!(ks.alias("movable type press", "printing press"));
        assert_eq!(ks.lookup("movable-type press")[0].source, "dbpedia:Printing_press");
        // aliasing a key that does not exist must fail loudly, not create a ghost
        assert!(!ks.alias("x", "no such entity"));
    }

    #[test]
    fn ambiguity_accumulates_under_one_key() {
        let mut ks = KnowledgeStore::new();
        ks.insert("mercury", "the planet", "dbpedia:Mercury_(planet)");
        ks.insert("mercury", "the element", "dbpedia:Mercury_(element)");
        assert_eq!(ks.lookup("Mercury").len(), 2);
    }

    #[test]
    fn jsonl_requires_a_source() {
        let mut ks = KnowledgeStore::new();
        let err = ks.load_jsonl(r#"{"key":"x","text":"y"}"#).unwrap_err();
        assert!(err.contains("source"), "{}", err);
    }

    #[test]
    fn jsonl_loads_facts_and_aliases() {
        let mut ks = KnowledgeStore::new();
        let data = concat!(
            "{\"key\":\"printing press\",\"text\":\"Gutenberg, c. 1440\",\"source\":\"dbpedia:Printing_press\"}\n",
            "# a comment\n",
            "{\"alias\":\"movable type press\",\"of\":\"printing press\"}\n"
        );
        assert_eq!(ks.load_jsonl(data).unwrap(), (1, 1));
        assert_eq!(ks.lookup("Movable Type Press").len(), 1);
    }

    fn sample() -> KnowledgeStore {
        let mut ks = KnowledgeStore::new();
        ks.insert("printing press", "Gutenberg, c. 1440", "dbpedia:Printing_press");
        ks.insert("mercury", "the planet", "dbpedia:Mercury_(planet)");
        ks.insert("mercury", "the element", "dbpedia:Mercury_(element)");
        assert!(ks.alias("movable type press", "printing press"));
        ks
    }

    #[test]
    fn frozen_and_compacted_stores_answer_as_the_map_did() {
        for stage in 0..3 {
            let mut ks = sample();
            if stage >= 1 { ks.freeze().unwrap(); }
            if stage >= 2 { ks.compact().unwrap(); }
            assert_eq!(ks.lookup("Mercury").len(), 2, "stage {stage}");
            assert_eq!(ks.lookup("movable type press")[0].text, "Gutenberg, c. 1440");
            assert!(ks.lookup("goldsmith").is_empty());
            assert_eq!(ks.keys(), vec!["mercury", "movable type press", "printing press"]);
        }
    }

    #[test]
    fn a_fork_masks_and_adds_without_touching_the_shared_segment() {
        let mut base = sample();
        base.compact().unwrap();
        let mut branch = base.clone();
        assert_eq!(branch.remove("printing press"), 2, "the key and its alias");
        branch.insert("printing press", "an assumed fact", "branch");
        branch.insert("mercury", "the god", "dbpedia:Mercury_(mythology)");
        assert_eq!(branch.lookup("printing press"), vec![Fact { text: "an assumed fact".into(), source: "branch".into() }]);
        assert!(branch.lookup("movable type press").is_empty());
        assert_eq!(branch.lookup("mercury").len(), 3);
        // the base is untouched
        assert_eq!(base.lookup("printing press")[0].text, "Gutenberg, c. 1440");
        assert_eq!(base.lookup("mercury").len(), 2);
        // and the branch's changes survive its own freeze and compaction
        branch.freeze().unwrap();
        assert_eq!(branch.segment_count(), 2);
        assert!(branch.lookup("movable type press").is_empty());
        assert_eq!(branch.lookup("printing press").len(), 1);
        branch.compact().unwrap();
        assert_eq!(branch.segment_count(), 1);
        assert_eq!(branch.keys(), vec!["mercury", "printing press"]);
    }

    #[test]
    fn an_alias_shares_its_record_in_a_segment() {
        let mut ks = KnowledgeStore::new();
        ks.insert("a", "x".repeat(1000), "s");
        let one = { let mut c = ks.clone(); c.compact().unwrap(); c.segment_bytes() };
        for i in 0..20 {
            assert!(ks.alias(&format!("alias {i}"), "a"));
        }
        ks.compact().unwrap();
        assert!(ks.segment_bytes() < one + 20 * 100, "{} vs {}", ks.segment_bytes(), one);
        assert_eq!(ks.remove("alias 3"), 21);
        assert!(ks.is_empty());
    }

    #[test]
    fn a_segment_file_round_trips() {
        let seg = sample().to_segment().unwrap();
        let back = KnowledgeSegment::from_file_bytes(Arc::from(seg.to_file_bytes().into_boxed_slice())).unwrap();
        let ks = KnowledgeStore::from_segment(back);
        assert_eq!(ks.lookup("mercury").len(), 2);
        assert_eq!(keys_with_prefix(&ks, "mo", 10), vec!["movable type press"]);
    }

    #[test]
    fn shards_opened_together_answer_as_one_store() {
        let mut a = KnowledgeStore::new();
        a.insert("Paris", "Q90", "label:en");
        a.insert("Mercury", "Q308", "label:en");
        let mut b = KnowledgeStore::new();
        b.insert("Paname", "Q90", "alias:fr");
        b.insert("Mercury", "Q925", "label:en");
        let both = KnowledgeStore::from_segments(vec![a.to_segment().unwrap(), b.to_segment().unwrap()]);
        assert_eq!(both.segment_count(), 2);
        assert_eq!(both.lookup("paname")[0].text, "Q90");
        let m: Vec<String> = both.lookup("mercury").into_iter().map(|f| f.text).collect();
        assert_eq!(m, vec!["Q308", "Q925"]);                        // a key in two shards: both, oldest first
        let near = fuzzy_lookup(&both, "parls", 1, 10);
        assert_eq!(near.len(), 1);
        assert_eq!((near[0].key.as_str(), near[0].edits), ("paris", 1));
        let far: Vec<String> = fuzzy_lookup(&both, "panama", 2, 10).into_iter().map(|h| h.key).collect();
        assert_eq!(far, vec!["paname"]);                             // through the other shard's index
    }
}
