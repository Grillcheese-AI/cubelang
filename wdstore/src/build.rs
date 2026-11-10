//! The build: the dump's text in its order -> the store's files and `manifest.json`, never sorting the entities.
//! The text comes from gzip or plain files of entity blocks (the 1% sample's segments) or from the dump's bzip2
//! parts (`bz2par`, every core). It is cut into entity blocks before `\ndata:`, ingested on every core a batch at a
//! time, and handed to one writer in the dump's order (so a rebuild writes the same bytes). A block that fails to
//! parse is counted and listed, never dropped silently. Every output is append-only while the build runs; from the
//! bzip2 parts, a checkpoint every `every` batches (`resume.rs`) lets a stopped build go on where it was.

use std::io::Read;

use crate::ingest::{self, Block, Params};
use crate::model::*;
use crate::resume::Ckpt;
use crate::writer::Writer;

pub const BATCH: usize = 64 << 20;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub blocks: u64, pub failed: Vec<String>, pub failed_n: u64, pub entities: [u64; 2], pub statements: [u64; 2],
    pub hot_qualifiers: u64, pub cold_qualifiers: u64, pub references: u64, pub names: u64, pub duplicates: u64,
    pub skipped: std::collections::BTreeMap<String, u64>,
    pub dump_date: String,
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

/// The blocks of `text` ingested on `threads` threads, in order.
pub fn ingest_all(text: &str, prm: &Params, threads: usize) -> Vec<Result<Block, String>> {
    let bl: Vec<&str> = blocks(text).collect();
    let per = bl.len().div_ceil(threads.max(1)).max(1);
    std::thread::scope(|s| {
        let hs: Vec<_> = bl.chunks(per).map(|part| s.spawn(move || part.iter().map(|b| ingest::block(b, prm)).collect::<Vec<_>>())).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

/// The release the parts are, from the download's `manifest.json` beside them (name, size, hash, the record, its
/// date and licence), as written there; `null` when there is none.
fn release(first: &str) -> String {
    let dir = std::path::Path::new(first).parent().map(|p| p.join("manifest.json"));
    match dir.and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(t) => t.split_whitespace().collect::<Vec<_>>().join(" "),
        None => "null".into(),
    }
}

/// Builds `out` from `inputs`: bzip2 parts (the dump, read as one file; resumable) or gzip / plain entity blocks.
#[allow(clippy::too_many_arguments)]
pub fn build(inputs: &[String], out: &str, scratch: &str, per_chunk: usize, cold: Vec<Eid>, threads: usize, every: u64,
             stop_after: Option<u64>) -> Result<String, String> {
    build_batched(inputs, out, scratch, per_chunk, cold, threads, every, stop_after, BATCH)
}

/// `build` with the batch size given (the tests use small ones; it is part of the checkpoint's key).
#[allow(clippy::too_many_arguments)]
pub fn build_batched(inputs: &[String], out: &str, scratch: &str, per_chunk: usize, cold: Vec<Eid>, threads: usize, every: u64,
                     stop_after: Option<u64>, batch: usize) -> Result<String, String> {
    let t0 = std::time::Instant::now();
    let first = std::fs::File::open(inputs.first().ok_or("no input")?).and_then(|mut f| { let mut b = [0u8; 3]; f.read_exact(&mut b).map(|_| b) })
        .map_err(|e| e.to_string())?;
    let part = std::path::Path::new(&inputs[0]).file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("part_"));
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    if &first != b"BZh" && !part {
        let mut w = Writer::create(out, scratch, Params::default(), cold, per_chunk)?;
        let mut names = Vec::new();
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
        return w.finish(names, t0);
    }
    let sizes: Vec<u64> = inputs.iter().map(|p| std::fs::metadata(p).map(|m| m.len()).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    let key = format!("code {} per_chunk {per_chunk} cold {:?} parts {} bytes {} batch {batch} every {every}", crate::code_hash(),
                      cold.iter().map(|c| c.text()).collect::<Vec<_>>(), inputs.len(), sizes.iter().sum::<u64>());
    let ck = Ckpt::load(out, &key)?;
    let (mut w, start_bit, mut skip, mut batches) = match &ck {
        Some(c) => { eprintln!("  resuming: {} batches done, from bit {}", c.batches, c.block_bit);
                     (Writer::restore(out, scratch, Params::default(), cold, per_chunk, c)?, c.block_bit, c.skip, c.batches) }
        None => (Writer::create(out, scratch, Params::default(), cold, per_chunk)?, 0, 0, 0),
    };
    let resumed = ck.as_ref().map(|c| c.batches);
    // the blocks whose text is in `carry`: (first bit, text length); `head` bytes of the first are already used
    let (mut carry, mut held, mut head): (Vec<u8>, std::collections::VecDeque<(u64, usize)>, usize) = (vec![], Default::default(), skip as usize);
    let mut started = ck.is_none();
    let mut stopped = false;
    let n = crate::bz2par::decode_from(inputs, start_bit / 8, threads, 64 << 20, |t, bit| {
        if stopped { return Ok(()); }
        if !started {
            if bit != start_bit { return Err(format!("the checkpoint's block is at bit {start_bit}, the first decoded at {bit}")); }
            started = true;
        }
        held.push_back((bit, t.len()));
        carry.extend_from_slice(&t[std::mem::take(&mut skip) as usize..]);
        if carry.len() < batch { return Ok(()); }
        let Some(cut) = carry.windows(6).rposition(|x| x == b"\ndata:").map(|i| i + 1) else { return Ok(()) };
        w.text(std::str::from_utf8(&carry[..cut]).map_err(|_| "the dump's text is not UTF-8")?, threads)?;
        carry.drain(..cut);
        let mut used = head + cut;
        while let Some(&(_, len)) = held.front() { if used >= len { used -= len; held.pop_front(); } else { break; } }
        head = used;
        batches += 1;
        if batches % every == 0 {
            let (b, _) = *held.front().ok_or("no block holds the next entity")?;
            w.checkpoint(&key, b, head as u64, batches)?;
            eprintln!("  checkpoint: {batches} batches, {} entities, {} s", w.st.entities[0] + w.st.entities[1], t0.elapsed().as_secs());
            if stop_after.is_some_and(|s| batches >= s) { stopped = true; }
        }
        Ok(())
    })?;
    if stopped { return Ok(format!("stopped after {batches} batches (a test of resuming)\n")); }
    w.text(std::str::from_utf8(&carry).map_err(|_| "the dump's text is not UTF-8")?, threads)?;
    let mut man = w.finish(vec![format!("{{\"parts\": {}, \"bytes\": {n}, \"resumed_at_batch\": {}, \"release\": {}}}", inputs.len(),
                                        resumed.map(|b| b.to_string()).unwrap_or("null".into()), release(&inputs[0]))], t0)?;
    if let Some(c) = ck { man = man.replace("\"build_s\"", &format!("\"resumed_from_bit\": {},\n \"build_s\"", c.block_bit)); }
    let _ = std::fs::rename(format!("{out}/{}", crate::resume::FILE), format!("{out}/checkpoint.done.txt"));
    Ok(man)
}
