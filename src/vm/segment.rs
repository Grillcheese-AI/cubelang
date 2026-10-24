//! Immutable store segments (2026-10-24): the frozen layers of the knowledge store and the
//! conversation history (`knowledge.rs`, `history.rs`).
//!
//! A segment is a sorted key index -- a finite-state transducer (`fst::Map`: keys share prefixes
//! and suffixes, so millions of names take a fraction of their text size) mapping each key to an
//! offset in a value blob. It never changes once built. A process loads its bytes once and every
//! request and every branch shares them through an `Arc`: forking a store costs a pointer copy, not
//! a copy of the store. Changes go in a small mutable top layer; freezing that layer adds a new
//! segment; merging folds segments together (an LSM tree, as a log-structured database does).
//!
//! ## File format
//!
//! ```text
//! [8]  magic "CUBESEG\0"
//! [1]  version (1)
//! [1]  kind (1 = knowledge, 2 = history)
//! [6]  reserved, zero
//! [8]  fst length, u64 LE
//! [8]  blob length, u64 LE
//! [..] fst bytes
//! [..] blob bytes
//! [32] BLAKE3 of everything above
//! ```
//!
//! The checksum catches a damaged or truncated file before anything reads it. It is not a
//! signature: who wrote a segment, and keeping it secret at rest, are the sealed-content layer's
//! job (task #118).

use std::sync::Arc;

pub const MAGIC: &[u8; 8] = b"CUBESEG\0";
pub const VERSION: u8 = 1;
pub const KIND_KNOWLEDGE: u8 = 1;
pub const KIND_HISTORY: u8 = 2;
const HEADER: usize = 32;
const CHECKSUM: usize = 32;

/// A window onto a shared, immutable buffer: what an `fst::Map` reads from without copying.
#[derive(Clone)]
pub struct Bytes {
    buf: Arc<[u8]>,
    start: usize,
    end: usize,
}

impl Bytes {
    pub fn new(buf: Arc<[u8]>) -> Self {
        let end = buf.len();
        Bytes { buf, start: 0, end }
    }

    fn window(&self, start: usize, end: usize) -> Self {
        Bytes { buf: self.buf.clone(), start: self.start + start, end: self.start + end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.buf[self.start..self.end]
    }
}

impl std::fmt::Debug for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bytes({} bytes)", self.len())
    }
}

/// A segment file's two parts, checked.
#[derive(Debug)]
pub struct SegmentFile {
    pub kind: u8,
    pub fst: Bytes,
    pub blob: Bytes,
}

/// Encode a segment file.
pub fn encode(kind: u8, fst: &[u8], blob: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + fst.len() + blob.len() + CHECKSUM);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(kind);
    out.extend_from_slice(&[0u8; 6]);
    out.extend_from_slice(&(fst.len() as u64).to_le_bytes());
    out.extend_from_slice(&(blob.len() as u64).to_le_bytes());
    out.extend_from_slice(fst);
    out.extend_from_slice(blob);
    let sum = blake3::hash(&out);
    out.extend_from_slice(sum.as_bytes());
    out
}

/// Check and split a segment file's bytes (no copy: the parts are windows onto `buf`).
pub fn decode(buf: Arc<[u8]>, want_kind: u8) -> Result<SegmentFile, String> {
    let n = buf.len();
    if n < HEADER + CHECKSUM || &buf[..8] != MAGIC {
        return Err("not a CubeLang segment (bad magic or too short)".into());
    }
    if buf[8] != VERSION {
        return Err(format!("segment version {} (this build reads {})", buf[8], VERSION));
    }
    if buf[9] != want_kind {
        return Err(format!("segment kind {} where kind {} was expected", buf[9], want_kind));
    }
    let sum = blake3::hash(&buf[..n - CHECKSUM]);
    if sum.as_bytes()[..] != buf[n - CHECKSUM..] {
        return Err("segment checksum mismatch: the file is damaged or truncated".into());
    }
    let fst_len = u64::from_le_bytes(buf[16..24].try_into().unwrap()) as usize;
    let blob_len = u64::from_le_bytes(buf[24..32].try_into().unwrap()) as usize;
    if HEADER.checked_add(fst_len).and_then(|x| x.checked_add(blob_len)).and_then(|x| x.checked_add(CHECKSUM)) != Some(n) {
        return Err("segment lengths do not add up".into());
    }
    let all = Bytes::new(buf);
    Ok(SegmentFile {
        kind: want_kind,
        fst: all.window(HEADER, HEADER + fst_len),
        blob: all.window(HEADER + fst_len, HEADER + fst_len + blob_len),
    })
}

/// Read and check a segment file. Read once per process: the store shares the bytes.
pub fn read_file(path: &std::path::Path, want_kind: u8) -> Result<SegmentFile, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    decode(Arc::from(data.into_boxed_slice()), want_kind).map_err(|e| format!("{}: {}", path.display(), e))
}

/// Write a file whole or not at all: to a temporary beside it, then renamed over it.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

// ── Varints (LEB128) ────────────────────────────────────────────────────────

pub fn put_uvarint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub fn get_uvarint(buf: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*at)?;
        *at += 1;
        if shift >= 64 {
            return None;
        }
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
    }
}

pub fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_uvarint(out, b.len() as u64);
    out.extend_from_slice(b);
}

pub fn get_str<'a>(buf: &'a [u8], at: &mut usize) -> Option<&'a str> {
    let n = get_uvarint(buf, at)? as usize;
    let end = at.checked_add(n)?;
    let s = std::str::from_utf8(buf.get(*at..end)?).ok()?;
    *at = end;
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_segment_round_trips_and_damage_is_caught() {
        let file = encode(KIND_KNOWLEDGE, b"fst-bytes", b"the blob");
        let seg = decode(Arc::from(file.clone().into_boxed_slice()), KIND_KNOWLEDGE).unwrap();
        assert_eq!(seg.fst.as_ref(), b"fst-bytes");
        assert_eq!(seg.blob.as_ref(), b"the blob");

        let mut bad = file.clone();
        bad[HEADER + 2] ^= 1;
        assert!(decode(Arc::from(bad.into_boxed_slice()), KIND_KNOWLEDGE).unwrap_err().contains("checksum"));
        let short = file[..file.len() - 1].to_vec();
        assert!(decode(Arc::from(short.into_boxed_slice()), KIND_KNOWLEDGE).is_err());
        assert!(decode(Arc::from(file.into_boxed_slice()), KIND_HISTORY).unwrap_err().contains("kind"));
    }

    #[test]
    fn varints_round_trip() {
        let mut out = Vec::new();
        for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
            put_uvarint(&mut out, v);
        }
        put_bytes(&mut out, "héllo".as_bytes());
        let mut at = 0;
        for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
            assert_eq!(get_uvarint(&out, &mut at), Some(v));
        }
        assert_eq!(get_str(&out, &mut at), Some("héllo"));
        assert_eq!(get_uvarint(&out, &mut at), None);
    }
}
