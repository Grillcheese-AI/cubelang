//! wdstore build --out <dir> [--scratch <dir>] [--per-chunk N] [--cold Q13442814,..|none] [--threads N]
//!               [--every N] [--stop-after N] <inputs>...
//!     inputs: the dump's bzip2 parts (read as one file; a checkpoint every N batches of 64 MB of text, and a stopped
//!     build resumes from it when run again) or gzip / plain files of entity blocks
//! wdstore get <dir> <Q42>                     one entity, as stored (a redirect followed; hot or cold said)
//! wdstore who <dir> <P31> <Q5>                the hot entities with that statement, and their ranks
//! wdstore name <dir> "Douglas Adams"          every hot entity the name may mean, in id order
//! wdstore bench <dir>                         lookup latency over a spread of ids
//! wdstore bunzip <out> <parts>...             the parts as one bzip2 file, decoded on every core
//! wdstore check <dir>                         every chunk re-written to the same bytes, the indexes rebuilt, hashes

use wdstore::{build, model::Eid, store::Store};

fn threads() -> usize { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) }

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r = match a.first().map(String::as_str) {
        Some("build") => {
            let (mut out, mut scratch, mut per, mut cold, mut th, mut ins) = (None, None, 100_000usize, "Q13442814".to_string(), threads(), Vec::new());
            let (mut every, mut stop) = (32u64, None);
            let mut it = a[1..].iter();
            while let Some(x) = it.next() {
                match x.as_str() {
                    "--out" => out = it.next().cloned(),
                    "--scratch" => scratch = it.next().cloned(),
                    "--per-chunk" => per = it.next().and_then(|v| v.parse().ok()).unwrap_or(per),
                    "--cold" => cold = it.next().cloned().unwrap_or_default(),
                    "--threads" => th = it.next().and_then(|v| v.parse().ok()).unwrap_or(th),
                    "--every" => every = it.next().and_then(|v| v.parse().ok()).unwrap_or(every).max(1),
                    "--stop-after" => stop = it.next().and_then(|v| v.parse().ok()),
                    _ => ins.push(x.clone()),
                }
            }
            let cold: Vec<Eid> = if cold == "none" { vec![] } else { cold.split(',').filter_map(Eid::parse).collect() };
            match out {
                Some(o) => {
                    let sc = scratch.unwrap_or_else(|| format!("{o}/_scratch"));
                    build::build(&ins, &o, &sc, per, cold, th, every, stop).map(|m| print!("{m}"))
                }
                None => Err("--out".into()),
            }
        }
        Some("get") if a.len() == 3 => match Eid::parse(&a[2]) {
            Some(id) => Store::open(&a[1]).and_then(|s| {
                let to = s.resolve(id)?;
                if to != id { println!("{} redirects to {}", id.text(), to.text()); }
                s.locate(to).map(|e| match e {
                    Some((cold, e)) => println!("{}\n{e:#?}", if cold { "cold" } else { "hot" }),
                    None => println!("not in the store"),
                })
            }),
            None => Err(format!("not an id: {}", a[2])),
        },
        Some("who") if a.len() == 4 => match (Eid::parse(&a[2]), Eid::parse(&a[3])) {
            (Some(p), Some(v)) if p.kind() == 'P' => Store::open(&a[1]).and_then(|s| s.who(p.number() as u32, v)).map(|h| {
                for x in &h { println!("{}\t{:?}{}", x.subject.text(), x.rank, if x.best { "\tbest" } else { "" }); }
                eprintln!("{} entities", h.len());
            }),
            _ => Err("who <dir> <P..> <id>".into()),
        },
        Some("name") if a.len() == 3 => Store::open(&a[1]).and_then(|s| s.name(&a[2])).map(|f| match f {
            wdstore::names::Found::Hits(h) => {
                for x in &h { println!("{}\t{}\t{:?}\t{:?}\t{}", x.entity.text(), x.lang, x.kind, x.how, x.key); }
                eprintln!("{} candidates", h.len());
            }
            wdstore::names::Found::TooMany(n) => println!("too many near names (over {n}): ask"),
            wdstore::names::Found::None => println!("no entity bears this name"),
        }),
        Some("bunzip") if a.len() >= 3 => {
            let th = threads();
            let mut f = std::io::BufWriter::new(std::fs::File::create(&a[1]).map_err(|e| e.to_string()).unwrap());
            let t0 = std::time::Instant::now();
            let mut n = 0u64;
            wdstore::bz2par::decode_parts(&a[2..], th, 64 << 20, |t| {
                n += t.len() as u64; std::io::Write::write_all(&mut f, t).map_err(|e| e.to_string())
            }).map(|c| {
                let s = t0.elapsed().as_secs_f64();
                eprintln!("{c} bytes in, {n} out, {s:.1} s: {:.1} MB/s in, {:.1} MB/s out, {th} threads", c as f64 / s / 1e6, n as f64 / s / 1e6);
            })
        }
        Some("bench") if a.len() == 2 => Store::open(&a[1]).and_then(|s| bench(&s)),
        Some("check") if a.len() == 2 => Store::open(&a[1]).and_then(|s| s.check()).map(|m| println!("{m}")),
        _ => Err("usage: see the head of src/main.rs".into()),
    };
    if let Err(e) = r { eprintln!("{e}"); std::process::exit(1); }
}

/// Lookup latency, from memory (the files read whole): one entity by id, then one hot name, over a spread of ids.
fn bench(s: &Store) -> Result<(), String> {
    let ids = s.ids(37);
    let pct = |mut v: Vec<f64>| { v.sort_by(|a, b| a.total_cmp(b)); let at = |p: usize| v[(v.len() - 1) * p / 100];
        format!("p50 {:.1} us, p95 {:.1} us, p99 {:.1} us (n {})", at(50), at(95), at(99), v.len()) };
    let (mut get, mut name, mut labels) = (Vec::new(), Vec::new(), Vec::new());
    for &id in &ids {
        let t = std::time::Instant::now();
        let (cold, e) = s.locate(id)?.ok_or("an indexed id not found")?;
        get.push(t.elapsed().as_secs_f64() * 1e6);
        if !cold { if let Some(n) = e.names.iter().find(|n| n.kind != wdstore::model::NameKind::Description) { labels.push(n.text.clone()); } }
    }
    for l in &labels {
        let t = std::time::Instant::now();
        if let wdstore::names::Found::None = s.name(l)? { return Err(format!("{l}: its own label not found")); }
        name.push(t.elapsed().as_secs_f64() * 1e6);
    }
    println!("get by id: {}\nname:      {}", pct(get), if name.is_empty() { "no hot names".into() } else { pct(name) });
    Ok(())
}
