//! The value nodes (`values.wds`) and reference nodes (`refs.wds`): each once, sorted by hash, numbered from 0; a
//! statement names a node by its number. A node a statement names but the build's input never wrote (the dump
//! writes a shared node once, so a part of the dump may lack it) is kept with its hash and marked undefined: the
//! build over the whole dump defines it, and its manifest counts the undefined ones (0 is the line).
//!
//!   "WDNODES\0" | u32 version | u8 kind (0 values, 1 references) | u64 count | u64 dict_at | u64 offsets_at
//!   records (hash, defined, then a value node's kind and fields, or a reference's snaks and value nodes)
//!   dictionary | offsets (u64 each)

use std::collections::HashMap;

use crate::chunk::{get_nodes, get_snaks, put_nodes, put_snaks};
use crate::codec::{entry, Dict, R, W};
use crate::model::*;

pub const MAGIC: &[u8; 8] = b"WDNODES\0";
pub const VERSION: u32 = 2;
const HEAD: usize = 8 + 4 + 1 + 8 * 3;

/// Hash -> number, for writing.
#[derive(Default)]
pub struct Ix { values: HashMap<NodeHash, u32>, refs: HashMap<NodeHash, u32> }

impl Ix {
    pub fn new(values: &[NodeHash], refs: &[NodeHash]) -> Ix {
        let num = |v: &[NodeHash]| v.iter().enumerate().map(|(i, h)| (h.clone(), i as u32)).collect();
        Ix { values: num(values), refs: num(refs) }
    }
    pub fn value(&self, h: &NodeHash) -> Result<u32, String> {
        self.values.get(h).copied().ok_or_else(|| format!("value node {} not numbered", String::from_utf8_lossy(&h.0)))
    }
    pub fn reference(&self, h: &NodeHash) -> Result<u32, String> {
        self.refs.get(h).copied().ok_or_else(|| format!("reference {} not numbered", String::from_utf8_lossy(&h.0)))
    }
}

/// Every hash a part of the dump names or writes, sorted and once: the numbering.
pub fn numbering(entities: &[Entity], values: &[ValueNode], refs: &[RefNode]) -> (Vec<NodeHash>, Vec<NodeHash>) {
    let (mut v, mut r): (Vec<NodeHash>, Vec<NodeHash>) = (values.iter().map(|n| n.hash.clone()).collect(),
                                                          refs.iter().map(|n| n.hash.clone()).collect());
    for n in refs { v.extend(n.nodes.iter().map(|x| x.1.clone())); }
    for e in entities {
        for s in &e.statements {
            v.extend(s.value_node.iter().cloned());
            for q in [&s.hot_qualifier_nodes, &s.cold_qualifier_nodes] { v.extend(q.iter().map(|x| x.1.clone())); }
            r.extend(s.references.iter().cloned());
        }
    }
    v.sort(); v.dedup(); r.sort(); r.dedup();
    (v, r)
}

fn head(kind: u8, count: u64, dict_at: u64, offsets_at: u64) -> Vec<u8> {
    let mut h = Vec::with_capacity(HEAD);
    h.extend_from_slice(MAGIC); h.extend_from_slice(&VERSION.to_le_bytes()); h.push(kind);
    for x in [count, dict_at, offsets_at] { h.extend_from_slice(&x.to_le_bytes()); }
    h
}

fn finish(mut w: W, d: Dict, offs: Vec<u64>, kind: u8) -> Vec<u8> {
    let dict_at = w.b.len() as u64;
    d.write(&mut w);
    let offsets_at = w.b.len() as u64;
    for o in &offs { w.raw(&o.to_le_bytes()); }
    let h = head(kind, offs.len() as u64, dict_at, offsets_at);
    w.b[..HEAD].copy_from_slice(&h);
    w.b
}

/// `values.wds`: one record per numbered hash, in number order; `defined` holds the nodes the input wrote.
pub fn write_values(order: &[NodeHash], defined: &[ValueNode]) -> Vec<u8> {
    let by: HashMap<&NodeHash, &ValueNode> = defined.iter().map(|n| (&n.hash, n)).collect();
    let (mut w, mut d, mut offs) = (W { b: vec![0; HEAD] }, Dict::default(), Vec::with_capacity(order.len()));
    for h in order {
        offs.push(w.b.len() as u64);
        w.hash(h);
        match by.get(h) {
            None => w.byte(0),
            Some(n) => {
                w.byte(1); let k = d.id(&n.kind); w.u(k); w.u(n.fields.len() as u64);
                for (f, v) in &n.fields { let i = d.id(f); w.u(i); w.s(v); }
            }
        }
    }
    finish(w, d, offs, 0)
}

/// `refs.wds`, the same way; a reference's value nodes by their number in `values.wds`.
pub fn write_refs(order: &[NodeHash], defined: &[RefNode], ix: &Ix) -> Result<Vec<u8>, String> {
    let by: HashMap<&NodeHash, &RefNode> = defined.iter().map(|n| (&n.hash, n)).collect();
    let (mut w, mut d, mut offs) = (W { b: vec![0; HEAD] }, Dict::default(), Vec::with_capacity(order.len()));
    for h in order {
        offs.push(w.b.len() as u64);
        w.hash(h);
        match by.get(h) {
            None => w.byte(0),
            Some(n) => { w.byte(1); put_snaks(&mut w, &mut d, &n.snaks); put_nodes(&mut w, ix, &n.nodes)?; }
        }
    }
    Ok(finish(w, d, offs, 1))
}

/// One node file, read in place.
pub struct NodeFile { b: Vec<u8>, kind: u8, n: usize, dict: Vec<String>, offsets_at: usize }

impl NodeFile {
    pub fn open(b: Vec<u8>) -> Result<NodeFile, String> {
        if b.len() < HEAD || &b[..8] != MAGIC { return Err("not a node file".into()); }
        if u32::from_le_bytes(b[8..12].try_into().unwrap()) != VERSION { return Err("node file version".into()); }
        let kind = b[12];
        let mut r = R::at(&b, 13);
        let (n, dict_at, offsets_at) = (r.u64le()? as usize, r.u64le()? as usize, r.u64le()? as usize);
        let dict = Dict::read(&mut R::at(&b, dict_at))?;
        Ok(NodeFile { b, kind, n, dict, offsets_at })
    }
    pub fn len(&self) -> usize { self.n }
    pub fn is_empty(&self) -> bool { self.n == 0 }
    pub fn bytes(&self) -> &[u8] { &self.b }

    fn at(&self, i: u32) -> Result<R<'_>, String> {
        if i as usize >= self.n { return Err(format!("node {i} of {}", self.n)); }
        let o = self.offsets_at + 8 * i as usize;
        Ok(R::at(&self.b, u64::from_le_bytes(self.b[o..o + 8].try_into().unwrap()) as usize))
    }
    pub fn hash(&self, i: u32) -> Result<NodeHash, String> { self.at(i)?.hash() }

    /// A value node by number: None if the input never wrote it.
    pub fn value(&self, i: u32) -> Result<Option<ValueNode>, String> {
        if self.kind != 0 { return Err("not the value nodes".into()); }
        let mut r = self.at(i)?;
        let hash = r.hash()?;
        if r.byte()? == 0 { return Ok(None); }
        let kind = entry(&self.dict, r.u()?)?;
        let fields = (0..r.u()?).map(|_| Ok((entry(&self.dict, r.u()?)?, r.s()?))).collect::<Result<_, String>>()?;
        Ok(Some(ValueNode { hash, kind, fields }))
    }

    pub fn reference(&self, i: u32, n: &Nodes) -> Result<Option<RefNode>, String> {
        if self.kind != 1 { return Err("not the reference nodes".into()); }
        let mut r = self.at(i)?;
        let hash = r.hash()?;
        if r.byte()? == 0 { return Ok(None); }
        let snaks = get_snaks(&mut r, &self.dict)?;
        Ok(Some(RefNode { hash, snaks, nodes: get_nodes(&mut r, n)? }))
    }

    /// How many numbered nodes the input never wrote.
    pub fn undefined(&self) -> Result<u64, String> {
        let mut k = 0;
        for i in 0..self.n as u32 { let mut r = self.at(i)?; r.hash()?; if r.byte()? == 0 { k += 1; } }
        Ok(k)
    }
}

/// The two node files, which a chunk's reader needs to turn numbers back into hashes.
pub struct Nodes { pub values: NodeFile, pub refs: NodeFile }
