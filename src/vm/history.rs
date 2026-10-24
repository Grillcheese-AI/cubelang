//! Conversation history (2026-10-24): every turn, searchable by the words in it -- exactly, by
//! prefix, or with a typo -- without reading the turns.
//!
//! Turns are appended to a small mutable top layer and frozen into an immutable segment at the end
//! of a turn (`freeze`). A segment holds its turns, an offset table (turn id -> record), and a term
//! index: a finite-state transducer from each normalized word to the list of turns containing it
//! (`segment.rs`). A search walks the index, not the text, so its cost follows the number of
//! matching words, not the length of the conversation. Small recent segments are merged into
//! larger ones (`maybe_merge`), as a log-structured store merges its runs.
//!
//! Forgetting is a first-class operation: `forget(id)` records the id in the top layer, every
//! search and read skips it at once, and the next merge drops the turn from the files (`gc_dir`
//! then removes the files that held it). Ids only grow, across segments, so a turn's id also says
//! when it was said relative to the others.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use fst::{IntoStreamer, Streamer};

use crate::vm::knowledge::normalize;
use crate::vm::segment::{self, Bytes};

/// One utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub id: u64,
    pub at_ms: u64,
    pub speaker: String,
    pub text: String,
}

/// How a query word matches an indexed word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// The same word (after normalizing case and punctuation).
    Exact,
    /// A word starting with it ("guten" finds "gutenberg").
    Prefix,
    /// Within a small edit distance: 0 for words up to 3 letters, 1 up to 7, 2 beyond
    /// ("gutenburg" finds "gutenberg").
    Fuzzy,
}

/// A turn a search found, with the indexed words that matched.
#[derive(Debug, Clone)]
pub struct Hit {
    pub turn: Turn,
    pub matched: Vec<String>,
}

/// The words of a text, as the index holds them: normalized, each once, in order.
pub fn terms_of(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    normalize(text).split(' ')
        .filter(|w| !w.is_empty() && seen.insert(w.to_string()))
        .map(str::to_string)
        .collect()
}

fn fuzzy_distance(term: &str) -> u32 {
    match term.chars().count() {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

/// Edit distance (Levenshtein), for the few turns still in the top layer.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// One query word, ready to match: built once per search and used on every segment (a
/// Levenshtein automaton is far costlier to build than to run).
enum Matcher {
    Exact(String),
    Prefix(String),
    Fuzzy(String, fst::automaton::Levenshtein),
}

impl Matcher {
    fn new(word: &str, mode: SearchMode) -> Self {
        match mode {
            SearchMode::Exact => Matcher::Exact(word.to_string()),
            SearchMode::Prefix => Matcher::Prefix(word.to_string()),
            SearchMode::Fuzzy => match fst::automaton::Levenshtein::new(word, fuzzy_distance(word)) {
                Ok(aut) => Matcher::Fuzzy(word.to_string(), aut),
                // a word too long for the automaton's state budget: match it exactly
                Err(_) => Matcher::Exact(word.to_string()),
            },
        }
    }

    /// The same test, on a word not in an index (the top layer's few turns).
    fn matches_word(&self, word: &str) -> bool {
        match self {
            Matcher::Exact(q) => q == word,
            Matcher::Prefix(q) => word.starts_with(q.as_str()),
            Matcher::Fuzzy(q, _) => edit_distance(q, word) <= fuzzy_distance(q) as usize,
        }
    }
}

const META: usize = 40;
const NO_TURN: u64 = u64::MAX;

/// One frozen layer of turns.
#[derive(Clone)]
pub struct HistorySegment {
    terms: fst::Map<Bytes>,
    blob: Bytes,
    first_id: u64,
    last_id: u64,
    n_turns: u64,
    table_at: usize,
    /// Ids of older turns this layer forgets.
    forgets: Vec<u64>,
    /// The file it lives in, once written.
    file: Option<String>,
}

impl std::fmt::Debug for HistorySegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HistorySegment(turns {}..={}, {} turns, {} words, {} bytes)",
               self.first_id, self.last_id, self.n_turns, self.terms.len(), self.size_bytes())
    }
}

impl HistorySegment {
    pub fn turns(&self) -> u64 {
        self.n_turns
    }

    pub fn size_bytes(&self) -> usize {
        self.terms.as_fst().as_bytes().len() + self.blob.len()
    }

    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        let f = segment::read_file(path, segment::KIND_HISTORY)?;
        let mut s = Self::from_parts(f.fst, f.blob).map_err(|e| format!("{}: {}", path.display(), e))?;
        s.file = path.file_name().and_then(|n| n.to_str()).map(str::to_string);
        Ok(s)
    }

    pub fn from_file_bytes(buf: Arc<[u8]>) -> Result<Self, String> {
        let f = segment::decode(buf, segment::KIND_HISTORY)?;
        Self::from_parts(f.fst, f.blob)
    }

    pub fn to_file_bytes(&self) -> Vec<u8> {
        segment::encode(segment::KIND_HISTORY, self.terms.as_fst().as_bytes(), self.blob.as_ref())
    }

    fn from_parts(fst: Bytes, blob: Bytes) -> Result<Self, String> {
        let terms = fst::Map::new(fst).map_err(|e| format!("history index: {}", e))?;
        let b = blob.as_ref();
        if b.len() < META {
            return Err("history segment too short".into());
        }
        let u = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
        let (first_id, last_id, n_turns, table_at, forgets_at) = (u(0), u(1), u(2), u(3) as usize, u(4) as usize);
        let span = if last_id >= first_id { last_id - first_id + 1 } else { 0 };
        let table_end = span.checked_mul(8).and_then(|x| (table_at as u64).checked_add(x));
        if table_end.map_or(true, |e| e > b.len() as u64) || forgets_at > b.len() {
            return Err("history segment tables out of range".into());
        }
        let mut at = forgets_at;
        let n = segment::get_uvarint(b, &mut at).ok_or("history segment forgets")?;
        let mut forgets = Vec::with_capacity(n.min(1 << 20) as usize);
        for _ in 0..n {
            forgets.push(segment::get_uvarint(b, &mut at).ok_or("history segment forgets")?);
        }
        Ok(HistorySegment { terms, blob, first_id, last_id, n_turns, table_at, forgets, file: None })
    }

    fn holds(&self, id: u64) -> bool {
        self.last_id >= self.first_id && id >= self.first_id && id <= self.last_id
    }

    fn turn(&self, id: u64) -> Option<Turn> {
        if !self.holds(id) {
            return None;
        }
        let b = self.blob.as_ref();
        let slot = self.table_at + ((id - self.first_id) as usize) * 8;
        let off = u64::from_le_bytes(b.get(slot..slot + 8)?.try_into().ok()?);
        if off == NO_TURN {
            return None;
        }
        let mut at = off as usize;
        let tid = segment::get_uvarint(b, &mut at)?;
        let at_ms = segment::get_uvarint(b, &mut at)?;
        let speaker = segment::get_str(b, &mut at)?.to_string();
        let text = segment::get_str(b, &mut at)?.to_string();
        (tid == id).then_some(Turn { id, at_ms, speaker, text })
    }

    fn all_turns(&self) -> Vec<Turn> {
        if self.last_id < self.first_id {
            return Vec::new();
        }
        (self.first_id..=self.last_id).filter_map(|id| self.turn(id)).collect()
    }

    fn postings(&self, offset: u64) -> Vec<u64> {
        let b = self.blob.as_ref();
        let mut at = offset as usize;
        let n = segment::get_uvarint(b, &mut at).unwrap_or(0);
        let mut out = Vec::with_capacity(n.min(1 << 16) as usize);
        let mut id = 0u64;
        for _ in 0..n {
            match segment::get_uvarint(b, &mut at) {
                Some(d) => {
                    id = id.wrapping_add(d);
                    out.push(id);
                }
                None => break,
            }
        }
        out
    }

    /// For one query word: every indexed word it matches here, with the turns holding it.
    fn matches(&self, m: &Matcher) -> Vec<(String, Vec<u64>)> {
        let mut out = Vec::new();
        let collect = |k: &[u8], v: u64, out: &mut Vec<(String, Vec<u64>)>| {
            out.push((String::from_utf8_lossy(k).into_owned(), self.postings(v)));
        };
        match m {
            Matcher::Exact(word) => {
                if let Some(v) = self.terms.get(word) {
                    collect(word.as_bytes(), v, &mut out);
                }
            }
            Matcher::Prefix(word) => {
                let aut = fst::Automaton::starts_with(fst::automaton::Str::new(word));
                let mut st = self.terms.search(aut).into_stream();
                while let Some((k, v)) = st.next() {
                    collect(k, v, &mut out);
                }
            }
            Matcher::Fuzzy(_, aut) => {
                let mut st = self.terms.search(aut).into_stream();
                while let Some((k, v)) = st.next() {
                    collect(k, v, &mut out);
                }
            }
        }
        out
    }

    /// Build a segment from turns (ascending ids) and the older ids it forgets.
    fn build(turns: &[Turn], forgets: &[u64]) -> Result<Self, String> {
        let (first_id, last_id) = match (turns.first(), turns.last()) {
            (Some(a), Some(b)) => (a.id, b.id),
            _ => (1, 0),
        };
        if turns.windows(2).any(|w| w[0].id >= w[1].id) {
            return Err("history turns must have ascending ids".into());
        }
        let mut blob = vec![0u8; META];
        let mut offsets: Vec<u64> = Vec::with_capacity(turns.len());
        let mut postings: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for t in turns {
            offsets.push(blob.len() as u64);
            segment::put_uvarint(&mut blob, t.id);
            segment::put_uvarint(&mut blob, t.at_ms);
            segment::put_bytes(&mut blob, t.speaker.as_bytes());
            segment::put_bytes(&mut blob, t.text.as_bytes());
            for w in terms_of(&t.text) {
                postings.entry(w).or_default().push(t.id);
            }
        }
        let table_at = blob.len();
        let span = if last_id >= first_id { (last_id - first_id + 1) as usize } else { 0 };
        let mut table = vec![NO_TURN; span];
        for (t, off) in turns.iter().zip(&offsets) {
            table[(t.id - first_id) as usize] = *off;
        }
        for off in table {
            blob.extend_from_slice(&off.to_le_bytes());
        }
        let mut b = fst::MapBuilder::memory();
        for (word, ids) in &postings {
            let at = blob.len() as u64;
            segment::put_uvarint(&mut blob, ids.len() as u64);
            let mut prev = 0u64;
            for id in ids {
                segment::put_uvarint(&mut blob, id - prev);
                prev = *id;
            }
            b.insert(word.as_bytes(), at).map_err(|e| format!("history index: {}", e))?;
        }
        let forgets_at = blob.len();
        segment::put_uvarint(&mut blob, forgets.len() as u64);
        for id in forgets {
            segment::put_uvarint(&mut blob, *id);
        }
        for (i, v) in [first_id, last_id, turns.len() as u64, table_at as u64, forgets_at as u64].iter().enumerate() {
            blob[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        let fst_bytes = b.into_inner().map_err(|e| format!("history index: {}", e))?;
        Self::from_parts(Bytes::new(Arc::from(fst_bytes.into_boxed_slice())),
                         Bytes::new(Arc::from(blob.into_boxed_slice())))
    }
}

/// A conversation's history: frozen segments (shared) and the turns since the last freeze.
#[derive(Debug, Clone, Default)]
pub struct History {
    segments: Vec<Arc<HistorySegment>>,
    top: Vec<Turn>,
    top_forgets: Vec<u64>,
    next_id: u64,
}

const MANIFEST: &str = "MANIFEST.json";

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn segment_bytes(&self) -> usize {
        self.segments.iter().map(|s| s.size_bytes()).sum()
    }

    fn forgotten(&self) -> HashSet<u64> {
        self.segments.iter().flat_map(|s| s.forgets.iter().copied())
            .chain(self.top_forgets.iter().copied())
            .collect()
    }

    /// Live turns.
    pub fn len(&self) -> usize {
        let forgotten = self.forgotten();
        let frozen: usize = self.segments.iter()
            .map(|s| s.all_turns().iter().filter(|t| !forgotten.contains(&t.id)).count())
            .sum();
        frozen + self.top.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Add a turn; its id.
    pub fn append(&mut self, speaker: &str, text: &str, at_ms: u64) -> u64 {
        let floor = self.segments.iter()
            .filter(|s| s.last_id >= s.first_id)
            .map(|s| s.last_id)
            .chain(self.top.last().map(|t| t.id))
            .max()
            .unwrap_or(0);
        let id = self.next_id.max(floor + 1).max(1);
        self.next_id = id + 1;
        self.top.push(Turn { id, at_ms, speaker: speaker.to_string(), text: text.to_string() });
        id
    }

    /// Forget a turn: no search or read returns it from now on, and the next merge drops it from
    /// the files. False when there is no such live turn.
    pub fn forget(&mut self, id: u64) -> bool {
        if let Some(i) = self.top.iter().position(|t| t.id == id) {
            self.top.remove(i);
            return true;
        }
        if self.forgotten().contains(&id) {
            return false;
        }
        if self.segments.iter().any(|s| s.turn(id).is_some()) {
            self.top_forgets.push(id);
            return true;
        }
        false
    }

    /// A live turn by id.
    pub fn get(&self, id: u64) -> Option<Turn> {
        self.get_unless(id, &self.forgotten())
    }

    fn get_unless(&self, id: u64, forgotten: &HashSet<u64>) -> Option<Turn> {
        if let Some(t) = self.top.iter().find(|t| t.id == id) {
            return Some(t.clone());
        }
        if forgotten.contains(&id) {
            return None;
        }
        // a handful of segments (about log2 of the turns): a linear look is enough
        self.segments.iter().rev().find(|s| s.holds(id)).and_then(|s| s.turn(id))
    }

    /// The newest `n` live turns, newest first.
    pub fn recent(&self, n: usize) -> Vec<Turn> {
        let forgotten = self.forgotten();
        let mut out: Vec<Turn> = self.top.iter().rev().take(n).cloned().collect();
        for s in self.segments.iter().rev() {
            if out.len() >= n {
                break;
            }
            let mut ts = s.all_turns();
            ts.retain(|t| !forgotten.contains(&t.id));
            out.extend(ts.into_iter().rev().take(n - out.len()));
        }
        out
    }

    /// Turns holding every word of `query` (each matched per `mode`), newest first, at most
    /// `limit`.
    pub fn search(&self, query: &str, mode: SearchMode, limit: usize) -> Vec<Hit> {
        let words = terms_of(query);
        if words.is_empty() || limit == 0 {
            return Vec::new();
        }
        let forgotten = self.forgotten();
        let mut found: Option<HashMap<u64, BTreeSet<String>>> = None;
        for w in &words {
            let matcher = Matcher::new(w, mode);
            let mut ids: HashMap<u64, BTreeSet<String>> = HashMap::new();
            for s in &self.segments {
                for (term, posting) in s.matches(&matcher) {
                    for id in posting {
                        ids.entry(id).or_default().insert(term.clone());
                    }
                }
            }
            for t in &self.top {
                for term in terms_of(&t.text) {
                    if matcher.matches_word(&term) {
                        ids.entry(t.id).or_default().insert(term);
                    }
                }
            }
            found = Some(match found {
                None => ids,
                Some(prev) => prev.into_iter()
                    .filter_map(|(id, mut terms)| ids.get(&id).map(|more| {
                        terms.extend(more.iter().cloned());
                        (id, terms)
                    }))
                    .collect(),
            });
            if found.as_ref().is_some_and(|f| f.is_empty()) {
                return Vec::new();
            }
        }
        let mut ids: Vec<(u64, BTreeSet<String>)> = found.unwrap_or_default().into_iter()
            .filter(|(id, _)| !forgotten.contains(id))
            .collect();
        ids.sort_by(|a, b| b.0.cmp(&a.0));
        ids.into_iter()
            .filter_map(|(id, terms)| self.get_unless(id, &forgotten).map(|turn| Hit { turn, matched: terms.into_iter().collect() }))
            .take(limit)
            .collect()
    }

    /// The turns since the last freeze become a segment.
    pub fn freeze(&mut self) -> Result<(), String> {
        if self.top.is_empty() && self.top_forgets.is_empty() {
            return Ok(());
        }
        let seg = HistorySegment::build(&self.top, &self.top_forgets)?;
        self.segments.push(Arc::new(seg));
        self.top.clear();
        self.top_forgets.clear();
        Ok(())
    }

    /// Fold segments `range` into one: the turns forgotten anywhere are dropped, and the forgets
    /// that point at older segments are kept.
    fn merge_range(&mut self, range: std::ops::Range<usize>) -> Result<(), String> {
        if range.len() < 2 {
            return Ok(());
        }
        let forgotten = self.forgotten();
        let first = self.segments[range.start].first_id;
        let mut turns = Vec::new();
        let mut keep_forgets = BTreeSet::new();
        for s in &self.segments[range.clone()] {
            turns.extend(s.all_turns().into_iter().filter(|t| !forgotten.contains(&t.id)));
            keep_forgets.extend(s.forgets.iter().copied().filter(|id| *id < first));
        }
        let forgets: Vec<u64> = keep_forgets.into_iter().collect();
        let seg = HistorySegment::build(&turns, &forgets)?;
        self.segments.splice(range, std::iter::once(Arc::new(seg)));
        Ok(())
    }

    /// Everything into one segment: forgotten turns leave the files.
    pub fn merge_all(&mut self) -> Result<(), String> {
        self.freeze()?;
        if self.segments.len() <= 1 && self.segments.iter().all(|s| s.forgets.is_empty()) {
            return Ok(());
        }
        // nothing is older than the whole history, so every forget is applied and none is kept
        let forgotten = self.forgotten();
        let turns: Vec<Turn> = self.segments.iter()
            .flat_map(|s| s.all_turns())
            .filter(|t| !forgotten.contains(&t.id))
            .collect();
        let only = HistorySegment::build(&turns, &[])?;
        self.segments = vec![Arc::new(only)];
        Ok(())
    }

    /// Merge as a binary counter carries: while the newest segment is at least half the size of
    /// the one before it, fold the two. Segment sizes then at least double going back in time, so
    /// a conversation of n turns sits in about log2(n) segments and each turn is rewritten about
    /// log2(n) times in its life (merging "the smallest adjacent pair" instead rewrites the
    /// newest segment on every turn: quadratic). `max` caps the count beyond that (oldest pairs
    /// first).
    pub fn maybe_merge(&mut self, max: usize) -> Result<(), String> {
        let size = |s: &HistorySegment| s.n_turns.max(1);
        while self.segments.len() >= 2 {
            let n = self.segments.len();
            if size(&self.segments[n - 2]) <= 2 * size(&self.segments[n - 1]) {
                self.merge_range(n - 2..n)?;
            } else {
                break;
            }
        }
        let max = max.max(1);
        while self.segments.len() > max {
            self.merge_range(0..2)?;
        }
        Ok(())
    }

    // ── A history directory ─────────────────────────────────────────────

    /// Open a history directory (`MANIFEST.json` and its segment files). A directory without a
    /// manifest is an empty history.
    pub fn open_dir(dir: &Path) -> Result<Self, String> {
        let mpath = dir.join(MANIFEST);
        let raw = match std::fs::read_to_string(&mpath) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(History::new()),
            Err(e) => return Err(format!("{}: {}", mpath.display(), e)),
        };
        let m: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("{}: {}", mpath.display(), e))?;
        let next_id = m.get("next_id").and_then(|v| v.as_u64()).unwrap_or(1);
        let mut segments = Vec::new();
        for name in m.get("segments").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            let name = name.as_str().ok_or_else(|| format!("{}: a segment name is not a string", mpath.display()))?;
            if name.contains(['/', '\\']) || name.contains("..") {
                return Err(format!("{}: segment name {:?} leaves the directory", mpath.display(), name));
            }
            segments.push(Arc::new(HistorySegment::open(&dir.join(name))?));
        }
        let spans: Vec<(u64, u64)> = segments.iter()
            .filter(|s| s.last_id >= s.first_id)
            .map(|s| (s.first_id, s.last_id))
            .collect();
        if spans.windows(2).any(|w| w[0].1 >= w[1].0) {
            return Err(format!("{}: segments out of order", mpath.display()));
        }
        Ok(History { segments, top: Vec::new(), top_forgets: Vec::new(), next_id })
    }

    /// Freeze the top layer, write every segment not yet on disk, then the manifest (atomically:
    /// a reader sees the old history or the new one, never half of one). Files the manifest no
    /// longer names stay until `gc_dir`.
    pub fn save_dir(&mut self, dir: &Path) -> Result<(), String> {
        self.freeze()?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        for s in self.segments.iter_mut() {
            if s.file.is_some() {
                continue;
            }
            let bytes = s.to_file_bytes();
            let tag = &blake3::hash(&bytes).to_hex()[..12];
            let name = format!("seg-{:016x}-{:016x}-{}.cubeseg", s.first_id, s.last_id, tag);
            segment::write_atomic(&dir.join(&name), &bytes).map_err(|e| format!("{}: {}", name, e))?;
            Arc::make_mut(s).file = Some(name);
        }
        let manifest = serde_json::json!({
            "version": 1,
            "next_id": self.next_id.max(1),
            "segments": self.segments.iter().map(|s| s.file.clone().unwrap_or_default()).collect::<Vec<_>>(),
        });
        segment::write_atomic(&dir.join(MANIFEST), serde_json::to_string_pretty(&manifest).unwrap().as_bytes())
            .map_err(|e| format!("{}: {}", MANIFEST, e))
    }

    /// Remove the segment files of `dir` its manifest no longer names (merged away, or holding a
    /// forgotten turn). Returns the names removed.
    pub fn gc_dir(dir: &Path) -> Result<Vec<String>, String> {
        let live: HashSet<String> = Self::open_dir(dir)?.segments.iter().filter_map(|s| s.file.clone()).collect();
        let mut removed = Vec::new();
        for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {}", dir.display(), e))? {
            let e = e.map_err(|e| e.to_string())?;
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("seg-") && name.ends_with(".cubeseg") && !live.contains(&name) {
                std::fs::remove_file(e.path()).map_err(|err| format!("{}: {}", name, err))?;
                removed.push(name);
            }
        }
        removed.sort();
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn talk() -> History {
        let mut h = History::new();
        h.append("user", "Who invented the printing press?", 1);
        h.append("cubby", "Johannes Gutenberg, around 1440, in Mainz.", 2);
        h.append("user", "And what did Gutenberg print first?", 3);
        h.append("cubby", "The Gutenberg Bible.", 4);
        h
    }

    fn ids(hits: &[Hit]) -> Vec<u64> {
        hits.iter().map(|h| h.turn.id).collect()
    }

    #[test]
    fn search_is_the_same_frozen_or_not() {
        for freeze in [false, true] {
            let mut h = talk();
            if freeze {
                h.freeze().unwrap();
            }
            assert_eq!(ids(&h.search("gutenberg", SearchMode::Exact, 10)), vec![4, 3, 2]);
            assert_eq!(ids(&h.search("Gutenberg print", SearchMode::Exact, 10)), vec![3]);
            assert_eq!(ids(&h.search("guten", SearchMode::Prefix, 10)), vec![4, 3, 2]);
            assert_eq!(ids(&h.search("gutenburg", SearchMode::Fuzzy, 10)), vec![4, 3, 2]);
            assert!(h.search("gutenburg", SearchMode::Exact, 10).is_empty());
            assert_eq!(h.search("printing", SearchMode::Exact, 10)[0].matched, vec!["printing"]);
            assert_eq!(ids(&h.search("print", SearchMode::Prefix, 10)), vec![3, 1]);
            assert_eq!(ids(&h.search("gutenberg", SearchMode::Exact, 2)), vec![4, 3]);
        }
    }

    #[test]
    fn turns_span_segments_and_merging_keeps_them() {
        let mut h = History::new();
        for i in 0..40u64 {
            h.append(if i % 2 == 0 { "user" } else { "cubby" }, &format!("turn number {i} about topic{}", i % 5), i);
            h.freeze().unwrap();
        }
        assert_eq!(h.segment_count(), 40);
        h.maybe_merge(4).unwrap();
        assert!(h.segment_count() <= 4);
        assert_eq!(h.len(), 40);
        assert_eq!(ids(&h.search("topic3", SearchMode::Exact, 100)), vec![39, 34, 29, 24, 19, 14, 9, 4]);
        assert_eq!(h.get(17).unwrap().text, "turn number 16 about topic1");
        assert_eq!(h.recent(3).iter().map(|t| t.id).collect::<Vec<_>>(), vec![40, 39, 38]);
    }

    #[test]
    fn a_forgotten_turn_is_gone_at_once_and_from_the_files_after_a_merge() {
        let mut h = talk();
        h.freeze().unwrap();
        assert!(h.forget(2));
        assert!(!h.forget(2), "already forgotten");
        assert!(h.get(2).is_none());
        assert_eq!(ids(&h.search("gutenberg", SearchMode::Exact, 10)), vec![4, 3]);
        h.merge_all().unwrap();
        assert_eq!(h.segment_count(), 1);
        let bytes = h.segments[0].to_file_bytes();
        let hay = String::from_utf8_lossy(&bytes);
        assert!(!hay.contains("Mainz"), "the forgotten turn's text is not in the merged file");
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn a_history_directory_round_trips_and_gc_removes_old_files() {
        let dir = std::env::temp_dir().join(format!("cubelang-history-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut h = talk();
        h.save_dir(&dir).unwrap();
        let mut back = History::open_dir(&dir).unwrap();
        assert_eq!(ids(&back.search("gutenberg", SearchMode::Exact, 10)), vec![4, 3, 2]);
        let id = back.append("user", "Thanks!", 5);
        assert_eq!(id, 5);
        back.forget(2);
        back.save_dir(&dir).unwrap();
        back.merge_all().unwrap();
        back.save_dir(&dir).unwrap();
        let removed = History::gc_dir(&dir).unwrap();
        assert_eq!(removed.len(), 2, "{removed:?}");
        let again = History::open_dir(&dir).unwrap();
        assert_eq!(again.len(), 4);
        assert!(again.get(2).is_none());
        assert_eq!(again.append_preview(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    impl History {
        fn append_preview(&self) -> u64 {
            let mut c = self.clone();
            c.append("x", "y", 0)
        }
    }

    #[test]
    fn edit_distance_is_levenshtein() {
        assert_eq!(edit_distance("gutenberg", "gutenburg"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}
