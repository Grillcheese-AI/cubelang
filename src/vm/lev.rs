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

/// The widest band: 2 * `knowledge::MAX_EDITS` + 1.
const BAND: usize = 5;

/// Keys within `distance` edits (Levenshtein, on Unicode characters) of `query`.
///
/// Banded (CubbyLLM order 0046, the typo index, 2026-11-04): a cell of the row whose column is more than
/// `distance` from the row's own index is over the distance whatever follows (|i - j| edits at least), so only
/// the 2 * distance + 1 cells around the diagonal are carried, capped at distance + 1. The state is then a
/// few bytes copied, not a row allocated for every byte the index reads. The same answers as the full row.
pub struct Lev {
    query: Vec<char>,
    distance: u32,
}

impl Lev {
    /// `distance` is capped at 2 (the band's width).
    pub fn new(query: &str, distance: u32) -> Self {
        Lev { query: query.chars().collect(), distance: distance.min(((BAND - 1) / 2) as u32) }
    }

    fn inf(&self) -> u8 {
        self.distance as u8 + 1
    }

    fn width(&self) -> usize {
        2 * self.distance as usize + 1
    }
}

/// Characters read so far (`i`), the band around column `i` (cell `k` is column `i + k - distance`), and
/// the bytes of a character not yet whole. `None`: no key below can match.
#[derive(Clone, Copy, Debug)]
pub struct LevState {
    i: u32,
    band: [u8; BAND],
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
        let (d, m, inf) = (self.distance as i64, self.query.len() as i64, self.inf());
        let mut band = [inf; BAND];
        for (k, cell) in band.iter_mut().enumerate().take(self.width()) {
            let j = k as i64 - d;
            if (0..=m).contains(&j) {
                *cell = (j as u8).min(inf);
            }
        }
        Some(LevState { i: 0, band, pending: [0; 4], n: 0 })
    }

    fn is_match(&self, state: &Self::State) -> bool {
        let Some(s) = state else { return false };
        let k = self.query.len() as i64 - s.i as i64 + self.distance as i64;
        s.n == 0 && (0..self.width() as i64).contains(&k) && u32::from(s.band[k as usize]) <= self.distance
    }

    fn can_match(&self, state: &Self::State) -> bool {
        state.is_some()
    }

    fn accept(&self, state: &Self::State, byte: u8) -> Self::State {
        let s = state.as_ref()?;
        let mut next = *s;
        next.pending[next.n as usize] = byte;
        next.n += 1;
        if next.n < utf8_len(next.pending[0]) {
            return Some(next);
        }
        let c = std::str::from_utf8(&next.pending[..next.n as usize]).ok()?.chars().next()?;
        next.n = 0;
        let (d, m, inf, w) = (self.distance as i64, self.query.len() as i64, self.inf(), self.width());
        let i = s.i as i64 + 1;
        let mut band = [inf; BAND];
        for k in 0..w {
            let j = i + k as i64 - d;
            if j < 0 || j > m {
                continue;
            }
            if j == 0 {
                band[k] = (i.min(inf as i64)) as u8;
                continue;
            }
            let diag = s.band[k] + u8::from(self.query[(j - 1) as usize] != c);
            let up = if k + 1 < w { s.band[k + 1] + 1 } else { inf };
            let left = if k > 0 { band[k - 1] + 1 } else { inf };
            band[k] = diag.min(up).min(left).min(inf);
        }
        if band[..w].iter().all(|&v| v > self.distance as u8) {
            return None;
        }
        next.i = i as u32;
        next.band = band;
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
