//! `wdstore cues <dir> <out dir>`: which kinds each word of the hot items' values goes with, and which classes are
//! fictional (CubbyLLM order 0055, step 3: the cue table its order-0037 rule reads, "a building cannot win a Nobel
//! Prize", and the fictional items its namesakes set aside).
//!
//! Two passes over the hot chunks. The first keeps each entity's English label (else `mul`) as words, the subclass
//! edges (best-rank P279), and which properties are plain strings (`wikibase:String`, not external ids). The second,
//! per item with a kind (best-rank P31 items): the words of its best-rank values -- an item value's label, English or
//! `mul` text, a plain string -- against each of its kinds, counted once per item. Words as CubbyLLM's `hdc_cues.words`
//! cuts them: lower case, runs of ASCII letters and digits, three characters or more, not a number, not a stop word.
//! Written to the out dir:
//!   word_kinds.tsv   word, Q-id of the kind, how many items of that kind carry a value with that word
//!   fictional.tsv    every class under "fictional entity" (Q14897293) by subclass of, itself included
//!   meta.tsv         the store's snapshot, items, typed items, words, pairs, fictional classes, seconds
//! Every pair counts; nothing is cut by frequency here (the reader's own threshold decides what a cue rules out).
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;

use crate::model::{Eid, NameKind, Value};
use crate::store::Store;

const STOP: &[&str] = &["the", "a", "an", "of", "and", "or", "in", "on", "at", "to", "for", "from", "by", "with", "into",
    "over", "under", "about", "as", "is", "are", "was", "were", "be", "been", "his", "her", "its", "their", "this", "that",
    "these", "those", "what", "which", "who", "whom", "whose", "where", "when", "why", "how", "did", "does", "do", "has",
    "have", "had", "not", "no", "de", "la", "le", "les", "du", "des", "del", "della", "di", "da", "von", "van", "der",
    "den", "das", "el", "los", "las", "et", "und"];
const FICTIONAL: &str = "Q14897293";                      // fictional entity

/// `hdc_cues.words`: the content words of a text.
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.to_lowercase().chars().chain(std::iter::once(' ')) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() { cur.push(c); continue; }
        if cur.len() >= 3 && !cur.bytes().all(|b| b.is_ascii_digit()) && !STOP.contains(&cur.as_str()) { out.push(cur.clone()); }
        cur.clear();
    }
    out
}

struct Dict { ids: HashMap<String, u32>, words: Vec<String> }
impl Dict {
    fn id(&mut self, w: String) -> u32 {
        if let Some(&i) = self.ids.get(&w) { return i; }
        let i = self.words.len() as u32;
        self.ids.insert(w.clone(), i);
        self.words.push(w);
        i
    }
}

fn err<E: std::fmt::Display>(e: E) -> String { e.to_string() }

pub fn cues(st: &Store, out: &str) -> Result<String, String> {
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(out).map_err(err)?;
    let n = st.hot_chunks();
    let mut dict = Dict { ids: HashMap::new(), words: Vec::new() };
    let mut label: HashMap<u64, (u32, u8)> = HashMap::new();       // entity -> its label's words in `arena`
    let mut arena: Vec<u32> = Vec::new();
    let mut sup: HashMap<u64, Vec<u64>> = HashMap::new();          // class -> its superclasses
    let mut strings: HashSet<u32> = HashSet::new();                // properties whose values are plain strings
    for k in 0..n {
        for e in st.hot_chunk(k)? {
            let Some(id) = e.id else { continue };
            if e.datatype.as_deref() == Some("wikibase:String") {
                if let Some(p) = id.text().strip_prefix('P').and_then(|x| x.parse().ok()) { strings.insert(p); }
            }
            let l = ["en", "mul"].iter().find_map(|g| e.names.iter().find(|x| x.kind == NameKind::Label && x.lang == *g));
            if let Some(l) = l {
                let ws: Vec<u32> = words(&l.text).into_iter().take(255).map(|w| dict.id(w)).collect();
                if !ws.is_empty() { label.insert(id.0, (arena.len() as u32, ws.len() as u8)); arena.extend(ws); }
            }
            for s in &e.statements {
                if s.pid == 279 && s.best && s.rank as u8 != 0 { if let Value::Entity(c) = &s.value { sup.entry(id.0).or_default().push(c.0); } }
            }
        }
        if k % 100 == 0 || k + 1 == n {
            eprintln!("  cues, labels: chunk {}/{n}, {} labelled, {} words, {} s", k + 1, label.len(), dict.words.len(), t0.elapsed().as_secs());
        }
    }
    let mut pairs: HashMap<(u32, u64), u32> = HashMap::new();
    let (mut items, mut typed) = (0u64, 0u64);
    let mut ws: Vec<u32> = Vec::new();
    for k in 0..n {
        for e in st.hot_chunk(k)? {
            items += 1;
            let kinds: Vec<u64> = e.statements.iter().filter(|s| s.pid == 31 && s.best && s.rank as u8 != 0)
                .filter_map(|s| if let Value::Entity(c) = &s.value { Some(c.0) } else { None }).collect();
            if kinds.is_empty() { continue; }
            typed += 1;
            ws.clear();
            for s in e.statements.iter().filter(|s| s.best && s.rank as u8 != 0) {
                match &s.value {
                    Value::Entity(v) => if let Some(&(at, len)) = label.get(&v.0) { ws.extend_from_slice(&arena[at as usize..at as usize + len as usize]); },
                    Value::Mono { text, lang } if lang == "en" || lang == "mul" => for w in words(text) { ws.push(dict.id(w)); },
                    Value::Str(text) if strings.contains(&s.pid) => for w in words(text) { ws.push(dict.id(w)); },
                    _ => {}
                }
            }
            ws.sort_unstable();
            ws.dedup();
            for &w in &ws { for &c in &kinds { *pairs.entry((w, c)).or_insert(0) += 1; } }
        }
        if k % 50 == 0 || k + 1 == n {
            eprintln!("  cues, pairs: chunk {}/{n}, {items} items, {typed} typed, {} pairs, {} s", k + 1, pairs.len(), t0.elapsed().as_secs());
        }
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(format!("{out}/word_kinds.tsv")).map_err(err)?);
    for ((w, c), m) in &pairs { writeln!(f, "{}\t{}\t{m}", dict.words[*w as usize], Eid(*c).text()).map_err(err)?; }
    f.flush().map_err(err)?;
    // the fictional classes: everything that reaches Q14897293 by subclass of
    let mut sub: HashMap<u64, Vec<u64>> = HashMap::new();
    for (c, ps) in &sup { for p in ps { sub.entry(*p).or_default().push(*c); } }
    let root = Eid::parse(FICTIONAL).ok_or("the fictional root")?.0;
    let (mut fic, mut q) = (HashSet::from([root]), VecDeque::from([root]));
    while let Some(c) = q.pop_front() {
        for &d in sub.get(&c).map(|v| v.as_slice()).unwrap_or(&[]) { if fic.insert(d) { q.push_back(d); } }
    }
    let mut fv: Vec<u64> = fic.into_iter().collect();
    fv.sort_unstable();
    std::fs::write(format!("{out}/fictional.tsv"), fv.iter().map(|c| Eid(*c).text() + "\n").collect::<String>()).map_err(err)?;
    let snap = crate::snapshot::at(&st.dir, None)?.hash;
    let secs = t0.elapsed().as_secs();
    std::fs::write(format!("{out}/meta.tsv"), format!("snapshot\t{snap}\nitems\t{items}\ntyped\t{typed}\nwords\t{}\npairs\t{}\nfictional\t{}\nseconds\t{secs}\n",
        dict.words.len(), pairs.len(), fv.len())).map_err(err)?;
    Ok(format!("{items} hot items, {typed} typed; {} words; {} (word, kind) pairs; {} fictional classes; {secs} s", dict.words.len(), pairs.len(), fv.len()))
}
