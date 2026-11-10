//! wdstore build --out <dir> [--per-chunk N] <input.ttl[.gz]>...   the store, its manifest
//! wdstore get <dir> <Q42>                                        one entity, as stored
//! wdstore check <dir>                                            every chunk re-read whole and re-hashed

use wdstore::{build, chunk, ingest::Params, model::Eid};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r = match a.first().map(String::as_str) {
        Some("build") => {
            let (mut out, mut per, mut ins) = (None, 100_000usize, Vec::new());
            let mut it = a[1..].iter();
            while let Some(x) = it.next() {
                match x.as_str() {
                    "--out" => out = it.next().cloned(),
                    "--per-chunk" => per = it.next().and_then(|v| v.parse().ok()).unwrap_or(per),
                    _ => ins.push(x.clone()),
                }
            }
            match out { Some(o) => build::build(&ins, &o, per, &Params::default()).map(|m| print!("{m}")), None => Err("--out".into()) }
        }
        Some("get") if a.len() == 3 => match Eid::parse(&a[2]) {
            Some(id) => build::get(&a[1], id).map(|e| println!("{e:#?}")),
            None => Err(format!("not an id: {}", a[2])),
        },
        Some("check") if a.len() == 2 => check(&a[1]),
        _ => Err("usage: wdstore build --out <dir> [--per-chunk N] <inputs>... | get <dir> <id> | check <dir>".into()),
    };
    if let Err(e) = r { eprintln!("{e}"); std::process::exit(1); }
}

fn check(out: &str) -> Result<(), String> {
    let man = std::fs::read_to_string(format!("{out}/manifest.json")).map_err(|e| e.to_string())?;
    let mut k = 0;
    while let Ok(b) = std::fs::read(format!("{out}/chunk_{k:05}.wds")) {
        let h = blake3::hash(&b).to_hex().to_string();
        if !man.contains(&h) { return Err(format!("chunk {k}: hash {h} is not the manifest's")); }
        let c = chunk::Reader::open(&b)?.read_all()?;
        if chunk::write(&c)? != b { return Err(format!("chunk {k}: re-encoding differs")); }
        k += 1;
    }
    println!("{k} chunks: hashes as recorded, each re-read and re-written to the same bytes");
    Ok(())
}
