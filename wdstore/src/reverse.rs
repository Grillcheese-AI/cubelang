//! The reverse index (`reverse.wds`): for each (property, value entity), the entities whose statement of that
//! property has that value, in id order, each with its statement's rank and best-rank flag (so a caller drops
//! deprecated statements without reading the entities). Main values only; qualifiers' values are not indexed.
//! Written as a stream from rows sorted by `spill` (more rows than memory holds).
//!
//!   "WDREVRS\0" | u32 version | u64 keys | u64 keys_at | postings... | keys
//!   postings: per key, varint (gap << 3 | best << 2 | rank) over the subjects' (kind << 36 | number)
//!   keys: (pid u32, value id u64, postings offset u64, count u32)* sorted by (pid, value id), 24 bytes each
//!
//! A subject twice under one key (two statements, same property and value) is listed twice: a gap of 0.

use std::io::{BufWriter, Seek, SeekFrom, Write};

use crate::codec::{Buf, R, W};
use crate::model::*;

pub const MAGIC: &[u8; 8] = b"WDREVRS\0";
pub const VERSION: u32 = 3;
const HEAD: usize = 8 + 4 + 8 + 8;
const KEY: usize = 4 + 8 + 8 + 4;

pub fn order(e: Eid) -> u64 { let (k, n, _) = e.parts(); (k as u64) << 36 | n }
pub fn unorder(x: u64) -> Option<Eid> { Eid::from_parts((x >> 36) as u8, x & ((1 << 36) - 1), 0) }

/// An entity's rows: one per statement whose main value is an entity; bytes that sort as (pid, value, subject).
pub fn rows(e: &Entity) -> Vec<Vec<u8>> {
    let Some(id) = e.id else { return vec![] };
    e.statements.iter().filter_map(|s| match s.value {
        Value::Entity(o) => {
            let mut r = Vec::with_capacity(21);
            r.extend_from_slice(&s.pid.to_be_bytes()); r.extend_from_slice(&o.0.to_be_bytes());
            r.extend_from_slice(&order(id).to_be_bytes()); r.push((s.best as u8) << 2 | s.rank as u8);
            Some(r)
        }
        _ => None,
    }).collect()
}

/// Writes `reverse.wds` from rows given in byte order.
pub struct RevWriter { f: BufWriter<std::fs::File>, pos: u64, keys: Vec<u8>, cur: Option<(u32, u64, u64, u32)>, prev: u64, nkeys: u64 }

impl RevWriter {
    pub fn create(path: &str) -> Result<RevWriter, String> {
        let mut f = BufWriter::new(std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?);
        f.write_all(&[0u8; HEAD]).map_err(|e| e.to_string())?;
        Ok(RevWriter { f, pos: HEAD as u64, keys: Vec::new(), cur: None, prev: 0, nkeys: 0 })
    }

    fn close_key(&mut self) {
        if let Some((p, v, off, n)) = self.cur.take() {
            self.keys.extend_from_slice(&p.to_le_bytes()); self.keys.extend_from_slice(&v.to_le_bytes());
            self.keys.extend_from_slice(&off.to_le_bytes()); self.keys.extend_from_slice(&n.to_le_bytes());
            self.nkeys += 1;
        }
    }

    pub fn add(&mut self, row: &[u8]) -> Result<(), String> {
        if row.len() != 21 { return Err("a reverse row of the wrong size".into()); }
        let pid = u32::from_be_bytes(row[0..4].try_into().unwrap());
        let val = u64::from_be_bytes(row[4..12].try_into().unwrap());
        let subj = u64::from_be_bytes(row[12..20].try_into().unwrap());
        if !matches!(self.cur, Some((p, v, _, _)) if p == pid && v == val) {
            self.close_key();
            self.cur = Some((pid, val, self.pos, 0)); self.prev = 0;
        }
        if subj < self.prev { return Err("reverse rows out of order".into()); }
        let mut w = W::default();
        w.u((subj - self.prev) << 3 | row[20] as u64);
        self.f.write_all(&w.b).map_err(|e| e.to_string())?;
        self.pos += w.b.len() as u64; self.prev = subj;
        if let Some(c) = self.cur.as_mut() { c.3 += 1; }
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64, String> {
        self.close_key();
        let keys_at = self.pos;
        self.f.write_all(&self.keys).map_err(|e| e.to_string())?;
        let mut f = self.f.into_inner().map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut h = Vec::with_capacity(HEAD);
        h.extend_from_slice(MAGIC); h.extend_from_slice(&VERSION.to_le_bytes());
        h.extend_from_slice(&self.nkeys.to_le_bytes()); h.extend_from_slice(&keys_at.to_le_bytes());
        f.write_all(&h).map_err(|e| e.to_string())?;
        Ok(self.nkeys)
    }
}

/// One hit: the subject, its statement's rank and whether that statement is of the best rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit { pub subject: Eid, pub rank: Rank, pub best: bool }

pub struct Reverse { b: Buf, keys: usize, keys_at: usize }

impl Reverse {
    pub fn open(b: Buf) -> Result<Reverse, String> {
        if b.len() < HEAD || &b[..8] != MAGIC { return Err("not a reverse index".into()); }
        if u32::from_le_bytes(b[8..12].try_into().unwrap()) != VERSION { return Err("reverse index version".into()); }
        let mut r = R::at(&b, 12);
        let (keys, keys_at) = (r.u64le()? as usize, r.u64le()? as usize);
        if keys_at + KEY * keys != b.len() { return Err("reverse index truncated".into()); }
        Ok(Reverse { b, keys, keys_at })
    }
    pub fn keys(&self) -> usize { self.keys }

    fn key(&self, k: usize) -> (u32, u64, usize, u32) {
        let o = self.keys_at + KEY * k;
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
        let mut r = R::at(&self.b, off);
        let (mut cur, mut out) = (0u64, Vec::with_capacity(n as usize));
        for _ in 0..n {
            let x = r.u()?;
            cur += x >> 3;
            let rank = match x & 3 { 0 => Rank::Deprecated, 1 => Rank::Normal, 2 => Rank::Preferred, _ => return Err("rank".into()) };
            out.push(Hit { subject: unorder(cur).ok_or("bad subject")?, rank, best: x & 4 != 0 });
        }
        Ok(out)
    }

    /// Every row, as `rows` writes them, in byte order, read from the file as it goes (for the check).
    pub fn rows_iter(&self) -> impl Iterator<Item = Result<Vec<u8>, String>> + '_ {
        let mut buf: std::collections::VecDeque<Result<Vec<u8>, String>> = Default::default();
        let mut k = 0;
        std::iter::from_fn(move || {
            while buf.is_empty() && k < self.keys {
                let (p, val, off, n) = self.key(k);
                k += 1;
                match self.list(off, n) {
                    Err(e) => buf.push_back(Err(e)),
                    Ok(hits) => for h in hits {
                        let mut r = Vec::with_capacity(21);
                        r.extend_from_slice(&p.to_be_bytes()); r.extend_from_slice(&val.to_be_bytes());
                        r.extend_from_slice(&order(h.subject).to_be_bytes()); r.push((h.best as u8) << 2 | h.rank as u8);
                        buf.push_back(Ok(r));
                    },
                }
            }
            buf.pop_front()
        })
    }
}
