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

use std::collections::HashMap;

/// One grounded fact. `source` is not optional: an unattributed fact is the
/// confabulation this store exists to prevent.
#[derive(Debug, Clone)]
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

#[derive(Debug, Default)]
pub struct KnowledgeStore {
    /// normalized key -> the facts carrying that name.
    ///
    /// A Vec, not a single Fact: ambiguity is real (`Mercury` is a planet, an
    /// element and a god). Several facts under one key is not a failure — it is
    /// the program's cue to ASK, which is the only honest move when only the
    /// caller knows which was meant.
    facts: HashMap<String, Vec<Fact>>,
}

impl KnowledgeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.facts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    /// Register a fact under a surface form. Repeated keys accumulate.
    pub fn insert(&mut self, key: &str, text: impl Into<String>, source: impl Into<String>) {
        let k = normalize(key);
        if k.is_empty() {
            return;
        }
        self.facts.entry(k).or_default().push(Fact {
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
        match self.facts.get(&c).cloned() {
            Some(f) => {
                self.facts.insert(a, f);
                true
            }
            None => false,
        }
    }

    /// Exact lookup. An empty slice means "no fact carries this name" — the
    /// honest gap. Never a nearest neighbour.
    pub fn lookup(&self, mention: &str) -> &[Fact] {
        self.facts
            .get(&normalize(mention))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
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
}
