//! Sorting more rows than memory holds: rows (byte strings, compared as bytes) are kept until `limit` bytes, sorted
//! and written as a run to the scratch folder; `merge` reads every run at once and hands the rows on in order (a
//! k-way merge). Equal rows are all kept. The reverse index and the names are written from these merges.

use std::collections::BinaryHeap;
use std::cmp::Reverse;
use std::io::{BufReader, BufWriter, Read, Write};

pub struct Runs { dir: String, name: String, rows: Vec<Vec<u8>>, held: usize, limit: usize, pub runs: Vec<String>, pub count: u64 }

fn put_len(w: &mut impl Write, n: usize) -> std::io::Result<()> {
    let mut x = n as u64;
    while x >= 0x80 { w.write_all(&[(x as u8) | 0x80])?; x >>= 7; }
    w.write_all(&[x as u8])
}

fn get_len(r: &mut impl Read) -> std::io::Result<Option<usize>> {
    let (mut x, mut sh) = (0u64, 0);
    loop {
        let mut b = [0u8];
        if r.read(&mut b)? == 0 {
            return if sh == 0 { Ok(None) } else { Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "run cut")) };
        }
        x |= ((b[0] & 0x7f) as u64) << sh;
        if b[0] < 0x80 { return Ok(Some(x as usize)); }
        sh += 7;
    }
}

impl Runs {
    pub fn new(dir: &str, name: &str, limit: usize) -> Runs {
        Runs { dir: dir.into(), name: name.into(), rows: vec![], held: 0, limit, runs: vec![], count: 0 }
    }

    /// Everything held written as a run; what a checkpoint records (runs, rows).
    pub fn state(&mut self) -> Result<(usize, u64), String> { self.flush()?; Ok((self.runs.len(), self.count)) }

    /// The runs as a checkpoint left them (runs written after it are written again under the same names).
    pub fn restore(dir: &str, name: &str, limit: usize, st: (usize, u64)) -> Runs {
        let runs = (0..st.0).map(|k| format!("{dir}/{name}.run{k:05}")).collect();
        Runs { dir: dir.into(), name: name.into(), rows: vec![], held: 0, limit, runs, count: st.1 }
    }

    pub fn push(&mut self, row: Vec<u8>) -> Result<(), String> {
        self.held += row.len() + 32;
        self.rows.push(row);
        self.count += 1;
        if self.held >= self.limit { self.flush()?; }
        Ok(())
    }

    /// The rows held so far, sorted, to a run on disk.
    pub fn flush(&mut self) -> Result<(), String> {
        if self.rows.is_empty() { return Ok(()); }
        self.rows.sort_unstable();
        let path = format!("{}/{}.run{:05}", self.dir, self.name, self.runs.len());
        let mut w = BufWriter::new(std::fs::File::create(&path).map_err(|e| format!("{path}: {e}"))?);
        for r in &self.rows { put_len(&mut w, r.len()).and_then(|_| w.write_all(r)).map_err(|e| e.to_string())?; }
        w.flush().map_err(|e| e.to_string())?;
        self.rows.clear(); self.held = 0;
        self.runs.push(path);
        Ok(())
    }

    /// Every row, in byte order, to `sink`; the runs are removed after (they are this build's own scratch).
    pub fn merge(mut self, mut sink: impl FnMut(&[u8]) -> Result<(), String>) -> Result<u64, String> {
        self.flush()?;
        let mut readers = Vec::new();
        for p in &self.runs { readers.push(BufReader::with_capacity(1 << 20, std::fs::File::open(p).map_err(|e| format!("{p}: {e}"))?)); }
        let next = |r: &mut BufReader<std::fs::File>| -> Result<Option<Vec<u8>>, String> {
            match get_len(r).map_err(|e| e.to_string())? {
                None => Ok(None),
                Some(n) => { let mut v = vec![0u8; n]; r.read_exact(&mut v).map_err(|e| e.to_string())?; Ok(Some(v)) }
            }
        };
        let mut heap = BinaryHeap::new();
        for (k, r) in readers.iter_mut().enumerate() { if let Some(v) = next(r)? { heap.push(Reverse((v, k))); } }
        let mut n = 0;
        while let Some(Reverse((v, k))) = heap.pop() {
            sink(&v)?; n += 1;
            if let Some(w) = next(&mut readers[k])? { heap.push(Reverse((w, k))); }
        }
        drop(readers);
        for p in &self.runs { std::fs::remove_file(p).map_err(|e| format!("{p}: {e}"))?; }
        if n != self.count { return Err(format!("{}: {n} rows merged of {}", self.name, self.count)); }
        Ok(n)
    }
}
