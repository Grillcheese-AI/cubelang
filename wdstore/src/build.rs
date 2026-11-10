//! The build: the dump's text in its order -> the store's files and `manifest.json`, never sorting the entities.
//! The text comes from gzip or plain files of entity blocks (the 1% sample's segments) or from the dump's bzip2
//! parts (`bz2par`, every core). It is cut into entity blocks before `\ndata:`, ingested on every core a batch at a
//! time, and handed to one writer in the dump's order (so a rebuild writes the same bytes). A block that fails to
//! parse is counted and listed, never dropped silently.

use std::io::Read;

use crate::chunk;
use crate::ingest::{self, Block, Params};
use crate::model::*;
use crate::names::NamesWriter;
use crate::nodes::{Logs, NodeLog};
use crate::reverse::{self, RevWriter};
use crate::spill::Runs;

pub const BATCH: usize = 64 << 20;

#[derive(Default)]
pub struct Stats {
    pub blocks: u64, pub failed: Vec<String>, pub failed_n: u64, pub entities: [u64; 2], pub statements: [u64; 2],
    pub hot_qualifiers: u64, pub cold_qualifiers: u64, pub references: u64, pub names: u64, pub duplicates: u64,
    pub skipped: std::collections::BTreeMap<String, u64>,
}

pub fn j(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() {
        match ch { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"),
                   c if (c as u32) < 32 => o.push_str(&format!("\\u{:04x}", c as u32)), c => o.push(c) }
    }
    o.push('"'); o
}

/// The text's entity blocks: each starts at `data:` (the first may be a header).
pub fn blocks(text: &str) -> impl Iterator<Item = &str> {
    let mut starts: Vec<usize> = text.match_indices("\ndata:").map(|(i, _)| i + 1).collect();
    starts.insert(0, 0);
    let ends: Vec<usize> = starts[1..].iter().copied().chain([text.len()]).collect();
    starts.into_iter().zip(ends).map(move |(a, b)| &text[a..b]).filter(|t| !t.trim().is_empty())
}

/// The store being written: one writer, fed in the dump's order.
pub struct Writer {
    out: String, prm: Params, cold_classes: Vec<Eid>, per_chunk: usize, logs: Logs, pending: [Vec<Entity>; 2],
    chunks: [Vec<String>; 2], ids: Vec<(u64, u32, u32)>, redirects: Vec<(Eid, Eid)>, rev: Runs, names: [Runs; 2],
    pub st: Stats, bytes: [u64; 2],
}

const SETS: [&str; 2] = ["hot", "cold"];

impl Writer {
    pub fn create(out: &str, scratch: &str, prm: Params, cold_classes: Vec<Eid>, per_chunk: usize, sort_mem: usize) -> Result<Writer, String> {
        std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
        let logs = Logs { values: NodeLog::create(&format!("{out}/values.wds"), 0)?, refs: NodeLog::create(&format!("{out}/refs.wds"), 1)? };
        Ok(Writer { out: out.into(), prm, cold_classes, per_chunk, logs, pending: [vec![], vec![]], chunks: [vec![], vec![]],
            ids: vec![], redirects: vec![], rev: Runs::new(scratch, "reverse", sort_mem / 3),
            names: [Runs::new(scratch, "names_exact", sort_mem / 3), Runs::new(scratch, "names_folded", sort_mem / 3)],
            st: Stats::default(), bytes: [0, 0] })
    }

    fn cold(&self, e: &Entity) -> bool {
        e.statements.iter().any(|s| s.pid == 31 && matches!(s.value, Value::Entity(c) if self.cold_classes.contains(&c)))
    }

    /// One ingested block, in the dump's order.
    pub fn add(&mut self, b: Block) -> Result<(), String> {
        for (k, v) in b.skipped { *self.st.skipped.entry(k).or_default() += v; }
        for n in &b.value_nodes { self.logs.values.define_value(n)?; }
        for n in &b.ref_nodes { self.logs.refs.define_ref(n, &mut self.logs.values)?; }
        self.redirects.extend(b.redirects);
        let Some(e) = b.entity else { return Ok(()) };
        for s in &e.statements {
            if let Some(h) = &s.value_node { self.logs.values.number(h); }
            for (_, h) in s.hot_qualifier_nodes.iter().chain(&s.cold_qualifier_nodes) { self.logs.values.number(h); }
            for h in &s.references { self.logs.refs.number(h); }
            self.st.hot_qualifiers += s.hot_qualifiers.len() as u64; self.st.cold_qualifiers += s.cold_qualifiers.len() as u64;
            self.st.references += s.references.len() as u64;
        }
        let set = self.cold(&e) as usize;
        self.st.entities[set] += 1; self.st.statements[set] += e.statements.len() as u64; self.st.names += e.names.len() as u64;
        if set == 0 {
            for r in reverse::rows(&e) { self.rev.push(r)?; }
            for (k, rs) in crate::names::rows(&e).into_iter().enumerate() { for r in rs { self.names[k].push(r)?; } }
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
        let k = self.chunks[set].len() as u32;
        let name = format!("chunk_{}_{k:05}.wds", SETS[set]);
        let rd = chunk::Reader::open(&bytes)?;
        for i in 0..rd.len() { let (id, off) = rd.slot(i); self.ids.push((id.0, (set as u32) << 31 | k, off as u32)); }
        self.bytes[set] += bytes.len() as u64;
        self.chunks[set].push(file_entry(&self.out, &name, &bytes)?);
        Ok(())
    }

    /// Ingests a text holding whole entity blocks, on `threads` threads, and adds the blocks in order.
    pub fn text(&mut self, text: &str, threads: usize) -> Result<(), String> {
        let bl: Vec<&str> = blocks(text).collect();
        let per = bl.len().div_ceil(threads.max(1)).max(1);
        let prm = &self.prm;
        let done: Vec<Vec<Result<Block, String>>> = std::thread::scope(|s| {
            let hs: Vec<_> = bl.chunks(per).map(|part| s.spawn(move || part.iter().map(|b| ingest::block(b, prm)).collect())).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for r in done.into_iter().flatten() {
            self.st.blocks += 1;
            match r { Ok(b) => self.add(b)?, Err(e) => { self.st.failed_n += 1; if self.st.failed.len() < 20 { self.st.failed.push(e); } } }
        }
        Ok(())
    }

    /// The last chunks, the node files, the id table, redirects, the reverse index, the names; the manifest.
    pub fn finish(mut self, inputs: Vec<String>, t0: std::time::Instant) -> Result<String, String> {
        self.flush(0)?; self.flush(1)?;
        let Writer { out, prm, cold_classes, per_chunk, logs, chunks, mut ids, mut redirects, rev, names, st, bytes, .. } = self;
        let (vn, vu) = logs.values.finish()?;
        let (rn, ru) = logs.refs.finish()?;
        ids.sort();
        let dup = ids.windows(2).filter(|w| w[0].0 == w[1].0).count();
        if dup > 0 { return Err(format!("{dup} ids in two chunks")); }
        let mut idb = Vec::with_capacity(20 + 16 * ids.len());
        idb.extend_from_slice(b"WDIDS\0\0\0"); idb.extend_from_slice(&3u32.to_le_bytes()); idb.extend_from_slice(&(ids.len() as u64).to_le_bytes());
        for (id, c, o) in &ids { idb.extend_from_slice(&id.to_le_bytes()); idb.extend_from_slice(&c.to_le_bytes()); idb.extend_from_slice(&o.to_le_bytes()); }
        let mut files = vec![file_entry(&out, "ids.wds", &idb)?];
        redirects.sort(); redirects.dedup();
        files.push(file_entry(&out, "redirects.wds", &crate::store::write_redirects(&redirects))?);
        let mut rw = RevWriter::create(&format!("{out}/reverse.wds"))?;
        rev.merge(|r| rw.add(r))?;
        let rkeys = rw.finish()?;
        let mut nw = NamesWriter::create(&out)?;
        for (k, runs) in names.into_iter().enumerate() { runs.merge(|r| nw.add(k, r))?; }
        let nkeys = nw.finish()?;
        let size = |f: &str| std::fs::metadata(format!("{out}/{f}")).map(|m| m.len()).unwrap_or(0);
        for f in ["values.wds", "refs.wds", "reverse.wds", "names_exact.fst", "names_folded.fst", "names.post"] {
            let b = std::fs::read(format!("{out}/{f}")).map_err(|e| e.to_string())?;
            files.push(format!("{{\"file\": {}, \"bytes\": {}, \"blake3\": {}}}", j(f), b.len(), j(&blake3::hash(&b).to_hex().to_string())));
        }
        let shared = size("values.wds") + size("refs.wds");
        let hot_idx = size("reverse.wds") + size("names_exact.fst") + size("names_folded.fst") + size("names.post") + size("ids.wds");
        for c in &chunks { files.extend(c.iter().cloned()); }
        let hot_total = bytes[0] + shared + hot_idx;
        let per = |b: u64, n: u64| b as f64 / n.max(1) as f64;
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
            " \"bytes_per_statement\": {{\"hot_chunks\": {:.1}, \"cold_chunks\": {:.1}, \"hot_total\": {:.1}}},\n \"build_s\": {:.1}\n}}\n"),
            j(&crate::code_hash()), prm.langs, prm.wikis, prm.hot_qualifiers,
            cold_classes.iter().map(|c| j(&c.text())).collect::<Vec<_>>().join(", "), per_chunk, inputs.join(", "),
            files.join(",\n  "), st.blocks, st.failed_n, st.entities[0], st.entities[1], st.statements[0], st.statements[1],
            st.duplicates, st.hot_qualifiers, st.cold_qualifiers, st.references, st.names, vn, vu, rn, ru, redirects.len(),
            rkeys, nkeys[0], nkeys[1], skipped.join(", "), st.failed.iter().map(|s| j(s)).collect::<Vec<_>>().join(", "),
            bytes[0], bytes[1], shared, hot_idx, hot_total, per(bytes[0], st.statements[0]), per(bytes[1], st.statements[1]),
            per(hot_total, st.statements[0]), t0.elapsed().as_secs_f64());
        std::fs::write(format!("{out}/manifest.json"), &man).map_err(|e| e.to_string())?;
        Ok(man)
    }
}

fn file_entry(out: &str, name: &str, bytes: &[u8]) -> Result<String, String> {
    std::fs::write(format!("{out}/{name}"), bytes).map_err(|e| format!("{name}: {e}"))?;
    Ok(format!("{{\"file\": {}, \"bytes\": {}, \"blake3\": {}}}", j(name), bytes.len(), j(&blake3::hash(bytes).to_hex().to_string())))
}

/// Builds `out` from `inputs`: bzip2 parts (the dump, read as one file) or gzip / plain files of entity blocks.
pub fn build(inputs: &[String], out: &str, scratch: &str, per_chunk: usize, cold: Vec<Eid>, threads: usize) -> Result<String, String> {
    let t0 = std::time::Instant::now();
    let mut w = Writer::create(out, scratch, Params::default(), cold, per_chunk, 3 << 30)?;
    let first = std::fs::File::open(inputs.first().ok_or("no input")?).and_then(|mut f| { let mut b = [0u8; 3]; f.read_exact(&mut b).map(|_| b) })
        .map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    let part = std::path::Path::new(&inputs[0]).file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("part_"));
    if &first == b"BZh" || part {
        let mut carry: Vec<u8> = Vec::new();
        let n = crate::bz2par::decode_parts(inputs, threads, 64 << 20, |t| {
            carry.extend_from_slice(t);
            if carry.len() >= BATCH {
                let cut = carry.windows(6).rposition(|x| x == b"\ndata:").map(|i| i + 1).unwrap_or(0);
                if cut > 0 {
                    let text = std::str::from_utf8(&carry[..cut]).map_err(|_| "the dump's text is not UTF-8")?;
                    w.text(text, threads)?;
                    carry.drain(..cut);
                }
            }
            Ok(())
        })?;
        w.text(std::str::from_utf8(&carry).map_err(|_| "the dump's text is not UTF-8")?, threads)?;
        names.push(format!("{{\"parts\": {}, \"bytes\": {n}}}", inputs.len()));
    } else {
        for p in inputs {
            let raw = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
            let hash = blake3::hash(&raw).to_hex().to_string();
            let text = if raw.starts_with(&[0x1f, 0x8b]) {
                let mut s = String::new();
                flate2::read::MultiGzDecoder::new(&raw[..]).read_to_string(&mut s).map_err(|e| format!("{p}: {e}"))?;
                s
            } else { String::from_utf8(raw).map_err(|_| format!("{p}: not UTF-8"))? };
            w.text(&text, threads)?;
            let name = std::path::Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p).to_string();
            names.push(format!("{{\"name\": {}, \"bytes\": {}, \"blake3\": {}}}", j(&name), text.len(), j(&hash)));
        }
    }
    w.finish(names, t0)
}
