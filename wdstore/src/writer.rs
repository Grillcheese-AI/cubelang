//! The store being written: one writer, fed ingested blocks in the dump's order. Every file it writes grows only at
//! its end while the build runs (the chunks; the node logs; `ids.log`, `redirects.log`, `chunks.log`; the sorted
//! runs), so a checkpoint is each file's length, and a resumed build cuts each back to it and goes on.

use std::io::{BufWriter, Read, Write};

use crate::build::{ingest_all, j, Stats};
use crate::chunk;
use crate::ingest::{Block, Params};
use crate::model::*;
use crate::names::NamesWriter;
use crate::nodes::{Logs, NodeLog};
use crate::resume::Ckpt;
use crate::reverse::{self, RevWriter};
use crate::spill::Runs;

pub const SETS: [&str; 2] = ["hot", "cold"];
const SORT_MEM: usize = 1 << 30;
const RUNS: [&str; 3] = ["reverse", "names_exact", "names_folded"];

pub struct Writer {
    out: String, prm: Params, cold_classes: Vec<Eid>, per_chunk: usize, logs: Logs, pending: [Vec<Entity>; 2],
    chunks: [u32; 2], bytes: [u64; 2], ids: BufWriter<std::fs::File>, redirects: BufWriter<std::fs::File>,
    chunk_log: BufWriter<std::fs::File>, lens: [u64; 3], runs: [Runs; 3], pub st: Stats,
}

fn append(path: &str, keep: Option<u64>) -> Result<BufWriter<std::fs::File>, String> {
    let f = match keep {
        None => std::fs::File::create(path),
        Some(n) => std::fs::OpenOptions::new().write(true).open(path).and_then(|f| f.set_len(n).map(|_| f)),
    }.map_err(|e| format!("{path}: {e}"))?;
    let mut f = f;
    std::io::Seek::seek(&mut f, std::io::SeekFrom::End(0)).map_err(|e| e.to_string())?;
    Ok(BufWriter::new(f))
}

fn sync(w: &mut BufWriter<std::fs::File>) -> Result<(), String> {
    w.flush().map_err(|e| e.to_string())?; w.get_ref().sync_data().map_err(|e| e.to_string())
}

impl Writer {
    pub fn create(out: &str, scratch: &str, prm: Params, cold_classes: Vec<Eid>, per_chunk: usize) -> Result<Writer, String> {
        std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
        let logs = Logs { values: NodeLog::create(&format!("{out}/values.wds"), 0)?, refs: NodeLog::create(&format!("{out}/refs.wds"), 1)? };
        Ok(Writer { prm, cold_classes, per_chunk, logs, pending: [vec![], vec![]], chunks: [0, 0], bytes: [0, 0],
            ids: append(&format!("{out}/ids.log"), None)?, redirects: append(&format!("{out}/redirects.log"), None)?,
            chunk_log: append(&format!("{out}/chunks.log"), None)?, lens: [0; 3],
            runs: RUNS.map(|n| Runs::new(scratch, n, SORT_MEM / 3)), st: Stats::default(), out: out.into() })
    }

    pub fn restore(out: &str, scratch: &str, prm: Params, cold_classes: Vec<Eid>, per_chunk: usize, c: &Ckpt) -> Result<Writer, String> {
        let logs = Logs { values: NodeLog::restore(&format!("{out}/values.wds"), 0, c.values.as_ref().unwrap())?,
                          refs: NodeLog::restore(&format!("{out}/refs.wds"), 1, c.refs.as_ref().unwrap())? };
        let mut k = 0;
        let runs = RUNS.map(|n| { let r = Runs::restore(scratch, n, SORT_MEM / 3, c.runs[k]); k += 1; r });
        Ok(Writer { prm, cold_classes, per_chunk, logs, pending: [vec![], vec![]], chunks: c.chunks, bytes: c.bytes,
            ids: append(&format!("{out}/ids.log"), Some(c.ids))?, redirects: append(&format!("{out}/redirects.log"), Some(c.redirects))?,
            chunk_log: append(&format!("{out}/chunks.log"), Some(c.chunk_log))?, lens: [c.ids, c.redirects, c.chunk_log],
            runs, st: c.stats.clone(), out: out.into() })
    }

    fn cold(&self, e: &Entity) -> bool {
        e.statements.iter().any(|s| s.pid == 31 && matches!(s.value, Value::Entity(c) if self.cold_classes.contains(&c)))
    }

    /// One ingested block, in the dump's order.
    pub fn add(&mut self, b: Block) -> Result<(), String> {
        for (k, v) in b.skipped { *self.st.skipped.entry(k).or_default() += v; }
        for n in &b.value_nodes { self.logs.values.define_value(n)?; }
        for n in &b.ref_nodes { self.logs.refs.define_ref(n, &mut self.logs.values)?; }
        for (a, c) in &b.redirects {
            self.redirects.write_all(&a.0.to_le_bytes()).and_then(|_| self.redirects.write_all(&c.0.to_le_bytes())).map_err(|e| e.to_string())?;
            self.lens[1] += 16;
        }
        let Some(e) = b.entity else { return Ok(()) };
        for s in &e.statements {
            if let Some(h) = &s.value_node { self.logs.values.number(h)?; }
            for (_, h) in s.hot_qualifier_nodes.iter().chain(&s.cold_qualifier_nodes) { self.logs.values.number(h)?; }
            for h in &s.references { self.logs.refs.number(h)?; }
            self.st.hot_qualifiers += s.hot_qualifiers.len() as u64; self.st.cold_qualifiers += s.cold_qualifiers.len() as u64;
            self.st.references += s.references.len() as u64;
        }
        let set = self.cold(&e) as usize;
        self.st.entities[set] += 1; self.st.statements[set] += e.statements.len() as u64; self.st.names += e.names.len() as u64;
        if set == 0 {
            for r in reverse::rows(&e) { self.runs[0].push(r)?; }
            for (k, rs) in crate::names::rows(&e).into_iter().enumerate() { for r in rs { self.runs[1 + k].push(r)?; } }
        }
        self.pending[set].push(e);
        if self.pending[set].len() >= self.per_chunk { self.flush(set)?; }
        Ok(())
    }

    fn flush(&mut self, set: usize) -> Result<(), String> {
        let mut ents = std::mem::take(&mut self.pending[set]);
        if ents.is_empty() { return Ok(()); }
        ents.sort_by_key(|e| e.id);
        let before = ents.len();
        ents.dedup_by_key(|e| e.id);
        self.st.duplicates += (before - ents.len()) as u64;
        let bytes = chunk::write(&ents, &self.logs)?;
        let k = self.chunks[set];
        let name = format!("chunk_{}_{k:05}.wds", SETS[set]);
        let rd = chunk::Reader::open(&bytes)?;
        for i in 0..rd.len() {
            let (id, off) = rd.slot(i);
            for x in [&id.0.to_le_bytes()[..], &((set as u32) << 31 | k).to_le_bytes(), &(off as u32).to_le_bytes()] {
                self.ids.write_all(x).map_err(|e| e.to_string())?;
            }
            self.lens[0] += 16;
        }
        std::fs::write(format!("{}/{name}", self.out), &bytes).map_err(|e| format!("{name}: {e}"))?;
        let line = format!("{{\"file\": {}, \"bytes\": {}, \"blake3\": {}, \"entities\": {}}}\n", j(&name), bytes.len(),
                           j(&blake3::hash(&bytes).to_hex().to_string()), ents.len());
        self.chunk_log.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        self.lens[2] += line.len() as u64;
        self.bytes[set] += bytes.len() as u64;
        self.chunks[set] += 1;
        Ok(())
    }

    /// Ingests a text holding whole entity blocks, on `threads` threads, and adds the blocks in order.
    pub fn text(&mut self, text: &str, threads: usize) -> Result<(), String> {
        for r in ingest_all(text, &self.prm, threads) {
            self.st.blocks += 1;
            match r { Ok(b) => self.add(b)?, Err(e) => { self.st.failed_n += 1; if self.st.failed.len() < 20 { self.st.failed.push(e); } } }
        }
        Ok(())
    }

    /// Everything written through and synced, then the checkpoint saved: the text resumes at `bit`, `skip` bytes in.
    pub fn checkpoint(&mut self, key: &str, bit: u64, skip: u64, batches: u64) -> Result<(), String> {
        self.flush(0)?; self.flush(1)?;
        for w in [&mut self.ids, &mut self.redirects, &mut self.chunk_log] { sync(w)?; }
        let runs = [self.runs[0].state()?, self.runs[1].state()?, self.runs[2].state()?];
        let c = Ckpt { key: key.into(), block_bit: bit, skip, batches, values: Some(self.logs.values.state()?), refs: Some(self.logs.refs.state()?),
                       ids: self.lens[0], redirects: self.lens[1], chunk_log: self.lens[2], chunks: self.chunks, bytes: self.bytes, runs,
                       stats: self.st.clone() };
        c.save(&self.out)
    }

    fn read_pairs(path: &str) -> Result<Vec<[u8; 16]>, String> {
        let mut b = Vec::new();
        std::fs::File::open(path).and_then(|mut f| f.read_to_end(&mut b)).map_err(|e| format!("{path}: {e}"))?;
        Ok(b.chunks_exact(16).map(|c| c.try_into().unwrap()).collect())
    }

    /// The last chunks, the node files, the id table, redirects, the reverse index, the names; the manifest.
    pub fn finish(mut self, inputs: Vec<String>, t0: std::time::Instant) -> Result<String, String> {
        self.flush(0)?; self.flush(1)?;
        for w in [&mut self.ids, &mut self.redirects, &mut self.chunk_log] { sync(w)?; }
        let Writer { out, prm, cold_classes, per_chunk, logs, runs, st, bytes, .. } = self;
        let (vn, vu) = logs.values.finish()?;
        let (rn, ru) = logs.refs.finish()?;
        let mut ids: Vec<(u64, [u8; 8])> = Self::read_pairs(&format!("{out}/ids.log"))?.into_iter()
            .map(|r| (u64::from_le_bytes(r[..8].try_into().unwrap()), r[8..].try_into().unwrap())).collect();
        ids.sort();
        let dup = ids.windows(2).filter(|w| w[0].0 == w[1].0).count();
        if dup > 0 { return Err(format!("{dup} ids in two chunks")); }
        let mut idb = Vec::with_capacity(20 + 16 * ids.len());
        idb.extend_from_slice(b"WDIDS\0\0\0"); idb.extend_from_slice(&3u32.to_le_bytes()); idb.extend_from_slice(&(ids.len() as u64).to_le_bytes());
        for (id, rest) in &ids { idb.extend_from_slice(&id.to_le_bytes()); idb.extend_from_slice(rest); }
        std::fs::write(format!("{out}/ids.wds"), &idb).map_err(|e| e.to_string())?;
        let mut redirects: Vec<(Eid, Eid)> = Self::read_pairs(&format!("{out}/redirects.log"))?.into_iter()
            .map(|r| (Eid(u64::from_le_bytes(r[..8].try_into().unwrap())), Eid(u64::from_le_bytes(r[8..].try_into().unwrap())))).collect();
        redirects.sort(); redirects.dedup();
        std::fs::write(format!("{out}/redirects.wds"), crate::store::write_redirects(&redirects)).map_err(|e| e.to_string())?;
        let [rev, ex, fo] = runs;
        let mut rw = RevWriter::create(&format!("{out}/reverse.wds"))?;
        rev.merge(|r| rw.add(r))?;
        let rkeys = rw.finish()?;
        let mut nw = NamesWriter::create(&out)?;
        ex.merge(|r| nw.add(0, r))?; fo.merge(|r| nw.add(1, r))?;
        let nkeys = nw.finish()?;
        let mut files = Vec::new();
        let size = |f: &str| std::fs::metadata(format!("{out}/{f}")).map(|m| m.len()).unwrap_or(0);
        for f in ["ids.wds", "redirects.wds", "values.wds", "refs.wds", "reverse.wds", "names_exact.fst", "names_folded.fst", "names.post"] {
            let b = std::fs::read(format!("{out}/{f}")).map_err(|e| e.to_string())?;
            files.push(format!("{{\"file\": {}, \"bytes\": {}, \"blake3\": {}}}", j(f), b.len(), j(&blake3::hash(&b).to_hex().to_string())));
        }
        files.extend(std::fs::read_to_string(format!("{out}/chunks.log")).map_err(|e| e.to_string())?.lines().map(String::from));
        let shared = size("values.wds") + size("refs.wds");
        let hot_idx = size("reverse.wds") + size("names_exact.fst") + size("names_folded.fst") + size("names.post") + size("ids.wds");
        let hot_total = bytes[0] + shared + hot_idx;
        // tenths, in integers: no float anywhere in the store's code, the manifest included
        let per = |b: u64, n: u64| { let t = b * 10 / n.max(1); format!("{}.{}", t / 10, t % 10) };
        let ms = t0.elapsed().as_millis() as u64;
        let secs = format!("{}.{}", ms / 1000, ms % 1000 / 100);
        let skipped: Vec<String> = st.skipped.iter().map(|(k, v)| format!("{}: {v}", j(k))).collect();
        let man = format!(concat!("{{\n \"format_version\": 3,\n \"code_blake3\": {},\n",
            " \"params\": {{\"langs\": {:?}, \"wikis\": {:?}, \"hot_qualifiers\": {:?}, \"cold_classes\": [{}], \"per_chunk\": {}}},\n",
            " \"inputs\": [{}],\n \"files\": [\n  {}\n ],\n",
            " \"counts\": {{\"blocks\": {}, \"failed\": {}, \"entities_hot\": {}, \"entities_cold\": {}, \"statements_hot\": {}, ",
            "\"statements_cold\": {}, \"duplicates\": {}, \"hot_qualifiers\": {}, \"cold_qualifiers\": {}, \"references\": {}, \"names\": {}, ",
            "\"value_nodes\": {}, \"value_nodes_undefined\": {}, \"ref_nodes\": {}, \"ref_nodes_undefined\": {}, \"redirects\": {}, ",
            "\"reverse_keys\": {}, \"name_keys_exact\": {}, \"name_keys_folded\": {}}},\n",
            " \"skipped_triples\": {{{}}},\n \"failed_first\": [{}],\n",
            " \"bytes\": {{\"hot_chunks\": {}, \"cold_chunks\": {}, \"nodes\": {}, \"hot_indexes\": {}, \"hot_total\": {}}},\n",
            " \"bytes_per_statement\": {{\"hot_chunks\": {}, \"cold_chunks\": {}, \"hot_total\": {}}},\n \"build_s\": {}\n}}\n"),
            j(&crate::code_hash()), prm.langs, prm.wikis, prm.hot_qualifiers,
            cold_classes.iter().map(|c| j(&c.text())).collect::<Vec<_>>().join(", "), per_chunk, inputs.join(", "),
            files.join(",\n  "), st.blocks, st.failed_n, st.entities[0], st.entities[1], st.statements[0], st.statements[1],
            st.duplicates, st.hot_qualifiers, st.cold_qualifiers, st.references, st.names, vn, vu, rn, ru, redirects.len(),
            rkeys, nkeys[0], nkeys[1], skipped.join(", "), st.failed.iter().map(|s| j(s)).collect::<Vec<_>>().join(", "),
            bytes[0], bytes[1], shared, hot_idx, hot_total, per(bytes[0], st.statements[0]), per(bytes[1], st.statements[1]),
            per(hot_total, st.statements[0]), secs);
        std::fs::write(format!("{out}/manifest.json"), &man).map_err(|e| e.to_string())?;
        Ok(man)
    }
}
