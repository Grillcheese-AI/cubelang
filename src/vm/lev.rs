//! A Levenshtein automaton over an `fst` index, by the dynamic-programming row (2026-10-31).
//!
//! `fst::automaton::Levenshtein` (fst 0.4.7) misses keys at distance 2 that hold several multi-byte
//! characters: "eéonset ünniauéüo" is one edit from "eéonseq ünniauéüo", and its DFA at distance 2 finds
//! nothing (at distance 1 it does; without the accents it does). Found by the knowledge store's typo lookup
//! answering differently from a scan (`tests/knowledge_fuzzy.rs`). This one carries the row of edit
//! distances between the query and the key read so far, one character at a time (UTF-8 bytes are gathered
//! until a character is whole), and stops a branch of the index as soon as every cell of the row is over the
//! distance. Correct by construction; no state budget.

use fst::Automaton;

/// Keys within `distance` edits (Levenshtein, on Unicode characters) of `query`.
pub struct Lev {
    query: Vec<char>,
    distance: u32,
}

impl Lev {
    pub fn new(query: &str, distance: u32) -> Self {
        Lev { query: query.chars().collect(), distance }
    }
}

/// The row so far, and the bytes of a character not yet whole. `None`: no key below can match.
#[derive(Clone, Debug)]
pub struct LevState {
    row: Vec<u32>,
    pending: [u8; 4],
    n: u8,
}

fn utf8_len(first: u8) -> u8 {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

impl Automaton for Lev {
    type State = Option<LevState>;

    fn start(&self) -> Self::State {
        Some(LevState { row: (0..=self.query.len() as u32).collect(), pending: [0; 4], n: 0 })
    }

    fn is_match(&self, state: &Self::State) -> bool {
        state.as_ref().is_some_and(|s| s.n == 0 && *s.row.last().unwrap() <= self.distance)
    }

    fn can_match(&self, state: &Self::State) -> bool {
        state.is_some()
    }

    fn accept(&self, state: &Self::State, byte: u8) -> Self::State {
        let s = state.as_ref()?;
        let mut next = s.clone();
        next.pending[next.n as usize] = byte;
        next.n += 1;
        if next.n < utf8_len(next.pending[0]) {
            return Some(next);
        }
        let c = std::str::from_utf8(&next.pending[..next.n as usize]).ok()?.chars().next()?;
        next.n = 0;
        let prev = &s.row;
        let mut row = Vec::with_capacity(prev.len());
        row.push(prev[0] + 1);
        for (j, q) in self.query.iter().enumerate() {
            let v = (prev[j] + u32::from(*q != c)).min(prev[j + 1] + 1).min(row[j] + 1);
            row.push(v);
        }
        if row.iter().all(|&v| v > self.distance) {
            return None;
        }
        next.row = row;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fst::{IntoStreamer, Set, Streamer};

    fn found(keys: &[&str], q: &str, d: u32) -> Vec<String> {
        let mut keys = keys.to_vec();
        keys.sort();
        let set = Set::from_iter(keys).unwrap();
        let mut st = set.search(Lev::new(q, d)).into_stream();
        let mut out = Vec::new();
        while let Some(k) = st.next() {
            out.push(String::from_utf8_lossy(k).into_owned());
        }
        out
    }

    #[test]
    fn accented_keys_at_two_edits_are_found() {
        // fst 0.4.7's own automaton finds nothing here at distance 2
        assert_eq!(found(&["eéonset ünniauéüo"], "eéonseq ünniauéüo", 2), vec!["eéonset ünniauéüo"]);
        assert_eq!(found(&["gutenberg", "gutenburg", "goldsmith"], "gutenbirg", 1), vec!["gutenberg", "gutenburg"]);
        assert!(found(&["gutenberg"], "gutenbxxg", 1).is_empty());
        assert_eq!(found(&["mainz", "maïnz"], "mainz", 0), vec!["mainz"]);
    }
}
