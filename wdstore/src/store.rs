//! A built store, opened: its hot and cold chunks, the id table, its node files, redirects, reverse index and names.
//! `get` finds an entity through `ids.wds` (sorted (id, set and chunk, offset), 16 bytes each) and reads its record
//! alone; `resolve` follows a redirect to the id that remains (a benchmark's gold may use the old id); `check`
//! re-reads every chunk whole and re-writes it to the same bytes, rebuilds the reverse and name rows from the hot
//! entities and compares them with the files, and compares every file's hash with the manifest's.

use crate::chunk;
use crate::codec::{Buf, R};
use crate::model::*;
use crate::names::{Found, Names};
use crate::nodes::{Ix, NodeFile, Nodes};
use crate::reverse::{self, Hit, Reverse};
use crate::spill::Runs;

pub const REDIR_MAGIC: &[u8; 8] = b"WDREDIR\0";

/// `redirects.wds`: "WDREDIR\0" | u32 version | u64 count | (old id u64, new id u64)* sorted by the old id.
pub fn write_redirects(r: &[(Eid, Eid)]) -> Vec<u8> {
    let mut b = Vec::with_capacity(20 + 16 * r.len());
    b.extend_from_slice(REDIR_MAGIC); b.extend_from_slice(&3u32.to_le_bytes());
    b.extend_from_slice(&(r.len() as u64).to_le_bytes());
    for (a, c) in r { b.extend_from_slice(&a.0.to_le_bytes()); b.extend_from_slice(&c.0.to_le_bytes()); }
    b
}

/// Where a store's cold chunks are: the folder `cold_dir.txt` names, else the store's own.
pub fn cold_of(dir: &str) -> String {
    std::fs::read_to_string(format!("{dir}/cold_dir.txt")).map(|s| s.trim().to_string()).unwrap_or_else(|_| dir.to_string())
}

fn read(dir: &str, name: &str) -> Result<Buf, String> { Buf::map(&format!("{dir}/{name}")) }

/// A sorted table of fixed rows whose first 8 bytes are a u64 key, searched in place.
fn find(b: &[u8], at: usize, n: usize, size: usize, key: u64) -> Option<usize> {
    let k = |i: usize| u64::from_le_bytes(b[at + size * i..at + size * i + 8].try_into().unwrap());
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let m = (lo + hi) / 2;
        match k(m).cmp(&key) { std::cmp::Ordering::Less => lo = m + 1, std::cmp::Ordering::Greater => hi = m, _ => return Some(at + size * m) }
    }
    None
}

pub struct Store {
    pub dir: String, chunks: [Vec<Buf>; 2], ids: Buf, pub nodes: Nodes, redirects: Buf, reverse: Reverse, names: Names,
}

impl Store {
    pub fn open(dir: &str) -> Result<Store, String> {
        let mut chunks = [Vec::new(), Vec::new()];
        for (s, set) in ["hot", "cold"].iter().enumerate() {
            let base = if s == 1 { cold_of(dir) } else { dir.to_string() };
            for k in 0.. {
                let p = format!("{base}/chunk_{set}_{k:05}.wds");
                if !std::path::Path::new(&p).exists() { break; }
                chunks[s].push(Buf::map(&p)?);
            }
        }
        let ids = read(dir, "ids.wds")?;
        if ids.len() < 20 || &ids[..8] != b"WDIDS\0\0\0" { return Err("not an id table".into()); }
        let redirects = read(dir, "redirects.wds")?;
        if redirects.len() < 20 || &redirects[..8] != REDIR_MAGIC { return Err("not a redirects file".into()); }
        let names = Names::open(read(dir, "names_exact.fst")?, read(dir, "names_folded.fst")?, read(dir, "names.post")?)?;
        Ok(Store { dir: dir.into(), chunks, ids, redirects, names, reverse: Reverse::open(read(dir, "reverse.wds")?)?,
                   nodes: Nodes::new(NodeFile::open(read(dir, "values.wds")?)?, NodeFile::open(read(dir, "refs.wds")?)?) })
    }

    fn count(b: &[u8]) -> usize { u64::from_le_bytes(b[12..20].try_into().unwrap()) as usize }

    /// (hot or cold, the entity) by id.
    pub fn locate(&self, id: Eid) -> Result<Option<(bool, Entity)>, String> {
        let Some(o) = find(&self.ids, 20, Self::count(&self.ids), 16, id.0) else { return Ok(None) };
        let c = u32::from_le_bytes(self.ids[o + 8..o + 12].try_into().unwrap());
        let off = u32::from_le_bytes(self.ids[o + 12..o + 16].try_into().unwrap()) as usize;
        let (set, k) = ((c >> 31) as usize, (c & 0x7fff_ffff) as usize);
        let b = self.chunks[set].get(k).ok_or("the id table names a missing chunk")?;
        Ok(Some((set == 1, chunk::Reader::open(b)?.at(off, id, &self.nodes)?)))
    }

    pub fn get(&self, id: Eid) -> Result<Option<Entity>, String> { Ok(self.locate(id)?.map(|x| x.1)) }

    /// (hot or cold, the entity, the value nodes its statements name, by hash).
    pub fn locate_with_nodes(&self, id: Eid) -> Result<Option<(bool, Entity, std::collections::HashMap<NodeHash, ValueNode>)>, String> {
        self.nodes.seen.borrow_mut().clear();
        let Some((cold, e)) = self.locate(id)? else { return Ok(None) };
        let seen: Vec<(NodeHash, u32)> = self.nodes.seen.borrow_mut().drain().collect();
        let mut m = std::collections::HashMap::new();
        for (h, i) in seen { if let Some(v) = self.nodes.values.value(i)? { m.insert(h, v); } }
        Ok(Some((cold, e, m)))
    }

    fn redirect(&self, id: Eid) -> Option<Eid> {
        find(&self.redirects, 20, Self::count(&self.redirects), 16, id.0)
            .map(|o| Eid(u64::from_le_bytes(self.redirects[o + 8..o + 16].try_into().unwrap())))
    }

    /// The id that remains: followed through redirects (at most 8; a loop is an error, not an answer).
    pub fn resolve(&self, id: Eid) -> Result<Eid, String> {
        let mut cur = id;
        for _ in 0..8 { match self.redirect(cur) { Some(n) => cur = n, None => return Ok(cur) } }
        Err(format!("{}: a redirect chain over 8 or a loop", id.text()))
    }

    /// Every hot entity with a statement `pid` = `value` (main values; at every rank, each hit says its rank).
    pub fn who(&self, pid: u32, value: Eid) -> Result<Vec<Hit>, String> { self.reverse.who(pid, value) }

    /// Every hot entity with a statement whose main value is `value`, whatever the property: (pid, hit).
    pub fn pointing_at(&self, value: Eid) -> Result<Vec<(u32, Hit)>, String> { self.reverse.pointing_at(value) }

    /// `who` and `pointing_at`, their first `limit` hits and how many there are in all.
    pub fn who_first(&self, pid: u32, value: Eid, limit: usize) -> Result<(u64, Vec<Hit>), String> { self.reverse.who_first(pid, value, limit) }
    pub fn pointing_at_first(&self, value: Eid, limit: usize) -> Result<(u64, Vec<(u32, Hit)>), String> {
        self.reverse.pointing_at_first(value, limit)
    }

    /// Every hot entity a name may mean (exact, else accent-folded, else a bounded typo), in id order, never by fame.
    pub fn name(&self, s: &str) -> Result<Found, String> { self.names.lookup(s) }

    /// Every `step`-th entity id, in id order (a spread sample for the latency measurement).
    pub fn ids(&self, step: usize) -> Vec<Eid> {
        (0..Self::count(&self.ids)).step_by(step.max(1))
            .map(|i| Eid(u64::from_le_bytes(self.ids[20 + 16 * i..28 + 16 * i].try_into().unwrap()))).collect()
    }

    /// Every chunk re-read whole and re-written to the same bytes; the reverse and name rows rebuilt from the hot
    /// entities and compared with the files; every file's hash as the manifest records it.
    pub fn check(&self) -> Result<String, String> {
        let man = std::fs::read_to_string(format!("{}/manifest.json", self.dir)).map_err(|e| e.to_string())?;
        let has = |name: &str| -> Result<(), String> {
            let h = blake3::hash(&read(&self.dir, name)?).to_hex().to_string();
            if man.contains(&h) { Ok(()) } else { Err(format!("{name}: hash {h} is not the manifest's")) }
        };
        for f in ["ids.wds", "redirects.wds", "values.wds", "refs.wds", "reverse.wds", "names_exact.fst", "names_folded.fst", "names.post"] { has(f)?; }
        let ix = Ix::from(&self.nodes)?;
        // the hot entities' index rows, sorted in runs on disk, then compared row by row with the index files
        let scratch = format!("{}/_check", self.dir);
        std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
        let mut runs = ["reverse", "names_exact", "names_folded"].map(|r| Runs::new(&scratch, r, 1 << 29));
        let mut n = [0usize; 2];
        for (s, set) in ["hot", "cold"].iter().enumerate() {
            for (k, b) in self.chunks[s].iter().enumerate() {
                let h = blake3::hash(b).to_hex().to_string();
                if !man.contains(&h) { return Err(format!("chunk_{set}_{k:05}.wds: hash {h} is not the manifest's")); }
                let ents = chunk::Reader::open(b)?.read_all(&self.nodes)?;
                if *chunk::write(&ents, &ix)? != **b { return Err(format!("chunk {set} {k}: re-writing differs")); }
                n[s] += ents.len();
                if s == 0 {
                    for e in &ents {
                        for r in reverse::rows(e) { runs[0].push(r)?; }
                        for (w, rs) in crate::names::rows(e).into_iter().enumerate() { for r in rs { runs[1 + w].push(r)?; } }
                    }
                }
            }
        }
        if n[0] + n[1] != Self::count(&self.ids) { return Err("the id table does not list every entity".into()); }
        let [rr, ex, fo] = runs;
        let mut it = self.reverse.rows_iter();
        rr.merge(|r| match it.next() { Some(Ok(x)) if x == r => Ok(()), _ => Err("reverse.wds: not the hot entities' statements".into()) })?;
        if it.next().is_some() { return Err("reverse.wds: rows no hot entity has".into()); }
        for (w, runs) in [ex, fo].into_iter().enumerate() {
            let what = ["exact", "folded"][w];
            let mut it = self.names.rows_iter(w);
            runs.merge(|r| match it.next() { Some(Ok(x)) if x == r => Ok(()), _ => Err(format!("names ({what}): not the hot entities' names")) })?;
            if it.next().is_some() { return Err(format!("names ({what}): names no hot entity bears")); }
        }
        let mut rd = R::at(&self.redirects, 12);
        let nr = rd.u64le()?;
        Ok(format!("{} hot and {} cold entities in {} and {} chunks, {} value nodes ({} undefined), {} references ({} undefined), \
                    {} redirects, {} reverse keys: each chunk re-written to the same bytes, the indexes the entities' own, \
                    hashes as recorded", n[0], n[1], self.chunks[0].len(), self.chunks[1].len(), self.nodes.values.len(),
                   self.nodes.values.undefined()?, self.nodes.refs.len(), self.nodes.refs.undefined()?, nr, self.reverse.keys()))
    }
}
