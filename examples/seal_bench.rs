//! What sealing costs (2026-10-24): one seal and one open, and a history appended turn by turn,
//! plain and sealed. `cargo run --release --example seal_bench -- [TURNS]`

use std::time::Instant;

use cubelang::auth::{keygen, Identity, Kind, Protect};
use cubelang::seal::{self, SealedVault};
use cubelang::vm::history::History;

fn main() {
    let turns: usize = std::env::args().nth(1).and_then(|n| n.parse().ok()).unwrap_or(500);
    let (file, entry) = keygen("Bench", Kind::Model, None, Protect::Plaintext).unwrap();
    let me = Identity::load(&file, None).unwrap();
    let body = vec![b'x'; 1024];

    let n = 200;
    let t = Instant::now();
    let mut last = Vec::new();
    for _ in 0..n {
        last = seal::seal(&entry, &body, b"", Some(&me)).unwrap();
    }
    let seal_us = t.elapsed().as_micros() as f64 / n as f64;
    let t = Instant::now();
    for _ in 0..n {
        seal::open_from(&me, &last, b"", &entry).unwrap();
    }
    let open_us = t.elapsed().as_micros() as f64 / n as f64;
    println!("1 KiB, signed: seal {:.0} us, open + verify {:.0} us; {} bytes sealed", seal_us, open_us, last.len());

    let vault = SealedVault::own(Identity::load(&file, None).unwrap());
    for sealed in [false, true] {
        let dir = std::env::temp_dir().join(format!("cubelang-seal-bench-{}-{}", std::process::id(), sealed));
        let _ = std::fs::remove_dir_all(&dir);
        let v = if sealed { Some(&vault as &dyn cubelang::vm::segment::Vault) } else { None };
        let t = Instant::now();
        for i in 0..turns {
            let mut h = History::open_dir_in(&dir, v).unwrap();
            h.append("user", &format!("turn {} about the printing press and Gutenberg in Mainz", i), i as u64);
            h.freeze().unwrap();
            h.maybe_merge(32).unwrap();
            h.save_dir_in(&dir, v).unwrap();
        }
        let per = t.elapsed().as_micros() as f64 / turns as f64;
        let t = Instant::now();
        let h = History::open_dir_in(&dir, v).unwrap();
        println!("{} history, {} turns: open + append + save {:.0} us a turn; open {} segments {:.1} ms",
                 if sealed { "sealed" } else { "plain " }, turns, per, h.segment_count(), t.elapsed().as_secs_f64() * 1e3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
