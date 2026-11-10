//! wdstore build --out <dir> [--per-chunk N] <input.ttl[.gz]>...   the store, its manifest
//! wdstore get <dir> <Q42>                                        one entity, as stored (a redirect followed)
//! wdstore check <dir>                                            every file re-read whole, re-written, re-hashed

use wdstore::{build, ingest::Params, model::Eid, store::Store};

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
            Some(id) => Store::open(&a[1]).and_then(|s| {
                let to = s.resolve(id)?;
                if to != id { println!("{} redirects to {}", id.text(), to.text()); }
                s.get(to).map(|e| println!("{e:#?}"))
            }),
            None => Err(format!("not an id: {}", a[2])),
        },
        Some("check") if a.len() == 2 => Store::open(&a[1]).and_then(|s| s.check()).map(|m| println!("{m}")),
        _ => Err("usage: wdstore build --out <dir> [--per-chunk N] <inputs>... | get <dir> <id> | check <dir>".into()),
    };
    if let Err(e) = r { eprintln!("{e}"); std::process::exit(1); }
}
