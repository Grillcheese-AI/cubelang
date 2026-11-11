//! The parallel bzip2 decoder gives exactly what the reference decoder gives: many blocks, two streams one after
//! the other (as the dump's shards), the file cut into parts at sizes that fall inside blocks, small windows.

use std::io::{Read, Write};

use wdstore::bz2par::{decode_parts, markers};

fn text(n: usize, seed: u64) -> Vec<u8> {
    // varied enough that blocks compress unevenly; deterministic
    let mut x = seed;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        let w = format!("wd:Q{} p:P{} \"{}\"@en ;\n", x % 1_000_000, x % 3000, x.wrapping_mul(2654435761) % 99991);
        out.extend_from_slice(w.as_bytes());
    }
    out.truncate(n);
    out
}

fn bz(data: &[u8], level: u32) -> Vec<u8> {
    let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(level));
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

#[test]
fn many_blocks_two_streams_odd_parts_decode_to_the_reference() {
    let (a, b) = (text(1_700_000, 1), text(900_000, 2));
    let file: Vec<u8> = [bz(&a, 1), bz(&b, 1)].concat();      // level 1: 100k blocks, so many of them
    let mut want = Vec::new();
    bzip2::read::MultiBzDecoder::new(&file[..]).read_to_end(&mut want).unwrap();
    assert_eq!(want, [a, b].concat());
    let blocks = markers(&file).iter().filter(|m| m.1).count();
    assert!(blocks >= 20, "{blocks} blocks");
    let dir = std::env::temp_dir().join(format!("wdstore_bz_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for cut in [77_777usize, 250_001] {
        let mut parts = Vec::new();
        for (k, c) in file.chunks(cut).enumerate() {
            let p = dir.join(format!("part_{cut}_{k:05}"));
            std::fs::write(&p, c).unwrap();
            parts.push(p.to_str().unwrap().to_string());
        }
        for (threads, window) in [(1, 300_000), (4, 300_000), (8, 2_000_000)] {
            let mut got = Vec::new();
            let n = decode_parts(&parts, threads, window, |t| { got.extend_from_slice(t); Ok(()) }).unwrap();
            assert_eq!(n, file.len() as u64);
            assert!(got == want, "cut {cut}, {threads} threads, window {window}: {} of {} bytes", got.len(), want.len());
        }
    }
}

#[test]
fn a_file_cut_short_inside_a_block_is_an_error() {
    let file = bz(&text(400_000, 3), 1);
    let dir = std::env::temp_dir().join(format!("wdstore_bz_cut_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("part_00000");
    std::fs::write(&p, &file[..file.len() - 5000]).unwrap();
    let r = decode_parts(&[p.to_str().unwrap().to_string()], 2, 1_000_000, |_| Ok(()));
    assert!(r.is_err());
}

#[test]
fn decoding_from_a_block_gives_the_text_from_that_block_on() {
    let a = text(1_500_000, 4);
    let file = [bz(&a, 1), bz(&text(300_000, 5), 1)].concat();
    let dir = std::env::temp_dir().join(format!("wdstore_bz_from_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut parts = Vec::new();
    for (k, c) in file.chunks(123_457).enumerate() {
        let p = dir.join(format!("part_{k:05}"));
        std::fs::write(&p, c).unwrap();
        parts.push(p.to_str().unwrap().to_string());
    }
    let (mut all, mut at) = (Vec::new(), Vec::new());
    wdstore::bz2par::decode_from(&parts, 0, 3, 400_000, |t, bit| { at.push((bit, all.len())); all.extend_from_slice(t); Ok(()) }).unwrap();
    let (bit, off) = at[11];
    let mut got = Vec::new();
    let mut first = None;
    wdstore::bz2par::decode_from(&parts, bit / 8, 3, 400_000, |t, b| { first.get_or_insert(b); got.extend_from_slice(t); Ok(()) }).unwrap();
    assert_eq!(first, Some(bit));
    assert!(got == all[off..], "{} vs {}", got.len(), all.len() - off);
}
