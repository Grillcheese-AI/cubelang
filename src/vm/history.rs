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
//!
//! A history directory can be sealed (2026-10-24, `*_in` with a `Vault`): every segment and the
//! manifest are sealed to one identity and signed by it (`cubelang::seal`), so the files say
//! nothing to whoever copies them, and nobody without the key can slip a forged turn in.
//!
//! Tags and meta (2026-10-25): a turn can also carry tags ("session:2026-10-24-ring-of-keys",
//! "doc:result.md") and meta (the caller's own data, JSON by convention). Tags are indexed beside
//! the words, under a mark no word can hold, so `search_in` finds "these words, in this session"
//! from the index; meta is stored and returned, never searched. A store built on a history (a
//! conversation per person, a model's transcripts) files its records this way. A segment holding no
//! tags or meta is written exactly as before (kind 2), so plain conversations are unchanged on disk.
//!
//! One writer at a time: `lock_dir` holds the directory's `LOCK` file for as long as a process may
//! write it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use fst::{IntoStreamer, Streamer};

use crate::vm::knowledge::normalize;
use crate::vm::segment::{self, Bytes, Vault};

/// One utterance.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Turn {
    pub id: u64,
    pub at_ms: u64,
    pub speaker: String,
    pub text: String,
    /// The labels the turn is filed under, each once; found exactly (`search_in`), never by a word.
    pub tags: Vec<String>,
    /// The caller's own data about the turn (JSON by convention): kept and returned, not searched.
    pub meta: String,
}

/// The most tags one turn may carry, and the longest tag.
pub const MAX_TAGS: usize = 32;
pub const MAX_TAG_BYTES: usize = 200;

/// The mark a tag's index entry starts with. Indexed words are letters and digits only
/// (`normalize`), so no word query can reach a tag, and no tag can pass for a word.
const TAG_MARK: char = '\u{1}';

fn tag_term(tag: &str) -> String {
    format!("{}{}", TAG_MARK, tag)
}

/// A tag a turn may carry: 1 to `MAX_TAG_BYTES` bytes, no control characters, no space at either end.
pub fn check_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() || tag.len() > MAX_TAG_BYTES {
        return Err(format!("a tag is 1 to {} bytes", MAX_TAG_BYTES));
    }
    if tag.chars().any(char::is_control) || tag.trim() != tag {
        return Err(format!("tag {:?}: no control characters, no space at either end", tag));
    }
    Ok(())
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
const KINDS: [u8; 2] = [segment::KIND_HISTORY, segment::KIND_HISTORY_TAGGED];
const NO_TURN: u64 = u64::MAX;

/// One frozen layer of turns.
#[derive(Clone)]
pub struct HistorySegment {
    terms: fst::Map<Bytes>,
    blob: Bytes,
    /// Its records carry tags and meta (file kind 3).
    tagged: bool,
    first_id: u64,
    last_id: u64,
    n_turns: u64,
    table_at: usize,
    /// Ids of older turns this layer forgets.
    forgets: Vec<u64>,
    /// The file it lives in, once written.
    file: Option<String>,
    /// BLAKE3 of its plaintext file bytes, once written to (or read from) a sealed directory: the
    /// sealed manifest names each segment by file AND content.
    sum: Option<String>,
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
        let f = segment::read_file_any(path, &KINDS)?;
        let mut s = Self::from_parts(f.fst, f.blob, f.kind == segment::KIND_HISTORY_TAGGED)
            .map_err(|e| format!("{}: {}", path.display(), e))?;
        s.file = path.file_name().and_then(|n| n.to_str()).map(str::to_string);
        Ok(s)
    }

    pub fn from_file_bytes(buf: Arc<[u8]>) -> Result<Self, String> {
        let f = segment::decode_any(buf, &KINDS)?;
        Self::from_parts(f.fst, f.blob, f.kind == segment::KIND_HISTORY_TAGGED)
    }

    pub fn to_file_bytes(&self) -> Vec<u8> {
        let kind = if self.tagged { segment::KIND_HISTORY_TAGGED } else { segment::KIND_HISTORY };
        segment::encode(kind, self.terms.as_fst().as_bytes(), self.blob.as_ref())
    }

    fn from_parts(fst: Bytes, blob: Bytes, tagged: bool) -> Result<Self, String> {
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
        Ok(HistorySegment { terms, blob, tagged, first_id, last_id, n_turns, table_at, forgets, file: None, sum: None })
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
        let (mut meta, mut tags) = (String::new(), Vec::new());
        if self.tagged {
            meta = segment::get_str(b, &mut at)?.to_string();
            let n = segment::get_uvarint(b, &mut at)?;
            if n > MAX_TAGS as u64 {
                return None;
            }
            for _ in 0..n {
                tags.push(segment::get_str(b, &mut at)?.to_string());
            }
        }
        (tid == id).then_some(Turn { id, at_ms, speaker, text, tags, meta })
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

    /// The turns here carrying `tag`.
    fn tag_ids(&self, tag: &str) -> Vec<u64> {
        self.terms.get(tag_term(tag)).map(|v| self.postings(v)).unwrap_or_default()
    }

    /// Every tag here starting with `prefix`, with the turns carrying it.
    fn tags_with_prefix(&self, prefix: &str) -> Vec<(String, Vec<u64>)> {
        let start = tag_term(prefix);
        let aut = fst::Automaton::starts_with(fst::automaton::Str::new(&start));
        let mut st = self.terms.search(aut).into_stream();
        let mut out = Vec::new();
        while let Some((k, v)) = st.next() {
            out.push((String::from_utf8_lossy(&k[TAG_MARK.len_utf8()..]).into_owned(), self.postings(v)));
        }
        out
    }

    /// For one query word: every indexed word it matches here, with the turns holding it.
    fn matches(&self, m: &Matcher) -> Vec<(String, Vec<u64>)> {
        let mut out = Vec::new();
        let collect = |k: &[u8], v: u64, out: &mut Vec<(String, Vec<u64>)>| {
            // a tag's entry is never a word's match (a typo of a word could otherwise reach one)
            if k.first() == Some(&(TAG_MARK as u8)) {
                return;
            }
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
        let tagged = turns.iter().any(|t| !t.tags.is_empty() || !t.meta.is_empty());
        let mut blob = vec![0u8; META];
        let mut offsets: Vec<u64> = Vec::with_capacity(turns.len());
        let mut postings: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for t in turns {
            offsets.push(blob.len() as u64);
            segment::put_uvarint(&mut blob, t.id);
            segment::put_uvarint(&mut blob, t.at_ms);
            segment::put_bytes(&mut blob, t.speaker.as_bytes());
            segment::put_bytes(&mut blob, t.text.as_bytes());
            if tagged {
                segment::put_bytes(&mut blob, t.meta.as_bytes());
                segment::put_uvarint(&mut blob, t.tags.len() as u64);
                for tag in &t.tags {
                    segment::put_bytes(&mut blob, tag.as_bytes());
                }
            }
            for w in terms_of(&t.text) {
                postings.entry(w).or_default().push(t.id);
            }
            for tag in &t.tags {
                postings.entry(tag_term(tag)).or_default().push(t.id);
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
                         Bytes::new(Arc::from(blob.into_boxed_slice())), tagged)
    }
}

/// A conversation's history: frozen segments (shared) and the turns since the last freeze.
#[derive(Debug, Clone, Default)]
pub struct History {
    segments: Vec<Arc<HistorySegment>>,
    top: Vec<Turn>,
    top_forgets: Vec<u64>,
    next_id: u64,
    /// How many times the directory it came from was saved: a host that remembers the highest
    /// generation it served can refuse an older copy put back in its place (a rollback).
    generation: u64,
}

const MANIFEST: &str = "MANIFEST.json";
const MANIFEST_SEALED: &str = "MANIFEST.cubeseal";
const LOCK: &str = "LOCK";

/// The one writer's hold on a history directory (`History::lock_dir`); released when dropped.
#[derive(Debug)]
pub struct DirLock {
    _file: std::fs::File,
}

/// A file name a manifest may name: one plain name inside the directory.
fn check_segment_name(mpath: &Path, name: &str, sealed: bool) -> Result<(), String> {
    let ext = if sealed { ".cubeseal" } else { ".cubeseg" };
    if name.contains(['/', '\\']) || name.contains("..") || !name.starts_with("seg-") || !name.ends_with(ext) {
        return Err(format!("{}: segment name {:?} is not a segment file of this directory", mpath.display(), name));
    }
    Ok(())
}

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
        self.append_tagged(speaker, text, at_ms, &[], "").expect("a turn without tags is always valid")
    }

    /// Add a turn filed under `tags` (each once; see `check_tag`) with the caller's `meta`; its id.
    pub fn append_tagged(&mut self, speaker: &str, text: &str, at_ms: u64, tags: &[String], meta: &str)
                         -> Result<u64, String> {
        let mut own: Vec<String> = Vec::with_capacity(tags.len());
        for t in tags {
            check_tag(t)?;
            if !own.contains(t) {
                own.push(t.clone());
            }
        }
        if own.len() > MAX_TAGS {
            return Err(format!("at most {} tags on a turn", MAX_TAGS));
        }
        let floor = self.segments.iter()
            .filter(|s| s.last_id >= s.first_id)
            .map(|s| s.last_id)
            .chain(self.top.last().map(|t| t.id))
            .max()
            .unwrap_or(0);
        let id = self.next_id.max(floor + 1).max(1);
        self.next_id = id + 1;
        self.top.push(Turn { id, at_ms, speaker: speaker.to_string(), text: text.to_string(), tags: own, meta: meta.to_string() });
        Ok(id)
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
        self.search_in(query, mode, &[], limit)
    }

    /// Turns carrying every one of `tags` and holding every word of `query` (either may be empty,
    /// not both), newest first, at most `limit`.
    pub fn search_in(&self, query: &str, mode: SearchMode, tags: &[String], limit: usize) -> Vec<Hit> {
        let words = terms_of(query);
        if (words.is_empty() && tags.is_empty()) || limit == 0 {
            return Vec::new();
        }
        let forgotten = self.forgotten();
        let mut found: Option<HashMap<u64, BTreeSet<String>>> = None;
        let and = |found: Option<HashMap<u64, BTreeSet<String>>>, ids: HashMap<u64, BTreeSet<String>>| match found {
            None => ids,
            Some(prev) => prev.into_iter()
                .filter_map(|(id, mut terms)| ids.get(&id).map(|more| {
                    terms.extend(more.iter().cloned());
                    (id, terms)
                }))
                .collect(),
        };
        // the tags first: a session's tag is usually the narrowest part of the question
        for tag in tags {
            let mut ids: HashMap<u64, BTreeSet<String>> = HashMap::new();
            for s in &self.segments {
                for id in s.tag_ids(tag) {
                    ids.entry(id).or_default();
                }
            }
            for t in &self.top {
                if t.tags.iter().any(|x| x == tag) {
                    ids.entry(t.id).or_default();
                }
            }
            found = Some(and(found, ids));
            if found.as_ref().is_some_and(|f| f.is_empty()) {
                return Vec::new();
            }
        }
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
            found = Some(and(found, ids));
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

    /// Every tag starting with `prefix` that a live turn carries, with how many do, in tag order
    /// ("session:" lists a store's sessions).
    pub fn tags(&self, prefix: &str) -> Vec<(String, usize)> {
        let forgotten = self.forgotten();
        let mut by: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
        for s in &self.segments {
            for (tag, ids) in s.tags_with_prefix(prefix) {
                by.entry(tag).or_default().extend(ids.into_iter().filter(|id| !forgotten.contains(id)));
            }
        }
        for t in &self.top {
            for tag in t.tags.iter().filter(|x| x.starts_with(prefix)) {
                by.entry(tag.clone()).or_default().insert(t.id);
            }
        }
        by.into_iter().filter(|(_, ids)| !ids.is_empty()).map(|(tag, ids)| (tag, ids.len())).collect()
    }

    /// Hold `dir`'s `LOCK` file: while the lock lives, no other process can take it, so one process
    /// writes the directory at a time. The operating system lets go when the process ends, so a
    /// crash leaves no stale lock behind.
    pub fn lock_dir(dir: &Path) -> Result<DirLock, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        let path = dir.join(LOCK);
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path)
            .map_err(|e| format!("{}: {}", path.display(), e))?;
        match file.try_lock() {
            Ok(()) => Ok(DirLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(format!("{}: another process is writing this history", dir.display())),
            Err(std::fs::TryLockError::Error(e)) => Err(format!("{}: {}", path.display(), e)),
        }
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

    /// How many saves the directory had seen when this history was read from it (see `generation`
    /// on the struct).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Open a history directory (`MANIFEST.json` and its segment files). A directory without a
    /// manifest is an empty history. A sealed directory opens only with its vault (`open_dir_in`).
    pub fn open_dir(dir: &Path) -> Result<Self, String> {
        Self::open_dir_in(dir, None)
    }

    /// Refuse a directory that holds both manifests, or the other kind than `vault` asks for.
    fn check_dir_kind(dir: &Path, vault: Option<&dyn Vault>) -> Result<(), String> {
        let plain = dir.join(MANIFEST).exists();
        let sealed = dir.join(MANIFEST_SEALED).exists();
        match (plain, sealed, vault.is_some()) {
            (true, true, _) => Err(format!("{}: holds both a plain and a sealed manifest; refusing to guess which \
                                            is the history (`cubelang history seal` finishes a sealing)", dir.display())),
            (true, false, true) => Err(format!("{}: this history is not sealed (open it without a key, or seal it \
                                                 first: `cubelang history seal`)", dir.display())),
            (false, true, false) => Err(format!("{}: this history is sealed: it opens only with its key (--key)", dir.display())),
            _ => Ok(()),
        }
    }

    /// Open a history directory: sealed with `vault` (`MANIFEST.cubeseal` and `seg-*.cubeseal`, each
    /// file sealed to and signed by the vault's identity, the manifest naming every segment by file
    /// and BLAKE3 of its content), or plain with `None`.
    pub fn open_dir_in(dir: &Path, vault: Option<&dyn Vault>) -> Result<Self, String> {
        Self::check_dir_kind(dir, vault)?;
        Self::read_dir(dir, vault)
    }

    fn read_dir(dir: &Path, vault: Option<&dyn Vault>) -> Result<Self, String> {
        let mpath = dir.join(if vault.is_some() { MANIFEST_SEALED } else { MANIFEST });
        let raw = match std::fs::read(&mpath) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(History::new()),
            Err(e) => return Err(format!("{}: {}", mpath.display(), e)),
        };
        let raw = match vault {
            Some(v) => v.open_file(MANIFEST_SEALED, &raw)?,
            None => raw,
        };
        let m: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| format!("{}: {}", mpath.display(), e))?;
        let next_id = m.get("next_id").and_then(|v| v.as_u64()).unwrap_or(1);
        let generation = m.get("generation").and_then(|v| v.as_u64()).unwrap_or(0);
        let mut segments = Vec::new();
        for entry in m.get("segments").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            let seg = match vault {
                None => {
                    let name = entry.as_str().ok_or_else(|| format!("{}: a segment name is not a string", mpath.display()))?;
                    check_segment_name(&mpath, name, false)?;
                    HistorySegment::open(&dir.join(name))?
                }
                Some(v) => {
                    let (Some(name), Some(sum)) = (entry.get("file").and_then(|x| x.as_str()),
                                                   entry.get("blake3").and_then(|x| x.as_str())) else {
                        return Err(format!("{}: a segment entry needs its file and blake3", mpath.display()));
                    };
                    check_segment_name(&mpath, name, true)?;
                    let path = dir.join(name);
                    let sealed = std::fs::read(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
                    let plain = v.open_file(name, &sealed)?;
                    if blake3::hash(&plain).to_hex().as_str() != sum {
                        return Err(format!("{}: {} is not the segment the manifest names", mpath.display(), name));
                    }
                    let mut s = HistorySegment::from_file_bytes(Arc::from(plain.into_boxed_slice()))
                        .map_err(|e| format!("{}: {}", path.display(), e))?;
                    s.file = Some(name.to_string());
                    s.sum = Some(sum.to_string());
                    s
                }
            };
            segments.push(Arc::new(seg));
        }
        let spans: Vec<(u64, u64)> = segments.iter()
            .filter(|s| s.last_id >= s.first_id)
            .map(|s| (s.first_id, s.last_id))
            .collect();
        if spans.windows(2).any(|w| w[0].1 >= w[1].0) {
            return Err(format!("{}: segments out of order", mpath.display()));
        }
        Ok(History { segments, top: Vec::new(), top_forgets: Vec::new(), next_id, generation })
    }

    /// Freeze the top layer, write every segment not yet on disk, then the manifest (atomically:
    /// a reader sees the old history or the new one, never half of one). Files the manifest no
    /// longer names stay until `gc_dir`.
    pub fn save_dir(&mut self, dir: &Path) -> Result<(), String> {
        self.save_dir_in(dir, None)
    }

    /// `save_dir`, sealed with `vault` or plain with `None`. Sealed segment files are named at
    /// random (`seg-<128 random bits>.cubeseal`), so a name says nothing about the turns in it.
    pub fn save_dir_in(&mut self, dir: &Path, vault: Option<&dyn Vault>) -> Result<(), String> {
        self.freeze()?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        Self::check_dir_kind(dir, vault)?;
        self.write_dir(dir, vault)
    }

    fn write_dir(&mut self, dir: &Path, vault: Option<&dyn Vault>) -> Result<(), String> {
        let ext = if vault.is_some() { ".cubeseal" } else { ".cubeseg" };
        for s in self.segments.iter_mut() {
            // on disk already, in this directory's kind (a plain segment being sealed is rewritten)
            if s.file.as_deref().is_some_and(|f| f.ends_with(ext)) && (vault.is_none() || s.sum.is_some()) {
                continue;
            }
            let bytes = s.to_file_bytes();
            let sum = blake3::hash(&bytes).to_hex().to_string();
            let (name, out) = match vault {
                None => (format!("seg-{:016x}-{:016x}-{}.cubeseg", s.first_id, s.last_id, &sum[..12]), bytes),
                Some(v) => {
                    let name = format!("seg-{:016x}{:016x}.cubeseal", rand::random::<u64>(), rand::random::<u64>());
                    let sealed = v.seal_file(&name, &bytes)?;
                    (name, sealed)
                }
            };
            segment::write_atomic(&dir.join(&name), &out).map_err(|e| format!("{}: {}", name, e))?;
            let m = Arc::make_mut(s);
            m.file = Some(name);
            m.sum = Some(sum);
        }
        let generation = self.generation + 1;
        let segments: Vec<serde_json::Value> = self.segments.iter().map(|s| {
            let file = s.file.clone().unwrap_or_default();
            match vault {
                None => serde_json::Value::String(file),
                Some(_) => serde_json::json!({ "file": file, "blake3": s.sum.clone().unwrap_or_default() }),
            }
        }).collect();
        let manifest = serde_json::json!({
            "version": 1,
            "next_id": self.next_id.max(1),
            "generation": generation,
            "segments": segments,
        });
        let text = serde_json::to_string_pretty(&manifest).unwrap();
        match vault {
            None => segment::write_atomic(&dir.join(MANIFEST), text.as_bytes())
                .map_err(|e| format!("{}: {}", MANIFEST, e))?,
            Some(v) => segment::write_atomic(&dir.join(MANIFEST_SEALED), &v.seal_file(MANIFEST_SEALED, text.as_bytes())?)
                .map_err(|e| format!("{}: {}", MANIFEST_SEALED, e))?,
        }
        self.generation = generation;
        Ok(())
    }

    /// Remove the segment files of `dir` its manifest no longer names (merged away, or holding a
    /// forgotten turn). Returns the names removed.
    pub fn gc_dir(dir: &Path) -> Result<Vec<String>, String> {
        Self::gc_dir_in(dir, None)
    }

    /// `gc_dir` for a sealed (`vault`) or plain directory.
    pub fn gc_dir_in(dir: &Path, vault: Option<&dyn Vault>) -> Result<Vec<String>, String> {
        let ext = if vault.is_some() { ".cubeseal" } else { ".cubeseg" };
        let live: HashSet<String> = Self::open_dir_in(dir, vault)?.segments.iter().filter_map(|s| s.file.clone()).collect();
        let mut removed = Vec::new();
        for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {}", dir.display(), e))? {
            let e = e.map_err(|e| e.to_string())?;
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("seg-") && name.ends_with(ext) && !live.contains(&name) {
                std::fs::remove_file(e.path()).map_err(|err| format!("{}: {}", name, err))?;
                removed.push(name);
            }
        }
        removed.sort();
        Ok(removed)
    }

    /// Seal a plain history directory in place (2026-10-24): every segment is written again sealed,
    /// the sealed manifest is written, then the plain manifest and segment files are removed (an
    /// interrupted run is finished by running it again). Returns the plain files removed. Copies
    /// made before -- backups, a sync folder, the disk's free blocks -- are out of its reach.
    pub fn seal_dir(dir: &Path, vault: &dyn Vault) -> Result<Vec<String>, String> {
        let plain = dir.join(MANIFEST).exists();
        let sealed = dir.join(MANIFEST_SEALED).exists();
        if !plain {
            return Err(if sealed {
                format!("{}: already sealed", dir.display())
            } else {
                format!("{}: no history here (no {})", dir.display(), MANIFEST)
            });
        }
        if sealed {
            // an earlier run wrote the sealed manifest (atomically) and stopped before removing the
            // plain files: check the sealed history opens whole before removing anything
            Self::read_dir(dir, Some(vault))?;
        } else {
            let mut h = Self::read_dir(dir, None)?;
            h.write_dir(dir, Some(vault))?;
            Self::read_dir(dir, Some(vault))?;
        }
        let mut removed = Vec::new();
        for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {}", dir.display(), e))? {
            let e = e.map_err(|e| e.to_string())?;
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("seg-") && name.ends_with(".cubeseg") {
                std::fs::remove_file(e.path()).map_err(|err| format!("{}: {}", name, err))?;
                removed.push(name);
            }
        }
        std::fs::remove_file(dir.join(MANIFEST)).map_err(|e| format!("{}: {}", MANIFEST, e))?;
        removed.push(MANIFEST.to_string());
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

    fn tags(t: &[&str]) -> Vec<String> {
        t.iter().map(|x| x.to_string()).collect()
    }

    /// A store's records: two sessions' model calls, filed by tag, with meta.
    fn store() -> History {
        let mut h = History::new();
        h.append_tagged("model-a", "The ring of keys is enough for a small house", 1, &tags(&["session:s1", "call"]), r#"{"cost":0.01}"#).unwrap();
        h.append_tagged("model-b", "A ring of keys gets lost; a key box is safer", 2, &tags(&["session:s1", "call"]), "").unwrap();
        h.append_tagged("model-a", "Keys for a factory need a registry", 3, &tags(&["session:s2", "call"]), "").unwrap();
        h.append("user", "what about keys in general", 4);
        h
    }

    #[test]
    fn tags_narrow_a_search_and_meta_comes_back() {
        for freeze in [false, true] {
            let mut h = store();
            if freeze {
                h.freeze().unwrap();
            }
            assert_eq!(ids(&h.search("keys", SearchMode::Exact, 10)), vec![4, 3, 2, 1]);
            assert_eq!(ids(&h.search_in("keys", SearchMode::Exact, &tags(&["session:s1"]), 10)), vec![2, 1]);
            assert_eq!(ids(&h.search_in("", SearchMode::Exact, &tags(&["session:s2"]), 10)), vec![3], "tags alone");
            assert_eq!(ids(&h.search_in("ring", SearchMode::Exact, &tags(&["session:s1", "call"]), 10)), vec![2, 1]);
            assert!(h.search_in("ring", SearchMode::Exact, &tags(&["session:s2"]), 10).is_empty());
            assert!(h.search_in("", SearchMode::Exact, &[], 10).is_empty(), "neither words nor tags");
            let first = h.get(1).unwrap();
            assert_eq!(first.tags, tags(&["session:s1", "call"]));
            assert_eq!(first.meta, r#"{"cost":0.01}"#);
            assert!(h.get(4).unwrap().tags.is_empty());
            assert_eq!(h.tags("session:"), vec![("session:s1".to_string(), 2), ("session:s2".to_string(), 1)]);
            assert_eq!(h.tags(""), vec![("call".to_string(), 3), ("session:s1".to_string(), 2), ("session:s2".to_string(), 1)]);
        }
    }

    #[test]
    fn no_word_query_reaches_a_tag() {
        let mut h = History::new();
        h.append_tagged("x", "nothing to see", 1, &tags(&["abcd", "session"]), "").unwrap();
        h.freeze().unwrap();
        // "abcd" is a tag, never a word: not found exactly, by prefix, or as a typo ("abce")
        for (q, mode) in [("abcd", SearchMode::Exact), ("abc", SearchMode::Prefix), ("abce", SearchMode::Fuzzy),
                          ("session", SearchMode::Exact), ("sessio", SearchMode::Fuzzy)] {
            assert!(h.search(q, mode, 10).is_empty(), "{q} {mode:?}");
        }
        assert_eq!(ids(&h.search_in("", SearchMode::Exact, &tags(&["abcd"]), 10)), vec![1]);
        // nor can a tag be made to look like a word
        assert!(check_tag("").is_err() && check_tag(" x").is_err() && check_tag("a\u{1}b").is_err()
                && check_tag(&"t".repeat(MAX_TAG_BYTES + 1)).is_err());
        assert!(h.append_tagged("x", "y", 2, &tags(&["a\nb"]), "").is_err());
        let many: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        assert!(h.append_tagged("x", "y", 2, &many, "").is_err());
        assert_eq!(h.append_tagged("x", "y", 2, &tags(&["a", "a"]), "").unwrap(), 2);
        assert_eq!(h.get(2).unwrap().tags, tags(&["a"]), "each tag once");
    }

    #[test]
    fn forgetting_and_merging_keep_tags_right() {
        let mut h = store();
        h.freeze().unwrap();
        assert!(h.forget(2));
        assert_eq!(h.tags("session:"), vec![("session:s1".to_string(), 1), ("session:s2".to_string(), 1)]);
        assert_eq!(ids(&h.search_in("", SearchMode::Exact, &tags(&["session:s1"]), 10)), vec![1]);
        for id in [1, 3] {
            h.forget(id);
        }
        h.merge_all().unwrap();
        assert!(h.tags("session:").is_empty(), "a forgotten session's tags are gone with it");
        let hay = String::from_utf8_lossy(&h.segments[0].to_file_bytes()).into_owned();
        assert!(!hay.contains("session:s1") && !hay.contains("registry"));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn a_plain_history_keeps_its_old_file_kind_and_tagged_ones_round_trip() {
        let mut plain = talk();
        plain.freeze().unwrap();
        let bytes = plain.segments[0].to_file_bytes();
        assert_eq!(bytes[9], segment::KIND_HISTORY, "no tags, no meta: written exactly as before");
        let mut h = store();
        h.freeze().unwrap();
        let bytes = h.segments[0].to_file_bytes();
        assert_eq!(bytes[9], segment::KIND_HISTORY_TAGGED);
        let back = HistorySegment::from_file_bytes(Arc::from(bytes.into_boxed_slice())).unwrap();
        assert_eq!(back.turn(1).unwrap().meta, r#"{"cost":0.01}"#);
        assert_eq!(back.tag_ids("session:s1"), vec![1, 2]);
        // a directory mixing both kinds (an old conversation that later got tagged turns) opens whole
        let dir = std::env::temp_dir().join(format!("cubelang-history-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut mixed = talk();
        mixed.save_dir(&dir).unwrap();
        mixed.append_tagged("model-a", "ring of keys", 9, &tags(&["session:s9"]), "{}").unwrap();
        mixed.save_dir(&dir).unwrap();
        let again = History::open_dir(&dir).unwrap();
        assert_eq!(again.segment_count(), 2);
        assert_eq!(ids(&again.search_in("keys", SearchMode::Exact, &tags(&["session:s9"]), 5)), vec![5]);
        assert_eq!(ids(&again.search("gutenberg", SearchMode::Exact, 5)), vec![4, 3, 2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_writer_at_a_time() {
        let dir = std::env::temp_dir().join(format!("cubelang-history-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        let held = History::lock_dir(&dir).unwrap();
        assert!(History::lock_dir(&dir).unwrap_err().contains("another process is writing"));
        drop(held);
        let again = History::lock_dir(&dir).unwrap();
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_distance_is_levenshtein() {
        assert_eq!(edit_distance("gutenberg", "gutenburg"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}
