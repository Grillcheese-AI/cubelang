//! bzip2 decoded on every core: a bzip2 block is independent of the others, so each is found by its 48-bit start
//! marker (at any bit offset), wrapped as a one-block stream of its own (its stream CRC is its block CRC, read from
//! its header), and decoded on a thread; the texts are handed on in the file's order. Concatenated streams (the
//! dump's shards) need nothing more: a block ends where the next block or a stream's end marker begins.
//!
//! A start marker can also occur by chance inside compressed data (about once in 2^48 bit positions). A block that
//! fails to decode is merged with the next one and decoded again; one that still fails stops the pass.

use std::io::Read;

const BLOCK: u64 = 0x3141_5926_5359;
const END: u64 = 0x1772_4538_5090;

/// Bit positions of every block start and stream end marker in `data`, sorted, with which it is (true: a block).
pub fn markers(data: &[u8]) -> Vec<(u64, bool)> {
    let mut out = Vec::new();
    for (magic, is_block) in [(BLOCK, true), (END, false)] {
        for shift in 0..8u32 {
            // the marker starting `shift` bits into a byte: bytes 1..5 of that 7-byte window are fixed
            let w = (magic << (64 - 48 - shift)).to_be_bytes();
            let mid = &w[1..5];
            for i in memchr::memmem::find_iter(data, mid) {
                if i == 0 { continue; }
                let start = (i as u64 - 1) * 8 + shift as u64;
                if bits(data, start, 48) == Some(magic) { out.push((start, is_block)); }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `n` (<= 64) bits of `data` from bit `at`, big-endian; None past the end.
fn bits(data: &[u8], at: u64, n: u32) -> Option<u64> {
    if at + n as u64 > data.len() as u64 * 8 { return None; }
    let mut x = 0u64;
    for k in 0..n as u64 {
        let b = at + k;
        x = x << 1 | ((data[(b / 8) as usize] >> (7 - b % 8)) & 1) as u64;
    }
    Some(x)
}

struct Bits { out: Vec<u8>, acc: u64, fill: u32 }

impl Bits {
    fn push(&mut self, v: u64, w: u32) {
        for k in (0..w).rev() {
            self.acc = self.acc << 1 | (v >> k) & 1; self.fill += 1;
            if self.fill == 8 { self.out.push(self.acc as u8); self.acc = 0; self.fill = 0; }
        }
    }
    fn byte(&mut self, b: u8) {
        if self.fill == 0 { self.out.push(b) } else { self.push(b as u64, 8) }
    }
}

/// The bits [a, b) of `data` as a one-block bzip2 stream.
fn wrap(data: &[u8], a: u64, b: u64) -> Option<Vec<u8>> {
    let crc = bits(data, a + 48, 32)?;
    if b > data.len() as u64 * 8 { return None; }
    let mut w = Bits { out: Vec::with_capacity(((b - a) / 8) as usize + 20), acc: 0, fill: 0 };
    w.out.extend_from_slice(b"BZh9");
    let mut at = a;
    while at < b && at % 8 != 0 { w.push(bits(data, at, 1)?, 1); at += 1; }
    if w.fill == 0 {
        let end = b / 8;
        w.out.extend_from_slice(&data[(at / 8) as usize..end as usize]);
        at = end * 8;
    } else {
        while at + 8 <= b { w.byte(data[(at / 8) as usize]); at += 8; }
    }
    while at < b { w.push(bits(data, at, 1)?, 1); at += 1; }
    w.push(END, 48);
    w.push(crc, 32);
    if w.fill > 0 { let pad = 8 - w.fill; w.push(0, pad); }
    Some(w.out)
}

fn decode(stream: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    bzip2::read::BzDecoder::new(stream).read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The blocks of `data` that end inside it, decoded on `threads` threads, in order, each with the bit it starts at;
/// and the bit where the first
/// block that does not end inside `data` begins (the caller keeps the bytes from there for the next read).
pub fn decode_complete(data: &[u8], threads: usize, last: bool) -> Result<(Vec<(u64, Vec<u8>)>, u64), String> {
    let m = markers(data);
    let end_bits = data.len() as u64 * 8;
    // spans: each block start to the next marker of either kind
    let mut spans: Vec<(u64, u64)> = Vec::new();
    let mut keep = m.iter().find(|x| x.1).map(|x| x.0).unwrap_or(end_bits);
    for (k, &(at, is_block)) in m.iter().enumerate() {
        if !is_block { continue; }
        match m.get(k + 1) {
            Some(&(next, _)) => spans.push((at, next)),
            None => { keep = at; break; }
        }
        keep = m.get(k + 1).map(|x| x.0).unwrap_or(end_bits);
    }
    if !last {
        // the last span may be cut short by the window's end only if its end is a block start we found: fine
    } else if m.last().is_some_and(|x| x.1) {
        return Err("the stream ends inside a block".into());
    }
    let n = spans.len();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let one = |i: usize| wrap(data, spans[i].0, spans[i].1).ok_or_else(|| "a block past the data".to_string()).and_then(|w| decode(&w));
    let mut outs: Vec<Option<Result<Vec<u8>, String>>> = (0..n).map(|_| None).collect();
    let got: Vec<Vec<(usize, Result<Vec<u8>, String>)>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads.max(1)).map(|_| s.spawn(|| {
            let mut mine = Vec::new();
            loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= n { return mine; }
                mine.push((i, one(i)));
            }
        })).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (i, r) in got.into_iter().flatten() { outs[i] = Some(r); }
    // a failed span: a chance marker cut it; merge it with the next span and decode again
    let mut res = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        match outs[i].take().unwrap() {
            Ok(v) => { res.push((spans[i].0, v)); i += 1; }
            Err(e) => {
                let mut j = i + 1;
                let mut got = None;
                while j < n && j <= i + 4 {
                    if let Ok(v) = wrap(data, spans[i].0, spans[j].1).ok_or("past").map_err(String::from).and_then(|w| decode(&w)) {
                        got = Some(v); break;
                    }
                    j += 1;
                }
                match got {
                    Some(v) => { res.push((spans[i].0, v)); for k in i + 1..=j { outs[k] = None; } i = j + 1; }
                    None => return Err(format!("a block at bit {} does not decode: {e}", spans[i].0)),
                }
            }
        }
    }
    Ok((res, keep))
}

/// Every part in order, as one bzip2 file, decoded; `sink` gets the text in order, a block's worth at a time.
/// `window` bytes are read at a time (each block is under 1 MB compressed).
pub fn decode_parts(parts: &[String], threads: usize, window: usize, mut sink: impl FnMut(&[u8]) -> Result<(), String>) -> Result<u64, String> {
    decode_from(parts, 0, threads, window, |t, _| sink(t))
}

/// The same from byte `start` of the parts taken as one file (a block that starts in that byte, for a resumed
/// build); `sink` also gets each block's first bit, counted from the start of the first part.
pub fn decode_from(parts: &[String], start: u64, threads: usize, window: usize,
                   mut sink: impl FnMut(&[u8], u64) -> Result<(), String>) -> Result<u64, String> {
    use std::io::{Seek, SeekFrom};
    let sizes: Vec<u64> = parts.iter().map(|p| std::fs::metadata(p).map(|m| m.len()).map_err(|e| format!("{p}: {e}"))).collect::<Result<_, _>>()?;
    let (mut k, mut skip) = (0usize, start);
    while k < parts.len() && skip >= sizes[k] { skip -= sizes[k]; k += 1; }
    let mut buf: Vec<u8> = Vec::new();
    let (mut base, mut compressed) = (start, 0u64);   // base: the byte of the parts that buf[0] is
    let mut files = parts[k.min(parts.len())..].iter();
    let mut cur: Option<std::fs::File> = None;
    if let Some(p) = files.next() {
        let mut f = std::fs::File::open(p).map_err(|e| format!("{p}: {e}"))?;
        f.seek(SeekFrom::Start(skip)).map_err(|e| e.to_string())?;
        cur = Some(f);
    }
    loop {
        let mut chunk = vec![0u8; window];
        let mut got = 0;
        while got < window {
            if cur.is_none() {
                match files.next() { Some(p) => cur = Some(std::fs::File::open(p).map_err(|e| format!("{p}: {e}"))?), None => break }
            }
            let n = cur.as_mut().unwrap().read(&mut chunk[got..]).map_err(|e| e.to_string())?;
            if n == 0 { cur = None; } else { got += n; }
        }
        chunk.truncate(got);
        compressed += got as u64;
        let last = got < window && cur.is_none();
        buf.extend_from_slice(&chunk);
        let (texts, keep) = decode_complete(&buf, threads, last)?;
        for (bit, t) in &texts { sink(t, base * 8 + bit)?; }
        if last { return Ok(compressed); }
        let keep_byte = ((keep / 8) as usize).min(buf.len());
        buf.drain(..keep_byte);
        base += keep_byte as u64;
    }
}
