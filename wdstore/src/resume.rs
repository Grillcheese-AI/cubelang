//! A build's checkpoint (`checkpoint.txt`, one `key value` a line): where the text resumes (the bzip2 block's first
//! bit, the bytes of its text already used, the batches done), every append-only file's length, the node logs'
//! dictionaries and the counts. A build that finds one for the same code, parameters and parts goes on from it; one
//! for anything else is set aside (`checkpoint.stale.txt`) and the build starts over.

use crate::build::Stats;
use crate::nodes::LogState;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ckpt {
    pub key: String, pub block_bit: u64, pub skip: u64, pub batches: u64,
    pub values: Option<LogState>, pub refs: Option<LogState>,
    pub ids: u64, pub redirects: u64, pub chunk_log: u64, pub chunks: [u32; 2], pub bytes: [u64; 2], pub runs: [(usize, u64); 3],
    pub stats: Stats,
}

fn esc(s: &str) -> String { s.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r") }

fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' { o.push(c); continue; }
        match it.next() { Some('n') => o.push('\n'), Some('r') => o.push('\r'), Some(x) => o.push(x), None => {} }
    }
    o
}

pub const FILE: &str = "checkpoint.txt";

impl Ckpt {
    pub fn text(&self) -> String {
        let mut o = format!("key {}\nblock_bit {}\nskip {}\nbatches {}\nids {}\nredirects {}\nchunk_log {}\n", esc(&self.key),
                            self.block_bit, self.skip, self.batches, self.ids, self.redirects, self.chunk_log);
        o += &format!("chunks {} {}\nbytes {} {}\n", self.chunks[0], self.chunks[1], self.bytes[0], self.bytes[1]);
        for (k, r) in self.runs.iter().enumerate() { o += &format!("runs {k} {} {}\n", r.0, r.1); }
        for (name, l) in [("values", &self.values), ("refs", &self.refs)] {
            let l = l.as_ref().unwrap();
            o += &format!("{name} {} {} {}\n", l.pos, l.num_pos, l.defined);
            for d in &l.dict { o += &format!("{name}_dict {}\n", esc(d)); }
        }
        let s = &self.stats;
        o += &format!("stats {} {} {} {} {} {} {} {} {} {} {}\n", s.blocks, s.failed_n, s.entities[0], s.entities[1],
                      s.statements[0], s.statements[1], s.hot_qualifiers, s.cold_qualifiers, s.references, s.names, s.duplicates);
        for f in &s.failed { o += &format!("failed {}\n", esc(f)); }
        for (k, v) in &s.skipped { o += &format!("skipped {v} {}\n", esc(k)); }
        o += &format!("dump_date {}\n", esc(&s.dump_date));
        o + "end\n"
    }

    pub fn parse(t: &str) -> Result<Ckpt, String> {
        let mut c = Ckpt::default();
        let (mut vals, mut refs) = (LogState { pos: 0, num_pos: 0, defined: 0, dict: vec![] }, LogState { pos: 0, num_pos: 0, defined: 0, dict: vec![] });
        let mut ended = false;
        for line in t.lines() {
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            let n = |i: usize| -> Result<u64, String> { v.split(' ').nth(i).and_then(|x| x.parse().ok()).ok_or(format!("checkpoint: {line}")) };
            match k {
                "key" => c.key = unesc(v),
                "block_bit" => c.block_bit = n(0)?, "skip" => c.skip = n(0)?, "batches" => c.batches = n(0)?,
                "ids" => c.ids = n(0)?, "redirects" => c.redirects = n(0)?, "chunk_log" => c.chunk_log = n(0)?,
                "chunks" => c.chunks = [n(0)? as u32, n(1)? as u32], "bytes" => c.bytes = [n(0)?, n(1)?],
                "runs" => c.runs[n(0)? as usize] = (n(1)? as usize, n(2)?),
                "values" => { vals.pos = n(0)?; vals.num_pos = n(1)?; vals.defined = n(2)?; }
                "refs" => { refs.pos = n(0)?; refs.num_pos = n(1)?; refs.defined = n(2)?; }
                "values_dict" => vals.dict.push(unesc(v)), "refs_dict" => refs.dict.push(unesc(v)),
                "stats" => {
                    let s = &mut c.stats;
                    s.blocks = n(0)?; s.failed_n = n(1)?; s.entities = [n(2)?, n(3)?]; s.statements = [n(4)?, n(5)?];
                    s.hot_qualifiers = n(6)?; s.cold_qualifiers = n(7)?; s.references = n(8)?; s.names = n(9)?; s.duplicates = n(10)?;
                }
                "failed" => c.stats.failed.push(unesc(v)),
                "skipped" => { let (cnt, name) = v.split_once(' ').ok_or("checkpoint: skipped")?;
                               c.stats.skipped.insert(unesc(name), cnt.parse().map_err(|_| "checkpoint: skipped")?); }
                "dump_date" => c.stats.dump_date = unesc(v),
                "end" => ended = true,
                _ => return Err(format!("checkpoint: unknown line {line}")),
            }
        }
        if !ended { return Err("checkpoint: cut short".into()); }
        c.values = Some(vals); c.refs = Some(refs);
        Ok(c)
    }

    /// Written to a temporary file, synced, then renamed over the last one: a stop leaves the old or the new whole.
    pub fn save(&self, out: &str) -> Result<(), String> {
        use std::io::Write;
        let tmp = format!("{out}/{FILE}.part");
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(self.text().as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, format!("{out}/{FILE}")).map_err(|e| e.to_string())
    }

    /// The checkpoint to resume from: none there, or set aside when it was written for another build.
    pub fn load(out: &str, key: &str) -> Result<Option<Ckpt>, String> {
        let path = format!("{out}/{FILE}");
        let Ok(t) = std::fs::read_to_string(&path) else { return Ok(None) };
        let c = Ckpt::parse(&t)?;
        if c.key != key {
            std::fs::rename(&path, format!("{out}/checkpoint.stale.txt")).map_err(|e| e.to_string())?;
            eprintln!("  a checkpoint for another build set aside (checkpoint.stale.txt); starting over");
            return Ok(None);
        }
        Ok(Some(c))
    }
}
