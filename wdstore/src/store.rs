//! A built store, opened: its chunks (by id range), its node files, its redirects. `get` reads one entity;
//! `resolve` follows a redirect to the id that remains (a benchmark's gold may use the old id); `check` re-reads
//! every file whole, re-writes it and compares the bytes, and compares each file's hash with the manifest's.

use crate::chunk;
use crate::codec::R;
use crate::model::*;
use crate::nodes::{self, Ix, NodeFile, Nodes};

pub const REDIR_MAGIC: &[u8; 8] = b"WDREDIR\0";

/// `redirects.wds`: "WDREDIR\0" | u32 version | u64 count | (old id u64, new id u64)* sorted by the old id.
pub fn write_redirects(r: &[(Eid, Eid)]) -> Vec<u8> {
    let mut b = Vec::with_capacity(20 + 16 * r.len());
    b.extend_from_slice(REDIR_MAGIC); b.extend_from_slice(&2u32.to_le_bytes());
    b.extend_from_slice(&(r.len() as u64).to_le_bytes());
    for (a, c) in r { b.extend_from_slice(&a.0.to_le_bytes()); b.extend_from_slice(&c.0.to_le_bytes()); }
    b
}

pub struct Store { pub dir: String, chunks: Vec<(Eid, Eid, Vec<u8>)>, pub nodes: Nodes, redirects: Vec<u8> }

fn read(dir: &str, name: &str) -> Result<Vec<u8>, String> { std::fs::read(format!("{dir}/{name}")).map_err(|e| format!("{name}: {e}")) }

impl Store {
    pub fn open(dir: &str) -> Result<Store, String> {
        let mut chunks = Vec::new();
        for k in 0.. {
            let Ok(b) = std::fs::read(format!("{dir}/chunk_{k:05}.wds")) else { break };
            let rd = chunk::Reader::open(&b)?;
            if rd.is_empty() { continue; }
            let (first, last) = (rd.slot(0).0, rd.slot(rd.len() - 1).0);
            chunks.push((first, last, b));
        }
        let redirects = read(dir, "redirects.wds")?;
        if redirects.len() < 20 || &redirects[..8] != REDIR_MAGIC { return Err("not a redirects file".into()); }
        Ok(Store { dir: dir.into(), chunks, redirects,
                   nodes: Nodes { values: NodeFile::open(read(dir, "values.wds")?)?, refs: NodeFile::open(read(dir, "refs.wds")?)? } })
    }

    pub fn get(&self, id: Eid) -> Result<Option<Entity>, String> {
        let k = self.chunks.partition_point(|c| c.1 < id);
        match self.chunks.get(k) {
            Some((first, _, b)) if *first <= id => chunk::Reader::open(b)?.get(id, &self.nodes),
            _ => Ok(None),
        }
    }

    fn redirect(&self, id: Eid) -> Option<Eid> {
        let n = u64::from_le_bytes(self.redirects[12..20].try_into().unwrap()) as usize;
        let at = |i: usize| u64::from_le_bytes(self.redirects[20 + 16 * i..28 + 16 * i].try_into().unwrap());
        let (mut lo, mut hi) = (0, n);
        while lo < hi {
            let m = (lo + hi) / 2;
            match at(m).cmp(&id.0) {
                std::cmp::Ordering::Equal => return Some(Eid(u64::from_le_bytes(self.redirects[28 + 16 * m..36 + 16 * m].try_into().unwrap()))),
                std::cmp::Ordering::Less => lo = m + 1,
                std::cmp::Ordering::Greater => hi = m,
            }
        }
        None
    }

    /// The id that remains: followed through redirects (at most 8; a loop is an error, not an answer).
    pub fn resolve(&self, id: Eid) -> Result<Eid, String> {
        let mut cur = id;
        for _ in 0..8 { match self.redirect(cur) { Some(n) => cur = n, None => return Ok(cur) } }
        Err(format!("{}: a redirect chain over 8 or a loop", id.text()))
    }

    pub fn statements(&self) -> Result<u64, String> {
        let mut n = 0;
        for (_, _, b) in &self.chunks { n += chunk::Reader::open(b)?.read_all(&self.nodes)?.iter().map(|e| e.statements.len() as u64).sum::<u64>(); }
        Ok(n)
    }

    /// Every file re-read whole and re-written to the same bytes; every file's hash as the manifest records it.
    pub fn check(&self) -> Result<String, String> {
        let man = std::fs::read_to_string(format!("{}/manifest.json", self.dir)).map_err(|e| e.to_string())?;
        let has = |b: &[u8], name: &str| -> Result<(), String> {
            let h = blake3::hash(b).to_hex().to_string();
            if man.contains(&h) { Ok(()) } else { Err(format!("{name}: hash {h} is not the manifest's")) }
        };
        let (v, r) = (&self.nodes.values, &self.nodes.refs);
        let vorder = (0..v.len() as u32).map(|i| v.hash(i)).collect::<Result<Vec<_>, _>>()?;
        let rorder = (0..r.len() as u32).map(|i| r.hash(i)).collect::<Result<Vec<_>, _>>()?;
        let vdef: Vec<ValueNode> = (0..v.len() as u32).map(|i| v.value(i)).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect();
        let rdef: Vec<RefNode> = (0..r.len() as u32).map(|i| r.reference(i, &self.nodes)).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect();
        let ix = Ix::new(&vorder, &rorder);
        if nodes::write_values(&vorder, &vdef) != v.bytes() { return Err("values.wds: re-writing differs".into()); }
        if nodes::write_refs(&rorder, &rdef, &ix)? != r.bytes() { return Err("refs.wds: re-writing differs".into()); }
        has(v.bytes(), "values.wds")?; has(r.bytes(), "refs.wds")?; has(&self.redirects, "redirects.wds")?;
        let mut rd = R::at(&self.redirects, 12);
        let n = rd.u64le()?;
        let pairs = (0..n).map(|_| Ok((Eid(rd.u64le()?), Eid(rd.u64le()?)))).collect::<Result<Vec<_>, String>>()?;
        if write_redirects(&pairs) != self.redirects { return Err("redirects.wds: re-writing differs".into()); }
        for (k, (_, _, b)) in self.chunks.iter().enumerate() {
            has(b, &format!("chunk {k}"))?;
            let ents = chunk::Reader::open(b)?.read_all(&self.nodes)?;
            if chunk::write(&ents, &ix)? != *b { return Err(format!("chunk {k}: re-writing differs")); }
        }
        Ok(format!("{} chunks, {} value nodes ({} undefined), {} references ({} undefined), {} redirects: each re-read and \
                    re-written to the same bytes, hashes as recorded", self.chunks.len(), v.len(), v.undefined()?, r.len(),
                   r.undefined()?, n))
    }
}
