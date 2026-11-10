//! The name index: every label and alias the store keeps (EN, FR, `mul`), under two keys each, in two finite-state
//! transducers (BurntSushi's `fst`): `names_exact.fst` (lowercase, spaces collapsed) and `names_folded.fst` (accents
//! and punctuation dropped as well). Each key maps to its postings in `names.post`: the entities that bear the name,
//! in id order, each with the name's language and kind (label or alias).
//!
//! A lookup tries exact, then folded, then a bounded typo path over the folded keys (one edit from 5 characters,
//! two from 9, none below 5); the first stage with a hit answers, with every candidate it found, in id order.
//! Nothing is ordered by fame (sitelinks, page views, id size): the caller filters by its own constraints, or asks.
//! A typo path that matches more than `TYPO_KEYS` keys returns `too_many` instead of a guess.
//!
//!   names.post: "WDNAMES\0" | u32 version | u64 dict_at | postings... | dictionary (languages)
//!   a posting list: count, then per entry varint (gap << 1 | kind) over (kind << 36 | number), then the language

use fst::{IntoStreamer, Map, MapBuilder, Streamer};
use unicode_normalization::UnicodeNormalization;

use crate::codec::{entry, Dict, R, W};
use crate::model::*;

pub const MAGIC: &[u8; 8] = b"WDNAMES\0";
pub const VERSION: u32 = 2;
pub const TYPO_KEYS: usize = 64;

pub fn exact_key(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

pub fn folded_key(s: &str) -> String {
    let base: String = s.nfd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).collect();
    let mut o = String::with_capacity(base.len());
    for c in base.to_lowercase().chars() { o.push(if c.is_alphanumeric() { c } else { ' ' }); }
    o.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How { Exact, Folded, Typo(u32) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameHit { pub entity: Eid, pub lang: String, pub kind: NameKind, pub key: String, pub how: How }

#[derive(Debug, PartialEq, Eq)]
pub enum Found { Hits(Vec<NameHit>), TooMany(usize), None }

fn order(e: Eid) -> u64 { let (k, n, _) = e.parts(); (k as u64) << 36 | n }

type Row = (String, u64, NameKind, String); // key, entity order, kind, lang

fn rows(entities: &[Entity], key: fn(&str) -> String) -> Vec<Row> {
    let mut v = Vec::new();
    for e in entities {
        let Some(id) = e.id else { continue };
        for n in &e.names {
            if n.kind == NameKind::Description { continue; }
            let k = key(&n.text);
            if !k.is_empty() { v.push((k, order(id), n.kind, n.lang.clone())); }
        }
    }
    v.sort(); v.dedup();
    v
}

/// (names_exact.fst, names_folded.fst, names.post).
pub fn write(entities: &[Entity]) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
    let (mut post, mut d) = (W { b: vec![0; 20] }, Dict::default());
    let mut fsts = Vec::new();
    for key in [exact_key as fn(&str) -> String, folded_key] {
        let rs = rows(entities, key);
        let mut b = MapBuilder::memory();
        let mut i = 0;
        while i < rs.len() {
            let k = &rs[i].0;
            let j = i + rs[i..].iter().take_while(|r| &r.0 == k).count();
            b.insert(k.as_bytes(), post.b.len() as u64).map_err(|e| e.to_string())?;
            post.u((j - i) as u64);
            let mut prev = 0u64;
            for r in &rs[i..j] { post.u((r.1 - prev) << 1 | (r.2 == NameKind::Alias) as u64); prev = r.1; let l = d.id(&r.3); post.u(l); }
            i = j;
        }
        fsts.push(b.into_inner().map_err(|e| e.to_string())?);
    }
    let dict_at = post.b.len() as u64;
    d.write(&mut post);
    post.b[..8].copy_from_slice(MAGIC); post.b[8..12].copy_from_slice(&VERSION.to_le_bytes());
    post.b[12..20].copy_from_slice(&dict_at.to_le_bytes());
    let folded = fsts.pop().unwrap();
    Ok((fsts.pop().unwrap(), folded, post.b))
}

pub struct Names { exact: Map<Vec<u8>>, folded: Map<Vec<u8>>, post: Vec<u8>, dict: Vec<String> }

impl Names {
    pub fn open(exact: Vec<u8>, folded: Vec<u8>, post: Vec<u8>) -> Result<Names, String> {
        if post.len() < 20 || &post[..8] != MAGIC { return Err("not a names file".into()); }
        if u32::from_le_bytes(post[8..12].try_into().unwrap()) != VERSION { return Err("names version".into()); }
        let dict = Dict::read(&mut R::at(&post, u64::from_le_bytes(post[12..20].try_into().unwrap()) as usize))?;
        Ok(Names { exact: Map::new(exact).map_err(|e| e.to_string())?, folded: Map::new(folded).map_err(|e| e.to_string())?, post, dict })
    }

    pub fn keys(&self) -> (usize, usize) { (self.exact.len(), self.folded.len()) }

    fn list(&self, off: u64, key: &str, how: How, out: &mut Vec<NameHit>) -> Result<(), String> {
        let mut r = R::at(&self.post, off as usize);
        let mut cur = 0u64;
        for _ in 0..r.u()? {
            let x = r.u()?;
            cur += x >> 1;
            let entity = Eid::from_parts((cur >> 36) as u8, cur & ((1 << 36) - 1), 0).ok_or("bad entity")?;
            let lang = entry(&self.dict, r.u()?)?;
            out.push(NameHit { entity, lang, kind: if x & 1 == 1 { NameKind::Alias } else { NameKind::Label }, key: key.into(), how });
        }
        Ok(())
    }

    /// Every entity a name may mean: exact, else folded, else within the typo bound; in id order.
    pub fn lookup(&self, name: &str) -> Result<Found, String> {
        let mut out = Vec::new();
        let ek = exact_key(name);
        if let Some(off) = self.exact.get(ek.as_bytes()) { self.list(off, &ek, How::Exact, &mut out)?; }
        let fk = folded_key(name);
        if out.is_empty() {
            if let Some(off) = self.folded.get(fk.as_bytes()) { self.list(off, &fk, How::Folded, &mut out)?; }
        }
        let chars = fk.chars().count();
        let dist = if chars >= 9 { 2 } else if chars >= 5 { 1 } else { 0 };
        if out.is_empty() && dist > 0 {
            let lev = fst::automaton::Levenshtein::new(&fk, dist).map_err(|e| e.to_string())?;
            let mut s = self.folded.search(&lev).into_stream();
            let mut keys = Vec::new();
            while let Some((k, off)) = s.next() {
                keys.push((String::from_utf8_lossy(k).to_string(), off));
                if keys.len() > TYPO_KEYS { return Ok(Found::TooMany(keys.len())); }
            }
            for (k, off) in keys {
                let d = crate::names::distance(&fk, &k);
                self.list(off, &k, How::Typo(d), &mut out)?;
            }
        }
        out.sort_by_key(|h| (order(h.entity), h.key.clone(), h.lang.clone(), h.kind));
        Ok(if out.is_empty() { Found::None } else { Found::Hits(out) })
    }
}

/// Levenshtein distance in characters (for the hit's record; the automaton already bounded it).
pub fn distance(a: &str, b: &str) -> u32 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<u32> = (0..=b.len() as u32).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i as u32 + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + (ca != cb) as u32).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}
