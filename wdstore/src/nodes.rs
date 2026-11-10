//! The value nodes (`values.wds`) and reference nodes (`refs.wds`), numbered from 0 in the order the build first
//! sees them (a reference to one or its definition): a statement names a node by its number. Each definition the
//! dump writes is a record, appended as it comes; the dump may write one node's triples in two places (a quantity's
//! amount and unit in one block, its `quantityNormalized` link in another), so a node may have several records,
//! merged on reading. A node named but never written gets a record with its hash alone, at the end, and is counted.
//!
//!   "WDNODES\0" | u32 version | u8 kind (0 values, 1 references) | u64 count | u64 dict_at | u64 offsets_at
//!   | u64 extras_at | u64 extras | records... | dictionary | offsets (u64 a node: its first record)
//!   | extras ((number u32, offset u64)* sorted: a node's further records)
//!   a record: varint length | varint number | hash | u8 defined | then a value node's kind and fields, or a
//!   reference's snaks and value-node numbers
//!
//! While the build runs, `<file>.num` holds every hash in the order it was numbered (a byte of length, the hash's
//! text), so a resumed build rebuilds the numbering exactly; the records themselves are scanned for their offsets.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::chunk::{get_snaks, put_snaks};
use crate::codec::{entry, Dict, R, W};
use crate::model::*;

pub const MAGIC: &[u8; 8] = b"WDNODES\0";
pub const VERSION: u32 = 3;
const HEAD: usize = 8 + 4 + 1 + 8 * 5;
const NONE: u64 = u64::MAX;

/// Hash -> number, as a chunk writer needs it.
pub trait Numbering {
    fn value(&self, h: &NodeHash) -> Result<u32, String>;
    fn reference(&self, h: &NodeHash) -> Result<u32, String>;
}

/// One node file being written: the numbering, the records appended so far, their offsets.
pub struct NodeLog {
    kind: u8, map: HashMap<NodeHash, u32>, first: Vec<u64>, extras: Vec<(u32, u64)>, pub dict: Dict,
    f: std::io::BufWriter<std::fs::File>, pos: u64, pub defined: u64, num: std::io::BufWriter<std::fs::File>, num_pos: u64,
}

/// What a checkpoint keeps of a node log: its two files' lengths, its definitions, its dictionary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogState { pub pos: u64, pub num_pos: u64, pub defined: u64, pub dict: Vec<String> }

impl NodeLog {
    pub fn create(path: &str, kind: u8) -> Result<NodeLog, String> {
        let mut f = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?);
        f.write_all(&[0u8; HEAD]).map_err(|e| e.to_string())?;
        let num = std::io::BufWriter::new(std::fs::File::create(format!("{path}.num")).map_err(|e| format!("{path}.num: {e}"))?);
        Ok(NodeLog { kind, map: HashMap::new(), first: vec![], extras: vec![], dict: Dict::default(), f, pos: HEAD as u64,
                     defined: 0, num, num_pos: 0 })
    }

    /// Both files written through; what a checkpoint records.
    pub fn state(&mut self) -> Result<LogState, String> {
        self.f.flush().map_err(|e| e.to_string())?; self.num.flush().map_err(|e| e.to_string())?;
        self.f.get_ref().sync_data().map_err(|e| e.to_string())?; self.num.get_ref().sync_data().map_err(|e| e.to_string())?;
        Ok(LogState { pos: self.pos, num_pos: self.num_pos, defined: self.defined, dict: self.dict.list.clone() })
    }

    /// The log as a checkpoint left it: both files cut to their recorded lengths, the numbering and the records'
    /// offsets read back from them.
    pub fn restore(path: &str, kind: u8, st: &LogState) -> Result<NodeLog, String> {
        let open = |p: &str| std::fs::OpenOptions::new().read(true).write(true).open(p).map_err(|e| format!("{p}: {e}"));
        let (mut f, mut nf) = (open(path)?, open(&format!("{path}.num"))?);
        f.set_len(st.pos).map_err(|e| e.to_string())?; nf.set_len(st.num_pos).map_err(|e| e.to_string())?;
        let mut nb = Vec::new();
        nf.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?; nf.read_to_end(&mut nb).map_err(|e| e.to_string())?;
        let (mut map, mut first) = (HashMap::new(), Vec::new());
        let mut i = 0;
        while i < nb.len() {
            let n = nb[i] as usize;
            map.insert(NodeHash(nb[i + 1..i + 1 + n].to_vec()), first.len() as u32);
            first.push(NONE); i += 1 + n;
        }
        let mut rb = Vec::new();
        f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?; f.read_to_end(&mut rb).map_err(|e| e.to_string())?;
        let mut extras = Vec::new();
        let mut r = R::at(&rb, HEAD);
        while r.i < rb.len() {
            let at = r.i as u64;
            let len = r.u()? as usize;
            let body = r.i;
            let n = r.u()? as usize;
            if n >= first.len() { return Err(format!("{path}: a record for node {n} past the numbering")); }
            if first[n] == NONE { first[n] = at; } else { extras.push((n as u32, at)); }
            r.i = body + len;
        }
        if r.i != rb.len() { return Err(format!("{path}: a record cut at the checkpoint")); }
        f.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?; nf.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        Ok(NodeLog { kind, map, first, extras, dict: Dict::from_list(st.dict.clone()), f: std::io::BufWriter::new(f), pos: st.pos,
                     defined: st.defined, num: std::io::BufWriter::new(nf), num_pos: st.num_pos })
    }

    pub fn len(&self) -> usize { self.first.len() }
    pub fn is_empty(&self) -> bool { self.first.is_empty() }

    /// The node's number: its own, or the next one if the build has not seen it yet.
    pub fn number(&mut self, h: &NodeHash) -> Result<u32, String> {
        if let Some(&n) = self.map.get(h) { return Ok(n); }
        if h.0.len() > 255 { return Err("a node hash over 255 bytes".into()); }
        let n = self.first.len() as u32;
        self.num.write_all(&[h.0.len() as u8]).and_then(|_| self.num.write_all(&h.0)).map_err(|e| e.to_string())?;
        self.num_pos += 1 + h.0.len() as u64;
        self.map.insert(h.clone(), n); self.first.push(NONE);
        Ok(n)
    }

    pub fn get(&self, h: &NodeHash) -> Option<u32> { self.map.get(h).copied() }

    fn record(&mut self, h: &NodeHash, body: &W, defined: bool) -> Result<(), String> {
        let n = self.number(h)?;
        let mut w = W::default();
        w.u(n as u64); w.hash(h); w.byte(defined as u8); w.raw(&body.b);
        let mut lw = W::default();
        lw.u(w.b.len() as u64); lw.raw(&w.b);
        if self.first[n as usize] == NONE { self.first[n as usize] = self.pos; } else { self.extras.push((n, self.pos)); }
        self.f.write_all(&lw.b).map_err(|e| e.to_string())?;
        self.pos += lw.b.len() as u64;
        if defined { self.defined += 1; }
        Ok(())
    }

    pub fn define_value(&mut self, n: &ValueNode) -> Result<(), String> {
        let mut b = W::default();
        let k = self.dict.id(&n.kind); b.u(k); b.u(n.fields.len() as u64);
        for (f, v) in &n.fields { let i = self.dict.id(f); b.u(i); b.s(v); }
        self.record(&n.hash, &b, true)
    }

    /// A reference node; its value nodes are numbered in `values`.
    pub fn define_ref(&mut self, n: &RefNode, values: &mut NodeLog) -> Result<(), String> {
        let mut b = W::default();
        put_snaks(&mut b, &mut self.dict, &n.snaks);
        b.u(n.nodes.len() as u64);
        for (p, h) in &n.nodes { b.u(*p as u64); b.u(values.number(h)? as u64); }
        self.record(&n.hash, &b, true)
    }

    /// Writes the hash-alone records, the dictionary, the offsets and the header; returns (nodes, undefined).
    pub fn finish(mut self) -> Result<(u64, u64), String> {
        let mut undefined: Vec<(u32, NodeHash)> = self.map.iter().filter(|(_, &n)| self.first[n as usize] == NONE)
            .map(|(h, &n)| (n, h.clone())).collect();
        undefined.sort();
        for (_, h) in &undefined { self.record(h, &W::default(), false)?; }
        let mut tail = W::default();
        let dict_at = self.pos;
        self.dict.write(&mut tail);
        let offsets_at = dict_at + tail.b.len() as u64;
        for o in &self.first { tail.raw(&o.to_le_bytes()); }
        let extras_at = dict_at + tail.b.len() as u64;
        self.extras.sort();
        for (n, o) in &self.extras { tail.raw(&n.to_le_bytes()); tail.raw(&o.to_le_bytes()); }
        self.f.write_all(&tail.b).map_err(|e| e.to_string())?;
        let mut h = Vec::with_capacity(HEAD);
        h.extend_from_slice(MAGIC); h.extend_from_slice(&VERSION.to_le_bytes()); h.push(self.kind);
        for x in [self.first.len() as u64, dict_at, offsets_at, extras_at, self.extras.len() as u64] { h.extend_from_slice(&x.to_le_bytes()); }
        let mut f = self.f.into_inner().map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        f.write_all(&h).map_err(|e| e.to_string())?;
        drop(self.num);
        Ok((self.first.len() as u64, undefined.len() as u64))
    }
}

/// The two logs of a build: the numbering its chunks are written with.
pub struct Logs { pub values: NodeLog, pub refs: NodeLog }

impl Numbering for Logs {
    fn value(&self, h: &NodeHash) -> Result<u32, String> { self.values.get(h).ok_or_else(|| format!("value node {} not numbered", String::from_utf8_lossy(&h.0))) }
    fn reference(&self, h: &NodeHash) -> Result<u32, String> { self.refs.get(h).ok_or_else(|| format!("reference {} not numbered", String::from_utf8_lossy(&h.0))) }
}

/// A numbering read back from the node files (for `check`).
pub struct Ix { values: HashMap<NodeHash, u32>, refs: HashMap<NodeHash, u32> }

impl Ix {
    pub fn from(n: &Nodes) -> Result<Ix, String> {
        let m = |f: &NodeFile| (0..f.len() as u32).map(|i| Ok((f.hash(i)?, i))).collect::<Result<HashMap<_, _>, String>>();
        Ok(Ix { values: m(&n.values)?, refs: m(&n.refs)? })
    }
}

impl Numbering for Ix {
    fn value(&self, h: &NodeHash) -> Result<u32, String> { self.values.get(h).copied().ok_or("value node not numbered".into()) }
    fn reference(&self, h: &NodeHash) -> Result<u32, String> { self.refs.get(h).copied().ok_or("reference not numbered".into()) }
}

/// One node file, read in place.
pub struct NodeFile { b: Vec<u8>, kind: u8, n: usize, dict: Vec<String>, offsets_at: usize, extras_at: usize, extras: usize }

impl NodeFile {
    pub fn open(b: Vec<u8>) -> Result<NodeFile, String> {
        if b.len() < HEAD || &b[..8] != MAGIC { return Err("not a node file".into()); }
        if u32::from_le_bytes(b[8..12].try_into().unwrap()) != VERSION { return Err("node file version".into()); }
        let kind = b[12];
        let mut r = R::at(&b, 13);
        let (n, dict_at, offsets_at, extras_at, extras) = (r.u64le()? as usize, r.u64le()? as usize, r.u64le()? as usize,
                                                           r.u64le()? as usize, r.u64le()? as usize);
        let dict = Dict::read(&mut R::at(&b, dict_at))?;
        Ok(NodeFile { b, kind, n, dict, offsets_at, extras_at, extras })
    }
    pub fn len(&self) -> usize { self.n }
    pub fn is_empty(&self) -> bool { self.n == 0 }

    fn off(&self, i: u32) -> Result<usize, String> {
        if i as usize >= self.n { return Err(format!("node {i} of {}", self.n)); }
        let o = self.offsets_at + 8 * i as usize;
        Ok(u64::from_le_bytes(self.b[o..o + 8].try_into().unwrap()) as usize)
    }

    /// Every record of node `i`, first then the further ones in file order.
    fn records(&self, i: u32) -> Result<Vec<usize>, String> {
        let mut v = vec![self.off(i)?];
        let at = |k: usize| { let o = self.extras_at + 12 * k;
            (u32::from_le_bytes(self.b[o..o + 4].try_into().unwrap()), u64::from_le_bytes(self.b[o + 4..o + 12].try_into().unwrap()) as usize) };
        let (mut lo, mut hi) = (0, self.extras);
        while lo < hi { let m = (lo + hi) / 2; if at(m).0 < i { lo = m + 1 } else { hi = m } }
        let mut k = lo;
        while k < self.extras && at(k).0 == i { v.push(at(k).1); k += 1; }
        Ok(v)
    }

    pub fn hash(&self, i: u32) -> Result<NodeHash, String> { let mut r = R::at(&self.b, self.off(i)?); r.u()?; r.u()?; r.hash() }

    /// A value node by number, its records merged: None if the dump never wrote it.
    pub fn value(&self, i: u32) -> Result<Option<ValueNode>, String> {
        if self.kind != 0 { return Err("not the value nodes".into()); }
        let mut out: Option<ValueNode> = None;
        for off in self.records(i)? {
            let mut r = R::at(&self.b, off);
            r.u()?; r.u()?; let hash = r.hash()?;
            if r.byte()? == 0 { continue; }
            let kind = entry(&self.dict, r.u()?)?;
            let fields: Vec<(String, String)> = (0..r.u()?).map(|_| Ok((entry(&self.dict, r.u()?)?, r.s()?))).collect::<Result<_, String>>()?;
            let n = out.get_or_insert(ValueNode { hash, kind: String::new(), fields: vec![] });
            if n.kind.is_empty() { n.kind = kind; } else if !kind.is_empty() && kind != n.kind { return Err(format!("value node {i}: two kinds")); }
            n.fields.extend(fields);
        }
        if let Some(n) = out.as_mut() { n.fields.sort(); n.fields.dedup(); }
        Ok(out)
    }

    pub fn reference(&self, i: u32, values: &NodeFile) -> Result<Option<RefNode>, String> {
        if self.kind != 1 { return Err("not the reference nodes".into()); }
        let mut out: Option<RefNode> = None;
        for off in self.records(i)? {
            let mut r = R::at(&self.b, off);
            r.u()?; r.u()?; let hash = r.hash()?;
            if r.byte()? == 0 { continue; }
            let snaks = get_snaks(&mut r, &self.dict)?;
            let nodes: Vec<(u32, NodeHash)> = (0..r.u()?).map(|_| Ok((r.u()? as u32, values.hash(r.u()? as u32)?))).collect::<Result<_, String>>()?;
            let n = out.get_or_insert(RefNode { hash, snaks: vec![], nodes: vec![] });
            n.snaks.extend(snaks); n.nodes.extend(nodes);
        }
        if let Some(n) = out.as_mut() {
            n.snaks.sort_by_key(|s| (s.pid, format!("{:?}", s.value))); n.snaks.dedup(); n.nodes.sort(); n.nodes.dedup();
        }
        Ok(out)
    }

    /// How many nodes the dump named but never wrote.
    pub fn undefined(&self) -> Result<u64, String> {
        let mut k = 0;
        for i in 0..self.n as u32 {
            if self.records(i)?.iter().all(|&o| { let mut r = R::at(&self.b, o); r.u().is_ok() && r.u().is_ok() && r.hash().is_ok() && r.byte() == Ok(0) }) { k += 1; }
        }
        Ok(k)
    }
}

/// The two node files, which a chunk's reader needs to turn numbers back into hashes.
pub struct Nodes { pub values: NodeFile, pub refs: NodeFile }
