//! Bytes in, bytes out: varints, strings, a chunk's small dictionary, node hashes. Little-endian, no padding, one
//! encoding per value, so the same input always writes the same bytes (the rebuild check).

use crate::model::NodeHash;

#[derive(Default)]
pub struct W { pub b: Vec<u8> }

impl W {
    pub fn u(&mut self, mut x: u64) {
        while x >= 0x80 { self.b.push((x as u8) | 0x80); x >>= 7; }
        self.b.push(x as u8);
    }
    pub fn byte(&mut self, x: u8) { self.b.push(x); }
    pub fn s(&mut self, x: &str) { self.u(x.len() as u64); self.b.extend_from_slice(x.as_bytes()); }
    pub fn raw(&mut self, x: &[u8]) { self.b.extend_from_slice(x); }

    /// A node hash: lowercase hex stored as its bytes (16 or 20), anything else as written (flag 0x80).
    pub fn hash(&mut self, h: &NodeHash) {
        let hex = &h.0;
        let low = hex.len() % 2 == 0 && hex.len() < 254 && hex.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
        if low {
            self.byte((hex.len() / 2) as u8);
            for pair in hex.chunks(2) {
                let v = |c: u8| if c <= b'9' { c - b'0' } else { c - b'a' + 10 };
                self.byte(v(pair[0]) << 4 | v(pair[1]));
            }
        } else {
            self.byte(0x80); self.u(hex.len() as u64); self.raw(hex);
        }
    }
}

pub struct R<'a> { pub b: &'a [u8], pub i: usize }

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> Self { R { b, i: 0 } }
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
    pub fn hash(&mut self) -> Result<NodeHash, String> {
        let f = self.byte()?;
        if f == 0x80 { let n = self.u()? as usize; return Ok(NodeHash(self.raw(n)?.to_vec())); }
        const HEX: &[u8; 16] = b"0123456789abcdef";
        Ok(NodeHash(self.raw(f as usize)?.iter().flat_map(|&x| [HEX[(x >> 4) as usize], HEX[(x & 15) as usize]]).collect()))
    }
}

/// A chunk's dictionary of the short strings that repeat (datatypes, languages, node kinds and field names, wikis):
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
}
