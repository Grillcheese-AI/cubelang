//! Inputs -> chunk files and `manifest.json`. An input is a gzip or plain file of entity blocks (the dump's sample
//! segments; the dump itself, later, in id-range buckets). Each block is cut before `\ndata:`; a block that fails to
//! parse is counted and listed, never dropped silently.

use std::io::Read;

use crate::chunk::{self, Chunk};
use crate::ingest::{self, Params};
use crate::model::*;

#[derive(Default)]
pub struct Stats {
    pub blocks: u64, pub failed: Vec<String>, pub failed_n: u64, pub entities: u64, pub statements: u64,
    pub hot_qualifiers: u64, pub cold_qualifiers: u64, pub references: u64, pub value_nodes: u64, pub ref_nodes: u64,
    pub redirects: u64, pub names: u64, pub skipped: std::collections::BTreeMap<String, u64>,
}

pub fn read_input(path: &str) -> Result<(String, String), String> {
    let raw = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let hash = blake3::hash(&raw).to_hex().to_string();
    let text = if raw.starts_with(&[0x1f, 0x8b]) {   // gzip, by its magic (the sample segments are .gz.segNNNNN)
        let mut s = String::new();
        flate2::read::MultiGzDecoder::new(&raw[..]).read_to_string(&mut s).map_err(|e| format!("{path}: {e}"))?;
        s
    } else { String::from_utf8(raw).map_err(|_| format!("{path}: not UTF-8"))? };
    Ok((text, hash))
}

/// The blocks of a text: each starts at `data:` (the first may be a header or a tail).
pub fn blocks(text: &str) -> impl Iterator<Item = &str> {
    let mut starts: Vec<usize> = text.match_indices("\ndata:").map(|(i, _)| i + 1).collect();
    starts.insert(0, 0);
    let ends: Vec<usize> = starts[1..].iter().copied().chain([text.len()]).collect();
    starts.into_iter().zip(ends).map(move |(a, b)| &text[a..b]).filter(|t| !t.trim().is_empty())
}

pub fn ingest_text(text: &str, prm: &Params, c: &mut Chunk, st: &mut Stats) {
    for b in blocks(text) {
        st.blocks += 1;
        match ingest::block(b, prm) {
            Ok(x) => {
                for (k, v) in x.skipped { *st.skipped.entry(k).or_default() += v; }
                st.value_nodes += x.value_nodes.len() as u64; st.ref_nodes += x.ref_nodes.len() as u64;
                st.redirects += x.redirects.len() as u64;
                c.value_nodes.extend(x.value_nodes); c.ref_nodes.extend(x.ref_nodes); c.redirects.extend(x.redirects);
                if let Some(e) = x.entity {
                    st.entities += 1; st.names += e.names.len() as u64; st.statements += e.statements.len() as u64;
                    for s in &e.statements {
                        st.hot_qualifiers += s.hot_qualifiers.len() as u64; st.cold_qualifiers += s.cold_qualifiers.len() as u64;
                        st.references += s.references.len() as u64;
                    }
                    c.entities.push(e);
                }
            }
            Err(e) => { st.failed_n += 1; if st.failed.len() < 20 { st.failed.push(e); } }
        }
    }
}

fn j(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() { match ch { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), c if (c as u32) < 32 => o.push_str(&format!("\\u{:04x}", c as u32)), c => o.push(c) } }
    o.push('"'); o
}

/// Builds `out`: the chunks (`chunk_NNNNN.wds`, `per_chunk` entities each, ids ascending), then `manifest.json`.
pub fn build(inputs: &[String], out: &str, per_chunk: usize, prm: &Params) -> Result<String, String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let (mut all, mut st, mut ins) = (Chunk::default(), Stats::default(), Vec::new());
    let t0 = std::time::Instant::now();
    for p in inputs {
        let (text, hash) = read_input(p)?;
        ins.push(format!("{{\"name\": {}, \"bytes\": {}, \"blake3\": {}}}",
            j(std::path::Path::new(p).file_name().unwrap().to_str().unwrap()), text.len(), j(&hash)));
        ingest_text(&text, prm, &mut all, &mut st);
    }
    all.entities.sort_by_key(|e| e.id);
    all.entities.dedup_by_key(|e| e.id);
    all.value_nodes.sort_by(|a, b| a.hash.cmp(&b.hash)); all.value_nodes.dedup();
    all.ref_nodes.sort_by(|a, b| a.hash.cmp(&b.hash)); all.ref_nodes.dedup();
    all.redirects.sort(); all.redirects.dedup();
    let mut chunks = Vec::new();
    let n = all.entities.len().div_ceil(per_chunk.max(1)).max(1);
    let mut ents = std::mem::take(&mut all.entities).into_iter();
    let mut total = 0u64;
    for k in 0..n {
        let mut c = Chunk { entities: ents.by_ref().take(per_chunk).collect(), ..Default::default() };
        if k == n - 1 {   // the shared nodes and the redirects go in the last chunk (v1; a node file of their own later)
            c.value_nodes = std::mem::take(&mut all.value_nodes); c.ref_nodes = std::mem::take(&mut all.ref_nodes);
            c.redirects = std::mem::take(&mut all.redirects);
        }
        let bytes = chunk::write(&c)?;
        let name = format!("chunk_{k:05}.wds");
        std::fs::write(format!("{out}/{name}"), &bytes).map_err(|e| e.to_string())?;
        total += bytes.len() as u64;
        let (first, last) = (c.entities.first().and_then(|e| e.id), c.entities.last().and_then(|e| e.id));
        chunks.push(format!("{{\"file\": {}, \"first\": {}, \"last\": {}, \"entities\": {}, \"bytes\": {}, \"blake3\": {}}}",
            j(&name), j(&first.map(|e| e.text()).unwrap_or_default()), j(&last.map(|e| e.text()).unwrap_or_default()),
            c.entities.len(), bytes.len(), j(&blake3::hash(&bytes).to_hex().to_string())));
    }
    let skipped: Vec<String> = st.skipped.iter().map(|(k, v)| format!("{}: {v}", j(k))).collect();
    let man = format!(concat!("{{\n \"format_version\": {},\n \"code_blake3\": {},\n \"params\": {{\"langs\": {:?}, \"wikis\": {:?}, ",
        "\"hot_qualifiers\": {:?}, \"per_chunk\": {}}},\n \"inputs\": [{}],\n \"chunks\": [\n  {}\n ],\n",
        " \"counts\": {{\"blocks\": {}, \"failed\": {}, \"entities\": {}, \"statements\": {}, \"hot_qualifiers\": {}, ",
        "\"cold_qualifiers\": {}, \"references\": {}, \"value_nodes\": {}, \"ref_nodes\": {}, \"redirects\": {}, \"names\": {}}},\n",
        " \"skipped_triples\": {{{}}},\n \"failed_first\": [{}],\n \"bytes\": {},\n \"bytes_per_statement\": {:.1},\n \"build_s\": {:.1}\n}}\n"),
        chunk::VERSION, j(&crate::code_hash()), prm.langs, prm.wikis, prm.hot_qualifiers, per_chunk, ins.join(", "),
        chunks.join(",\n  "), st.blocks, st.failed_n, st.entities, st.statements, st.hot_qualifiers, st.cold_qualifiers,
        st.references, st.value_nodes, st.ref_nodes, st.redirects, st.names, skipped.join(", "),
        st.failed.iter().map(|s| j(s)).collect::<Vec<_>>().join(", "), total,
        total as f64 / st.statements.max(1) as f64, t0.elapsed().as_secs_f64());
    std::fs::write(format!("{out}/manifest.json"), &man).map_err(|e| e.to_string())?;
    Ok(man)
}

/// Finds one entity in a built store: the chunk whose id range holds it, then its index.
pub fn get(out: &str, id: Eid) -> Result<Option<Entity>, String> {
    let mut k = 0;
    loop {
        let path = format!("{out}/chunk_{k:05}.wds");
        let Ok(b) = std::fs::read(&path) else { return Ok(None) };
        if let Some(e) = chunk::Reader::open(&b)?.get(id)? { return Ok(Some(e)); }
        k += 1;
    }
}
