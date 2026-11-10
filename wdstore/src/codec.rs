//! Bytes in, bytes out: varints, strings, a file's small dictionary, ids, statement ids, node hashes.
//! Little-endian, no padding, one encoding per value, so the same input always writes the same bytes.

use crate::model::{Eid, NodeHash};

#[derive(Default)]
pub struct W { pub b: Vec<u8> }

const HEX: &[u8; 16] = b"0123456789abcdef";
const HEX_UP: &[u8; 16] = b"0123456789ABCDEF";

fn nib(c: u8) -> Option<u8> {
    match c { b'0'..=b'9' => Some(c - b'0'), b'a'..=b'f' => Some(c - b'a' + 10), b'A'..=b'F' => Some(c - b'A' + 10), _ => None }
}

/// A statement id's case, when it is a UUID in one case (8-4-4-4-12 hex digits): 1 lower, 2 upper; 0 otherwise
/// (stored as written). Digits alone count as lower: written back lower, they are the same text.
pub fn guid_kind(g: &str) -> u8 {
    let b = g.as_bytes();
    if b.len() != 36 || [8, 13, 18, 23].iter().any(|&i| b[i] != b'-') { return 0; }
    let hex: Vec<u8> = b.iter().copied().filter(|&c| c != b'-').collect();
    if hex.len() != 32 || hex.iter().any(|&c| nib(c).is_none()) { return 0; }
    let (lo, up) = (hex.iter().any(|c| c.is_ascii_lowercase()), hex.iter().any(|c| c.is_ascii_uppercase()));
    match (lo, up) { (_, false) => 1, (false, true) => 2, (true, true) => 0 }
}

impl W {
    pub fn u(&mut self, mut x: u64) {
        while x >= 0x80 { self.b.push((x as u8) | 0x80); x >>= 7; }
        self.b.push(x as u8);
    }
    pub fn byte(&mut self, x: u8) { self.b.push(x); }
    pub fn s(&mut self, x: &str) { self.u(x.len() as u64); self.b.extend_from_slice(x.as_bytes()); }
    pub fn raw(&mut self, x: &[u8]) { self.b.extend_from_slice(x); }

    /// An id: (number << 3 | kind << 1 | has a sub-id), then the sub-id (a lexeme's form or sense).
    pub fn eid(&mut self, e: Eid) {
        let (kind, n, sub) = e.parts();
        self.u(n << 3 | (kind as u64) << 1 | (sub != 0) as u64);
        if sub != 0 { self.u(sub); }
    }

    /// A statement id of kind 1 or 2 (`guid_kind`) as its 16 bytes; kind 0 as written.
    pub fn guid(&mut self, g: &str, kind: u8) {
        if kind == 0 { self.s(g); return; }
        let hex: Vec<u8> = g.bytes().filter(|&c| c != b'-').collect();
        for p in hex.chunks(2) { self.byte(nib(p[0]).unwrap() << 4 | nib(p[1]).unwrap()); }
    }

    /// A node hash: lowercase hex stored as its bytes (16 or 20), anything else as written (flag 0x80).
    pub fn hash(&mut self, h: &NodeHash) {
        let hex = &h.0;
        if hex.len() % 2 == 0 && hex.len() < 254 && hex.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
            self.byte((hex.len() / 2) as u8);
            for p in hex.chunks(2) { self.byte(nib(p[0]).unwrap() << 4 | nib(p[1]).unwrap()); }
        } else {
            self.byte(0x80); self.u(hex.len() as u64); self.raw(hex);
        }
    }
}

pub struct R<'a> { pub b: &'a [u8], pub i: usize }

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> Self { R { b, i: 0 } }
    pub fn at(b: &'a [u8], i: usize) -> Self { R { b, i } }
    fn need(&self, n: usize) -> Result<(), String> {
        if self.i + n > self.b.len() { Err(format!("truncated at {}", self.i)) } else { Ok(()) }
    }
    pub fn u(&mut self) -> Result<u64, String> {
        let (mut x, mut sh) = (0u64, 0);
        loop {
            self.need(1)?;
            let c = self.b[self.i]; self.i += 1;
            if sh >= 64 { return Err("varint too long".into()); }
            x |= ((c & 0x7f) as u64) << sh;
            if c < 0x80 { return Ok(x); }
            sh += 7;
        }
    }
    pub fn byte(&mut self) -> Result<u8, String> { self.need(1)?; self.i += 1; Ok(self.b[self.i - 1]) }
    pub fn raw(&mut self, n: usize) -> Result<&'a [u8], String> { self.need(n)?; self.i += n; Ok(&self.b[self.i - n..self.i]) }
    pub fn s(&mut self) -> Result<String, String> {
        let n = self.u()? as usize;
        String::from_utf8(self.raw(n)?.to_vec()).map_err(|_| format!("not UTF-8 at {}", self.i))
    }
    pub fn eid(&mut self) -> Result<Eid, String> {
        let x = self.u()?;
        let sub = if x & 1 != 0 { self.u()? } else { 0 };
        Eid::from_parts(((x >> 1) & 3) as u8, x >> 3, sub).ok_or_else(|| format!("bad id at {}", self.i))
    }
    pub fn guid(&mut self, kind: u8) -> Result<String, String> {
        if kind == 0 { return self.s(); }
        let t = if kind == 1 { HEX } else { HEX_UP };
        let mut o = String::with_capacity(36);
        for (k, &x) in self.raw(16)?.iter().enumerate() {
            if [4, 6, 8, 10].contains(&k) { o.push('-'); }
            o.push(t[(x >> 4) as usize] as char); o.push(t[(x & 15) as usize] as char);
        }
        Ok(o)
    }
    pub fn hash(&mut self) -> Result<NodeHash, String> {
        let f = self.byte()?;
        if f == 0x80 { let n = self.u()? as usize; return Ok(NodeHash(self.raw(n)?.to_vec())); }
        Ok(NodeHash(self.raw(f as usize)?.iter().flat_map(|&x| [HEX[(x >> 4) as usize], HEX[(x & 15) as usize]]).collect()))
    }
    pub fn u64le(&mut self) -> Result<u64, String> { Ok(u64::from_le_bytes(self.raw(8)?.try_into().unwrap())) }
    pub fn u32le(&mut self) -> Result<u32, String> { Ok(u32::from_le_bytes(self.raw(4)?.try_into().unwrap())) }
}

/// A file's dictionary of the short strings that repeat (datatypes, languages, node kinds and field names, wikis):
/// stored once, referenced by number.
#[derive(Default)]
pub struct Dict { pub list: Vec<String>, map: std::collections::HashMap<String, u64> }

impl Dict {
    pub fn id(&mut self, s: &str) -> u64 {
        if let Some(&i) = self.map.get(s) { return i; }
        let i = self.list.len() as u64;
        self.list.push(s.to_string()); self.map.insert(s.to_string(), i);
        i
    }
    pub fn from_list(list: Vec<String>) -> Dict {
        let map = list.iter().enumerate().map(|(i, s)| (s.clone(), i as u64)).collect();
        Dict { list, map }
    }
    pub fn write(&self, w: &mut W) { w.u(self.list.len() as u64); for s in &self.list { w.s(s); } }
    pub fn read(r: &mut R) -> Result<Vec<String>, String> { (0..r.u()?).map(|_| r.s()).collect() }
}

pub fn entry(d: &[String], i: u64) -> Result<String, String> {
    d.get(i as usize).cloned().ok_or_else(|| format!("dictionary entry {i}"))
}
