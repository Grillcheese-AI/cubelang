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

use std::io::{BufWriter, Seek, SeekFrom, Write};

use fst::{IntoStreamer, Map, MapBuilder, Streamer};
use unicode_normalization::UnicodeNormalization;

use crate::codec::{entry, Dict, R, W};
use crate::model::*;

pub const MAGIC: &[u8; 8] = b"WDNAMES\0";
pub const VERSION: u32 = 3;
pub const TYPO_KEYS: usize = 64;

pub fn exact_key(s: &str) -> String {
    s.replace('\0', " ").split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
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

fn order(e: Eid) -> u64 { crate::reverse::order(e) }

/// An entity's name rows, exact then folded: key, a 0 byte, the entity, the kind, the language (bytes that sort as
/// (key, entity, kind, language); a key never holds a 0 byte).
pub fn rows(e: &Entity) -> [Vec<Vec<u8>>; 2] {
    let mut out = [Vec::new(), Vec::new()];
    let Some(id) = e.id else { return out };
    for n in &e.names {
        if n.kind == NameKind::Description { continue; }
        for (k, key) in [exact_key(&n.text), folded_key(&n.text)].into_iter().enumerate() {
            if key.is_empty() { continue; }
            let mut r = Vec::with_capacity(key.len() + 14);
            r.extend_from_slice(key.as_bytes()); r.push(0); r.extend_from_slice(&order(id).to_be_bytes());
            r.push((n.kind == NameKind::Alias) as u8); r.extend_from_slice(n.lang.as_bytes());
            out[k].push(r);
        }
    }
    for v in out.iter_mut() { v.sort(); v.dedup(); }
    out
}

fn split(row: &[u8]) -> Result<(&[u8], u64, u8, &str), String> {
    let z = row.iter().position(|&b| b == 0).ok_or("a name row without its key's end")?;
    if row.len() < z + 10 { return Err("a short name row".into()); }
    Ok((&row[..z], u64::from_be_bytes(row[z + 1..z + 9].try_into().unwrap()), row[z + 9],
        std::str::from_utf8(&row[z + 10..]).map_err(|_| "a name row's language")?))
}

/// Writes the two FSTs and `names.post` from rows given in byte order: all the exact rows (`which` 0), then all
/// the folded ones (1).
pub struct NamesWriter {
    fsts: [Option<MapBuilder<BufWriter<std::fs::File>>>; 2], post: BufWriter<std::fs::File>, pos: u64, dict: Dict,
    which: usize, key: Option<Vec<u8>>, group: Vec<(u64, u8, String)>, pub keys: [u64; 2],
}

impl NamesWriter {
    pub fn create(dir: &str) -> Result<NamesWriter, String> {
        let open = |f: &str| std::fs::File::create(format!("{dir}/{f}")).map(BufWriter::new).map_err(|e| format!("{f}: {e}"));
        let mut post = open("names.post")?;
        post.write_all(&[0u8; 20]).map_err(|e| e.to_string())?;
        let fst = |f: &str| -> Result<MapBuilder<BufWriter<std::fs::File>>, String> { MapBuilder::new(open(f)?).map_err(|e| e.to_string()) };
        Ok(NamesWriter { fsts: [Some(fst("names_exact.fst")?), Some(fst("names_folded.fst")?)], post, pos: 20,
                         dict: Dict::default(), which: 0, key: None, group: vec![], keys: [0, 0] })
    }

    fn close(&mut self) -> Result<(), String> {
        let Some(k) = self.key.take() else { return Ok(()) };
        self.fsts[self.which].as_mut().unwrap().insert(&k, self.pos).map_err(|e| e.to_string())?;
        self.keys[self.which] += 1;
        let mut w = W::default();
        w.u(self.group.len() as u64);
        let mut prev = 0u64;
        for (o, kind, lang) in self.group.drain(..) { w.u((o - prev) << 1 | kind as u64); prev = o; let l = self.dict.id(&lang); w.u(l); }
        self.post.write_all(&w.b).map_err(|e| e.to_string())?;
        self.pos += w.b.len() as u64;
        Ok(())
    }

    pub fn add(&mut self, which: usize, row: &[u8]) -> Result<(), String> {
        if which < self.which { return Err("exact rows after folded ones".into()); }
        if which != self.which { self.close()?; self.which = which; }
        let (key, o, kind, lang) = split(row)?;
        if self.key.as_deref() != Some(key) { self.close()?; self.key = Some(key.to_vec()); }
        self.group.push((o, kind, lang.to_string()));
        Ok(())
    }

    pub fn finish(mut self) -> Result<[u64; 2], String> {
        self.close()?;
        for f in self.fsts.iter_mut() { f.take().unwrap().finish().map_err(|e| e.to_string())?; }
        let dict_at = self.pos;
        let mut w = W::default();
        self.dict.write(&mut w);
        self.post.write_all(&w.b).map_err(|e| e.to_string())?;
        let mut f = self.post.into_inner().map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut h = Vec::with_capacity(20);
        h.extend_from_slice(MAGIC); h.extend_from_slice(&VERSION.to_le_bytes()); h.extend_from_slice(&dict_at.to_le_bytes());
        f.write_all(&h).map_err(|e| e.to_string())?;
        Ok(self.keys)
    }
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

    /// Every row of one key set (0 exact, 1 folded), as `rows` writes them, in byte order (for the check).
    pub fn all_rows(&self, which: usize) -> Result<Vec<Vec<u8>>, String> {
        let m = if which == 0 { &self.exact } else { &self.folded };
        let mut s = m.stream();
        let mut v = Vec::new();
        while let Some((k, off)) = s.next() {
            let key = String::from_utf8_lossy(k).to_string();
            let mut hits = Vec::new();
            self.list(off, &key, How::Exact, &mut hits)?;
            for h in hits {
                let mut r = k.to_vec(); r.push(0); r.extend_from_slice(&order(h.entity).to_be_bytes());
                r.push((h.kind == NameKind::Alias) as u8); r.extend_from_slice(h.lang.as_bytes());
                v.push(r);
            }
        }
        Ok(v)
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
