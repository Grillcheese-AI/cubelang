//! Inputs -> the store's files and `manifest.json`. An input is a gzip or plain file of entity blocks (the dump's
//! sample segments; the dump itself, later, in id-range buckets). Each block is cut before `\ndata:`; a block that
//! fails to parse is counted and listed, never dropped silently.

use std::io::Read;

use crate::chunk;
use crate::ingest::{self, Params};
use crate::model::*;
use crate::nodes::{self, Ix};
use crate::reverse;
use crate::store;

#[derive(Default)]
pub struct Parts { pub entities: Vec<Entity>, pub value_nodes: Vec<ValueNode>, pub ref_nodes: Vec<RefNode>, pub redirects: Vec<(Eid, Eid)> }

#[derive(Default)]
pub struct Stats {
    pub blocks: u64, pub failed: Vec<String>, pub failed_n: u64, pub entities: u64, pub statements: u64,
    pub hot_qualifiers: u64, pub cold_qualifiers: u64, pub references: u64, pub names: u64,
    pub skipped: std::collections::BTreeMap<String, u64>,
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

pub fn ingest_text(text: &str, prm: &Params, p: &mut Parts, st: &mut Stats) {
    for b in blocks(text) {
        st.blocks += 1;
        match ingest::block(b, prm) {
            Ok(x) => {
                for (k, v) in x.skipped { *st.skipped.entry(k).or_default() += v; }
                p.value_nodes.extend(x.value_nodes); p.ref_nodes.extend(x.ref_nodes); p.redirects.extend(x.redirects);
                if let Some(e) = x.entity {
                    st.entities += 1; st.names += e.names.len() as u64; st.statements += e.statements.len() as u64;
                    for s in &e.statements {
                        st.hot_qualifiers += s.hot_qualifiers.len() as u64; st.cold_qualifiers += s.cold_qualifiers.len() as u64;
                        st.references += s.references.len() as u64;
                    }
                    p.entities.push(e);
                }
            }
            Err(e) => { st.failed_n += 1; if st.failed.len() < 20 { st.failed.push(e); } }
        }
    }
}

pub fn j(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() {
        match ch { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"),
                   c if (c as u32) < 32 => o.push_str(&format!("\\u{:04x}", c as u32)), c => o.push(c) }
    }
    o.push('"'); o
}

/// One node per hash. The dump may write one node's triples in two places (a quantity's amount and unit in one
/// block, its `quantityNormalized` link in another): the node is their union. Two kinds for one hash is an error.
pub fn merge_values(mut v: Vec<ValueNode>) -> Result<Vec<ValueNode>, String> {
    v.sort_by(|a, b| a.hash.cmp(&b.hash));
    let mut out: Vec<ValueNode> = Vec::with_capacity(v.len());
    for n in v {
        match out.last_mut() {
            Some(l) if l.hash == n.hash => {
                if l.kind.is_empty() { l.kind = n.kind; }
                else if !n.kind.is_empty() && n.kind != l.kind {
                    return Err(format!("value node {}: two kinds", String::from_utf8_lossy(&n.hash.0)));
                }
                l.fields.extend(n.fields); l.fields.sort(); l.fields.dedup();
            }
            _ => out.push(n),
        }
    }
    Ok(out)
}

pub fn merge_refs(mut v: Vec<RefNode>) -> Vec<RefNode> {
    v.sort_by(|a, b| a.hash.cmp(&b.hash));
    let mut out: Vec<RefNode> = Vec::with_capacity(v.len());
    for n in v {
        match out.last_mut() {
            Some(l) if l.hash == n.hash => {
                l.snaks.extend(n.snaks); l.snaks.sort_by_key(|s| (s.pid, format!("{:?}", s.value))); l.snaks.dedup();
                l.nodes.extend(n.nodes); l.nodes.sort(); l.nodes.dedup();
            }
            _ => out.push(n),
        }
    }
    out
}

fn file_entry(out: &str, name: &str, bytes: &[u8]) -> Result<String, String> {
    std::fs::write(format!("{out}/{name}"), bytes).map_err(|e| format!("{name}: {e}"))?;
    Ok(format!("{{\"file\": {}, \"bytes\": {}, \"blake3\": {}}}", j(name), bytes.len(), j(&blake3::hash(bytes).to_hex().to_string())))
}

/// Builds `out`: `chunk_NNNNN.wds` (`per_chunk` entities each, ids ascending), `values.wds`, `refs.wds`,
/// `redirects.wds`, then `manifest.json`.
pub fn build(inputs: &[String], out: &str, per_chunk: usize, prm: &Params) -> Result<String, String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let (mut p, mut st, mut ins) = (Parts::default(), Stats::default(), Vec::new());
    let t0 = std::time::Instant::now();
    for path in inputs {
        let (text, hash) = read_input(path)?;
        let name = std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string();
        ins.push(format!("{{\"name\": {}, \"bytes\": {}, \"blake3\": {}}}", j(&name), text.len(), j(&hash)));
        ingest_text(&text, prm, &mut p, &mut st);
    }
    p.entities.sort_by_key(|e| e.id);
    p.entities.dedup_by_key(|e| e.id);
    p.value_nodes = merge_values(std::mem::take(&mut p.value_nodes))?;
    p.ref_nodes = merge_refs(std::mem::take(&mut p.ref_nodes));
    p.redirects.sort(); p.redirects.dedup();
    let (vorder, rorder) = nodes::numbering(&p.entities, &p.value_nodes, &p.ref_nodes);
    let ix = Ix::new(&vorder, &rorder);
    let mut files = vec![file_entry(out, "values.wds", &nodes::write_values(&vorder, &p.value_nodes))?,
                         file_entry(out, "refs.wds", &nodes::write_refs(&rorder, &p.ref_nodes, &ix)?)?,
                         file_entry(out, "redirects.wds", &store::write_redirects(&p.redirects))?];
    let (mut chunks, mut chunk_bytes) = (Vec::new(), 0u64);
    for (k, part) in p.entities.chunks(per_chunk.max(1)).enumerate() {
        let bytes = chunk::write(part, &ix)?;
        chunk_bytes += bytes.len() as u64;
        let e = file_entry(out, &format!("chunk_{k:05}.wds"), &bytes)?;
        chunks.push(format!("{}, \"first\": {}, \"last\": {}, \"entities\": {}}}", &e[..e.len() - 1],
            j(&part[0].id.unwrap().text()), j(&part[part.len() - 1].id.unwrap().text()), part.len()));
    }
    files.push(file_entry(out, "reverse.wds", &reverse::write(&reverse::rows(&p.entities))?)?);
    let (ne, nf, np) = crate::names::write(&p.entities)?;
    for (f, b) in [("names_exact.fst", &ne), ("names_folded.fst", &nf), ("names.post", &np)] { files.push(file_entry(out, f, b)?); }
    let undefined_v = vorder.len() - p.value_nodes.len();
    let undefined_r = rorder.len() - p.ref_nodes.len();
    let size = |f: &str| std::fs::metadata(format!("{out}/{f}")).map(|m| m.len()).unwrap_or(0);
    let (node_bytes, reverse_bytes) = (size("values.wds") + size("refs.wds"), size("reverse.wds"));
    let name_bytes = size("names_exact.fst") + size("names_folded.fst") + size("names.post");
    files.extend(chunks);
    let skipped: Vec<String> = st.skipped.iter().map(|(k, v)| format!("{}: {v}", j(k))).collect();
    let total = chunk_bytes + node_bytes + reverse_bytes + name_bytes;
    let man = format!(concat!("{{\n \"format_version\": {},\n \"code_blake3\": {},\n",
        " \"params\": {{\"langs\": {:?}, \"wikis\": {:?}, \"hot_qualifiers\": {:?}, \"per_chunk\": {}}},\n",
        " \"inputs\": [{}],\n \"files\": [\n  {}\n ],\n",
        " \"counts\": {{\"blocks\": {}, \"failed\": {}, \"entities\": {}, \"statements\": {}, \"hot_qualifiers\": {}, ",
        "\"cold_qualifiers\": {}, \"references\": {}, \"names\": {}, \"value_nodes\": {}, \"value_nodes_undefined\": {}, ",
        "\"ref_nodes\": {}, \"ref_nodes_undefined\": {}, \"redirects\": {}}},\n",
        " \"skipped_triples\": {{{}}},\n \"failed_first\": [{}],\n",
        " \"bytes\": {{\"chunks\": {}, \"nodes\": {}, \"reverse\": {}, \"names\": {}, \"total\": {}}},\n",
        " \"bytes_per_statement\": {{\"chunks\": {:.1}, \"total\": {:.1}}},\n \"build_s\": {:.1}\n}}\n"),
        chunk::VERSION, j(&crate::code_hash()), prm.langs, prm.wikis, prm.hot_qualifiers, per_chunk, ins.join(", "),
        files.join(",\n  "), st.blocks, st.failed_n, st.entities, st.statements, st.hot_qualifiers, st.cold_qualifiers,
        st.references, st.names, vorder.len(), undefined_v, rorder.len(), undefined_r, p.redirects.len(),
        skipped.join(", "), st.failed.iter().map(|s| j(s)).collect::<Vec<_>>().join(", "),
        chunk_bytes, node_bytes, reverse_bytes, name_bytes, total, chunk_bytes as f64 / st.statements.max(1) as f64,
        total as f64 / st.statements.max(1) as f64, t0.elapsed().as_secs_f64());
    std::fs::write(format!("{out}/manifest.json"), &man).map_err(|e| e.to_string())?;
    Ok(man)
}
