//! The reverse index (`reverse.wds`): for each (property, value entity), the entities whose statement of that
//! property has that value, in id order, each with its statement's rank and best-rank flag (so a caller drops
//! deprecated statements without reading the entities). Main values only; qualifiers' values are not indexed.
//!
//!   "WDREVRS\0" | u32 version | u64 keys | u64 postings_at
//!   keys: (pid u32, value id u64, postings offset u64, count u32)* sorted by (pid, value id)   -- 24 bytes each
//!   postings: per key, varint (gap << 3 | best << 2 | rank) over the subjects' (kind << 36 | number)
//!
//! A subject twice under one key (two statements, same property and value) is listed twice: a gap of 0.

use crate::codec::{R, W};
use crate::model::*;

pub const MAGIC: &[u8; 8] = b"WDREVRS\0";
pub const VERSION: u32 = 2;
const HEAD: usize = 8 + 4 + 8 + 8;
const KEY: usize = 4 + 8 + 8 + 4;

fn order(e: Eid) -> u64 { let (k, n, _) = e.parts(); (k as u64) << 36 | n }
fn unorder(x: u64) -> Option<Eid> { Eid::from_parts((x >> 36) as u8, x & ((1 << 36) - 1), 0) }

/// One row per statement whose main value is an entity: (pid, value, subject, best << 2 | rank).
pub fn rows(entities: &[Entity]) -> Vec<(u32, Eid, u64, u8)> {
    let mut v = Vec::new();
    for e in entities {
        let Some(id) = e.id else { continue };
        for s in &e.statements {
            if let Value::Entity(o) = s.value { v.push((s.pid, o, order(id), (s.best as u8) << 2 | s.rank as u8)); }
        }
    }
    v.sort();
    v
}

pub fn write(rows: &[(u32, Eid, u64, u8)]) -> Result<Vec<u8>, String> {
    let (mut keys, mut post) = (Vec::new(), W::default());
    let mut i = 0;
    while i < rows.len() {
        let (pid, val) = (rows[i].0, rows[i].1);
        let (start, mut prev, mut n) = (post.b.len() as u64, 0u64, 0u32);
        while i < rows.len() && rows[i].0 == pid && rows[i].1 == val {
            post.u((rows[i].2 - prev) << 3 | rows[i].3 as u64);
            prev = rows[i].2; n += 1; i += 1;
        }
        keys.push((pid, val, start, n));
    }
    let mut b = Vec::with_capacity(HEAD + KEY * keys.len() + post.b.len());
    b.extend_from_slice(MAGIC); b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&(keys.len() as u64).to_le_bytes());
    b.extend_from_slice(&((HEAD + KEY * keys.len()) as u64).to_le_bytes());
    for (p, v, o, n) in keys {
        b.extend_from_slice(&p.to_le_bytes()); b.extend_from_slice(&v.0.to_le_bytes());
        b.extend_from_slice(&o.to_le_bytes()); b.extend_from_slice(&n.to_le_bytes());
    }
    b.extend_from_slice(&post.b);
    Ok(b)
}

/// One hit: the subject, its statement's rank and whether that statement is of the best rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit { pub subject: Eid, pub rank: Rank, pub best: bool }

pub struct Reverse { b: Vec<u8>, keys: usize, postings_at: usize }

impl Reverse {
    pub fn open(b: Vec<u8>) -> Result<Reverse, String> {
        if b.len() < HEAD || &b[..8] != MAGIC { return Err("not a reverse index".into()); }
        if u32::from_le_bytes(b[8..12].try_into().unwrap()) != VERSION { return Err("reverse index version".into()); }
        let mut r = R::at(&b, 12);
        let (keys, postings_at) = (r.u64le()? as usize, r.u64le()? as usize);
        if HEAD + KEY * keys != postings_at || postings_at > b.len() { return Err("reverse index truncated".into()); }
        Ok(Reverse { b, keys, postings_at })
    }
    pub fn bytes(&self) -> &[u8] { &self.b }
    pub fn keys(&self) -> usize { self.keys }

    fn key(&self, k: usize) -> (u32, u64, usize, u32) {
        let o = HEAD + KEY * k;
        let b = &self.b;
        (u32::from_le_bytes(b[o..o + 4].try_into().unwrap()), u64::from_le_bytes(b[o + 4..o + 12].try_into().unwrap()),
         u64::from_le_bytes(b[o + 12..o + 20].try_into().unwrap()) as usize, u32::from_le_bytes(b[o + 20..o + 24].try_into().unwrap()))
    }

    /// Every entity with a statement `pid` = `value`, in id order (at every rank: the caller filters).
    pub fn who(&self, pid: u32, value: Eid) -> Result<Vec<Hit>, String> {
        let want = (pid, value.0);
        let (mut lo, mut hi) = (0, self.keys);
        while lo < hi {
            let m = (lo + hi) / 2;
            let (p, v, off, n) = self.key(m);
            match (p, v).cmp(&want) {
                std::cmp::Ordering::Less => lo = m + 1,
                std::cmp::Ordering::Greater => hi = m,
                std::cmp::Ordering::Equal => return self.list(off, n),
            }
        }
        Ok(vec![])
    }

    fn list(&self, off: usize, n: u32) -> Result<Vec<Hit>, String> {
        let mut r = R::at(&self.b, self.postings_at + off);
        let (mut cur, mut out) = (0u64, Vec::with_capacity(n as usize));
        for _ in 0..n {
            let x = r.u()?;
            cur += x >> 3;
            let rank = match x & 3 { 0 => Rank::Deprecated, 1 => Rank::Normal, 2 => Rank::Preferred, _ => return Err("rank".into()) };
            out.push(Hit { subject: unorder(cur).ok_or("bad subject")?, rank, best: x & 4 != 0 });
        }
        Ok(out)
    }

    /// Every row, back in `rows` order (for the check).
    pub fn all_rows(&self) -> Result<Vec<(u32, Eid, u64, u8)>, String> {
        let mut v = Vec::new();
        for k in 0..self.keys {
            let (p, val, off, n) = self.key(k);
            for h in self.list(off, n)? { v.push((p, Eid(val), order(h.subject), (h.best as u8) << 2 | h.rank as u8)); }
        }
        Ok(v)
    }
}
