//! wdstore build --out <dir> [--per-chunk N] <input.ttl[.gz]>...   the store, its manifest
//! wdstore get <dir> <Q42>                                        one entity, as stored (a redirect followed)
//! wdstore who <dir> <P31> <Q5>                                  the entities with that statement, and their ranks
//! wdstore name <dir> "Douglas Adams"                             every entity the name may mean, in id order
//! wdstore check <dir>                                          every file re-read whole, re-written, re-hashed

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
            // wdstore bunzip <out> <parts>...: the parts as one bzip2 file, decoded on every core
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
            let mut f = std::io::BufWriter::new(std::fs::File::create(&a[1]).map_err(|e| e.to_string()).unwrap());
            let t0 = std::time::Instant::now();
            let mut n = 0u64;
            wdstore::bz2par::decode_parts(&a[2..], threads, 64 << 20, |t| {
                n += t.len() as u64; std::io::Write::write_all(&mut f, t).map_err(|e| e.to_string())
            }).map(|c| {
                let s = t0.elapsed().as_secs_f64();
                eprintln!("{c} bytes in, {n} out, {s:.1} s: {:.1} MB/s in, {:.1} MB/s out, {threads} threads", c as f64 / s / 1e6, n as f64 / s / 1e6);
            })
        }
        Some("bench") if a.len() == 2 => Store::open(&a[1]).and_then(|s| bench(&s)),
        Some("check") if a.len() == 2 => Store::open(&a[1]).and_then(|s| s.check()).map(|m| println!("{m}")),
        _ => Err("usage: wdstore build --out <dir> [--per-chunk N] <inputs>... | get <dir> <id> | check <dir>".into()),
    };
    if let Err(e) = r { eprintln!("{e}"); std::process::exit(1); }
}

/// Lookup latency, from memory (the files read whole): one entity by id, then one name, over a spread of ids.
fn bench(s: &Store) -> Result<(), String> {
    let ids = s.ids(37)?;
    let pct = |mut v: Vec<f64>| { v.sort_by(|a, b| a.total_cmp(b)); let at = |p: usize| v[(v.len() - 1) * p / 100];
        format!("p50 {:.1} us, p95 {:.1} us, p99 {:.1} us (n {})", at(50), at(95), at(99), v.len()) };
    let (mut get, mut name, mut labels) = (Vec::new(), Vec::new(), Vec::new());
    for &id in &ids {
        let t = std::time::Instant::now();
        let e = s.get(id)?.ok_or("an indexed id not found")?;
        get.push(t.elapsed().as_secs_f64() * 1e6);
        if let Some(n) = e.names.iter().find(|n| n.kind != wdstore::model::NameKind::Description) { labels.push(n.text.clone()); }
    }
    for l in &labels {
        let t = std::time::Instant::now();
        if let wdstore::names::Found::None = s.name(l)? { return Err(format!("{l}: its own label not found")); }
        name.push(t.elapsed().as_secs_f64() * 1e6);
    }
    println!("get by id: {}\nname:      {}", pct(get), pct(name));
    Ok(())
}
