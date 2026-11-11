//! wdstore build --out <dir> [--scratch <dir>] [--per-chunk N] [--cold Q13442814,..|none] [--threads N]
//!               [--every N] [--stop-after N] <inputs>...
//!     inputs: the dump's bzip2 parts (read as one file; a checkpoint every N batches of 64 MB of text, and a stopped
//!     build resumes from it when run again) or gzip / plain files of entity blocks
//! wdstore get <dir> <Q42>                     one entity, as stored (a redirect followed; hot or cold said)
//! wdstore who <dir> <P31> <Q5>                the hot entities with that statement, and their ranks
//! wdstore name <dir> "Douglas Adams"          every hot entity the name may mean, in id order
//! wdstore resolve <dir> <ids.txt>            each id: hot / cold / missing, or the id it redirects to
//! wdstore dump <dir> <ids.txt|all>             entities as stored, one JSON line each
//! wdstore cues <dir> <out dir>                 which kinds each value word goes with; the fictional classes
//! wdstore kinds <dir> <out dir>                which properties each class's members hold (CubbyLLM 0055's kinds table)
//! wdstore subclasses <dir> <out dir>           every best-rank subclass-of edge (CubbyLLM 0066's class-roots table)
//! wdstore bench <dir> [offset]                lookup latency over a spread of ids, hot and cold apart
//! wdstore bunzip <out> <parts>...             the parts as one bzip2 file, decoded on every core
//! wdstore check <dir>                         every chunk re-written to the same bytes, the indexes rebuilt, hashes

use wdstore::{build, model::Eid, store::Store};

/// mimalloc, not the system heap: ingest's threads allocate many small strings, and Windows' heap serialized them
/// (1.2 of 12 cores busy in the full build, 2026-11-10).
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn threads() -> usize { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) }

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r = match a.first().map(String::as_str) {
        Some("build") => {
            let (mut out, mut scratch, mut per, mut cold, mut th, mut ins) = (None, None, 100_000usize, "Q13442814".to_string(), threads(), Vec::new());
            let (mut every, mut stop) = (64u64, None);
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
        Some("dump") if a.len() == 3 => Store::open(&a[1]).and_then(|s| {
            // every entity of the store, or those of an ids file (one a line), as JSON lines
            let ids: Vec<Eid> = if a[2] == "all" { s.ids(1) } else {
                std::fs::read_to_string(&a[2]).map_err(|e| e.to_string())?.lines().filter_map(|l| Eid::parse(l.trim())).collect() };
            let mut o = std::io::BufWriter::new(std::io::stdout().lock());
            for id in ids {
                if let Some((cold, e)) = s.locate(id)? {
                    std::io::Write::write_all(&mut o, (wdstore::export::entity(&e, cold) + "\n").as_bytes()).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }),
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
        Some("resolve") if a.len() == 3 => Store::open(&a[1]).and_then(|s| {
            // one id a line in; out: the id, then hot / cold / missing, or "-> <id that remains>" and where that is
            let text = std::fs::read_to_string(&a[2]).map_err(|e| e.to_string())?;
            let mut o = std::io::BufWriter::new(std::io::stdout().lock());
            let mut n = [0u64; 4];
            for id in text.lines().filter_map(|l| Eid::parse(l.trim())) {
                let to = s.resolve(id)?;
                let at = match s.locate(to)? { Some((true, _)) => "cold", Some((false, _)) => "hot", None => "missing" };
                let k = if to != id { 3 } else { ["hot", "cold", "missing"].iter().position(|x| *x == at).unwrap() };
                n[k] += 1;
                let line = if to != id { format!("{}\t-> {}\t{at}\n", id.text(), to.text()) } else { format!("{}\t{at}\n", id.text()) };
                std::io::Write::write_all(&mut o, line.as_bytes()).map_err(|e| e.to_string())?;
            }
            eprintln!("hot {}, cold {}, missing {}, through a redirect {}", n[0], n[1], n[2], n[3]);
            Ok(())
        }),
        Some("snapshot") if a.len() == 2 || (a.len() == 4 && a[2] == "--at") => {
            let upto = if a.len() == 4 { a[3].parse().map(Some).map_err(|_| "--at N".to_string()) } else { Ok(None) };
            upto.and_then(|u| wdstore::snapshot::at(&a[1], u)).map(|s| println!(
                "{{\"snapshot\": \"{}\", \"manifest\": \"{}\", \"changes\": {}, \"head\": \"{}\"}}", s.hash, s.manifest, s.changes, s.head))
        }
        Some("serve") if a.len() == 2 => wdstore::serve::serve(&a[1], std::io::stdin().lock(), std::io::stdout().lock()),
        Some("probe") if a.len() >= 2 => {
            // the parts' entities ingested one at a time, timed: the slowest, with their sizes
            let mut carry: Vec<u8> = Vec::new();
            let mut times: Vec<(u128, usize, String)> = Vec::new();
            let prm = wdstore::ingest::Params::default();
            let r = wdstore::bz2par::decode_parts(&a[1..], threads(), 64 << 20, |t| {
                carry.extend_from_slice(t);
                if let Some(cut) = carry.windows(6).rposition(|x| x == b"\ndata:").map(|i| i + 1) {
                    let text = String::from_utf8_lossy(&carry[..cut]).to_string();
                    for b in wdstore::build::blocks(&text) {
                        let t0 = std::time::Instant::now();
                        let _ = wdstore::ingest::block(b, &prm);
                        times.push((t0.elapsed().as_micros(), b.len(), b.chars().take(24).collect()));
                    }
                    carry.drain(..cut);
                }
                Ok(())
            });
            if let Err(e) = r { eprintln!("(decoding stopped: {e})"); }
            let total: u128 = times.iter().map(|x| x.0).sum();
            let bytes: usize = times.iter().map(|x| x.1).sum();
            times.sort_by(|x, y| y.0.cmp(&x.0));
            println!("{} entities, {} MB, {} ms in all (one thread)", times.len(), bytes >> 20, total / 1000);
            for (us, b, head) in times.iter().take(12) { println!("  {:>8} ms  {:>9} bytes  {head}", us / 1000, b); }
            Ok(())
        }
        Some("bench") if a.len() == 2 || a.len() == 3 => Store::open(&a[1])
            .and_then(|s| bench(&s, a.get(2).map(|x| x.parse().unwrap_or(0)).unwrap_or(0))),
        Some("cues") if a.len() == 3 => Store::open(&a[1]).and_then(|s| wdstore::cues::cues(&s, &a[2])).map(|m| println!("{m}")),
        Some("kinds") if a.len() == 3 => Store::open(&a[1]).and_then(|s| wdstore::kinds::kinds(&s, &a[2])).map(|m| println!("{m}")),
        Some("subclasses") if a.len() == 3 => Store::open(&a[1]).and_then(|s| wdstore::classes::subclasses(&s, &a[2])).map(|m| println!("{m}")),
        Some("check") if a.len() == 2 => Store::open(&a[1]).and_then(|s| s.check()).map(|m| println!("{m}")),
        _ => Err("usage: see the head of src/main.rs".into()),
    };
    if let Err(e) = r { eprintln!("{e}"); std::process::exit(1); }
}

/// Lookup latency over about 20,000 ids spread over the store, read twice: the first pass from the disk (pages not
/// yet in memory: the order's "from SSD"), the second from the page cache; the hot store and the cold set apart (they
/// may sit on different disks). Then one hot name per entity, the same way. `offset` picks another spread of ids, so a
/// rerun reads pages the last run did not bring into memory.
fn bench(s: &Store, offset: usize) -> Result<(), String> {
    let all = s.ids(1);
    let step = (all.len() / 20_000).max(1);
    let ids: Vec<_> = all.iter().skip(offset % step).step_by(step).cloned().collect();
    drop(all);
    let pct = |mut v: Vec<u128>| { if v.is_empty() { return "none".to_string(); } v.sort(); let at = |p: usize| v[(v.len() - 1) * p / 100];
        format!("p50 {} us, p95 {} us, p99 {} us (n {})", at(50), at(95), at(99), v.len()) };
    let mut labels = Vec::new();
    for pass in ["from disk", "cached"] {
        let (mut hot, mut cold_t) = (Vec::with_capacity(ids.len()), Vec::new());
        for &id in &ids {
            let t = std::time::Instant::now();
            let (cold, e) = s.locate(id)?.ok_or("an indexed id not found")?;
            if cold { cold_t.push(t.elapsed().as_micros()) } else { hot.push(t.elapsed().as_micros()) }
            if pass == "from disk" && !cold {
                if let Some(x) = e.names.iter().find(|x| x.kind != wdstore::model::NameKind::Description) { labels.push(x.text.clone()); }
            }
        }
        println!("get by id, hot, {pass}: {}", pct(hot));
        println!("get by id, cold set, {pass}: {}", pct(cold_t));
    }
    for pass in ["from disk", "cached"] {
        let mut name = Vec::with_capacity(labels.len());
        for l in &labels {
            let t = std::time::Instant::now();
            if let wdstore::names::Found::None = s.name(l)? { return Err(format!("{l}: its own label not found")); }
            name.push(t.elapsed().as_micros());
        }
        println!("name, {pass}: {}", if name.is_empty() { "no hot names".into() } else { pct(name) });
    }
    Ok(())
}
