//! The layered store, measured (2026-10-24): what a request costs before and after.
//!
//!     cargo run --release --example store_bench -- [facts] [turns]
//!
//! Defaults: 1,000,000 facts (with a quarter as many aliases) and 100,000 history turns, all
//! synthetic but shaped like the real thing (multi-word titles, sentence-length facts with a
//! source, turns drawn from a Zipf-distributed vocabulary). "Before" is the store as it was: one
//! hash map, copied whole into every request and every branch, and a history with no index (a
//! search reads every turn).

use std::collections::HashMap;
use std::time::Instant;

use cubelang::vm::history::{History, SearchMode};
use cubelang::vm::knowledge::{Fact, KnowledgeStore, normalize};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

fn word(rng: &mut SmallRng) -> String {
    const SYL: [&str; 24] = ["ba", "ka", "lo", "mi", "nu", "ra", "te", "vo", "zen", "dor", "fil", "gar",
                             "hel", "jus", "kor", "lin", "mar", "nor", "pel", "quin", "ros", "sal", "tor", "wen"];
    let n = rng.gen_range(2..=4);
    (0..n).map(|_| SYL[rng.gen_range(0..SYL.len())]).collect()
}

fn title(rng: &mut SmallRng) -> String {
    let n = rng.gen_range(1..=3);
    (0..n).map(|_| word(rng)).collect::<Vec<_>>().join(" ")
}

/// A rough heap size of the old store: the map's slots plus every string's bytes.
fn old_bytes(m: &HashMap<String, Vec<Fact>>) -> usize {
    let slot = std::mem::size_of::<String>() + std::mem::size_of::<Vec<Fact>>() + 1;
    let mut b = m.capacity() * slot;
    for (k, v) in m {
        b += k.capacity() + v.capacity() * std::mem::size_of::<Fact>();
        for f in v {
            b += f.text.capacity() + f.source.capacity();
        }
    }
    b
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn mib(b: usize) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n_facts: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
    let n_turns: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let mut rng = SmallRng::seed_from_u64(7);

    // ── Knowledge ───────────────────────────────────────────────────────
    let mut titles = Vec::with_capacity(n_facts);
    let mut old: HashMap<String, Vec<Fact>> = HashMap::new();
    let mut store = KnowledgeStore::new();
    let t = Instant::now();
    for i in 0..n_facts {
        let key = format!("{} {}", title(&mut rng), i % 1000);
        let text = format!("{} is a {} of {} first recorded in {}", key, word(&mut rng), title(&mut rng), 1000 + i % 1000);
        let source = format!("dbpedia:{}", key.replace(' ', "_"));
        old.entry(normalize(&key)).or_default().push(Fact { text: text.clone(), source: source.clone() });
        store.insert(&key, text, source);
        titles.push(key);
    }
    let n_alias = n_facts / 4;
    for i in 0..n_alias {
        let alias = format!("{} redirect {}", title(&mut rng), i);
        let target = &titles[rng.gen_range(0..titles.len())];
        let facts = old.get(&normalize(target)).cloned().unwrap();
        old.insert(normalize(&alias), facts);
        store.alias(&alias, target);
    }
    let load_ms = ms(t);
    let t = Instant::now();
    store.compact().unwrap();
    let freeze_ms = ms(t);
    let seg_bytes = store.segment_bytes();
    let old_b = old_bytes(&old);

    // a request's copy, and a branch (copy + exclude one key + assume one fact)
    let reps = 5;
    let t = Instant::now();
    for _ in 0..reps {
        std::hint::black_box(old.clone());
    }
    let old_fork_ms = ms(t) / reps as f64;
    let t = Instant::now();
    let forks = 10_000;
    for _ in 0..forks {
        std::hint::black_box(store.clone());
    }
    let new_fork_us = ms(t) * 1e3 / forks as f64;

    let probe = normalize(&titles[12345 % titles.len()]);
    let t = Instant::now();
    {
        let mut b = old.clone();
        // the old remove: the key, then every key carrying the same facts (a full scan)
        let gone = b.remove(&probe).unwrap();
        let same: Vec<String> = b.iter().filter(|(_, v)| **v == gone).map(|(k, _)| k.clone()).collect();
        for k in same { b.remove(&k); }
        b.entry("assumed".into()).or_default().push(Fact { text: "x".into(), source: "branch".into() });
        std::hint::black_box(b);
    }
    let old_branch_ms = ms(t);
    let t = Instant::now();
    let branches = 1000;
    for i in 0..branches {
        let mut b = store.clone();
        b.remove(&titles[(12345 + i) % titles.len()]);
        b.insert("assumed", "x", "branch");
        std::hint::black_box(b);
    }
    let new_branch_us = ms(t) * 1e3 / branches as f64;

    let lookups = 200_000;
    let keys: Vec<String> = (0..lookups).map(|i| titles[(i * 7919) % titles.len()].clone()).collect();
    let t = Instant::now();
    let mut hits = 0usize;
    for k in &keys {
        hits += old.get(&normalize(k)).map_or(0, |v| v.clone().len());
    }
    let old_lookup_ns = ms(t) * 1e6 / lookups as f64;
    let t = Instant::now();
    let mut hits2 = 0usize;
    for k in &keys {
        hits2 += store.lookup(k).len();
    }
    let new_lookup_ns = ms(t) * 1e6 / lookups as f64;
    assert_eq!(hits, hits2, "the layered store answers as the map did");

    println!("## Knowledge store: {} facts, {} aliases ({} keys)\n", n_facts, n_alias, old.len());
    println!("| | before (one map, copied per request) | after (shared segment + top layer) |");
    println!("|---|---|---|");
    println!("| memory | ≈{:.0} MiB per copy | {:.0} MiB once per process |", mib(old_b), mib(seg_bytes));
    println!("| a request's copy (fork) | {:.0} ms | {:.2} µs |", old_fork_ms, new_fork_us);
    println!("| a branch (fork, exclude 1 key, assume 1 fact) | {:.0} ms | {:.1} µs |", old_branch_ms, new_branch_us);
    println!("| exact lookup | {:.0} ns | {:.0} ns |", old_lookup_ns, new_lookup_ns);
    println!("| build | load {:.0} ms | + freeze {:.0} ms (once, or offline: `cubelang knowledge build`) |\n", load_ms, freeze_ms);

    // ── History ─────────────────────────────────────────────────────────
    let vocab: Vec<String> = (0..5000).map(|_| word(&mut rng)).collect();
    // Zipf-ish: word i drawn with weight 1/(i+1)
    let weights: Vec<f64> = (0..vocab.len()).map(|i| 1.0 / (i as f64 + 1.0)).collect();
    let total: f64 = weights.iter().sum();
    let cdf: Vec<f64> = weights.iter().scan(0.0, |acc, w| { *acc += w / total; Some(*acc) }).collect();
    let pick = |rng: &mut SmallRng| -> usize {
        let x: f64 = rng.gen();
        cdf.partition_point(|c| *c < x).min(vocab.len() - 1)
    };
    let mut texts = Vec::with_capacity(n_turns);
    for _ in 0..n_turns {
        let n = rng.gen_range(6..=24);
        let t: Vec<&str> = (0..n).map(|_| vocab[pick(&mut rng)].as_str()).collect();
        texts.push(t.join(" "));
    }
    let mut h = History::new();
    let t = Instant::now();
    for (i, text) in texts.iter().enumerate() {
        h.append(if i % 2 == 0 { "user" } else { "cubby" }, text, i as u64);
        h.freeze().unwrap();
        h.maybe_merge(32).unwrap();
    }
    let per_turn_us = ms(t) * 1e3 / n_turns as f64;
    let raw: usize = texts.iter().map(|t| t.len()).sum();

    let rare = vocab[4000].clone();
    let common = vocab[3].clone();
    let pair = format!("{} {}", vocab[10], vocab[200]);
    let prefix = vocab[1500][..3.min(vocab[1500].len())].to_string();
    let typo = {
        let mut w = vocab[900].clone();
        w.push('x');
        w
    };
    let scan = |q: &str, f: &dyn Fn(&str, &str) -> bool| -> (usize, f64) {
        let words = cubelang::vm::history::terms_of(q);
        let t = Instant::now();
        let n = texts.iter().filter(|text| {
            let ts = cubelang::vm::history::terms_of(text);
            words.iter().all(|w| ts.iter().any(|x| f(w, x)))
        }).count();
        (n, ms(t))
    };
    let timed = |q: &str, m: SearchMode, limit: usize| -> (usize, f64) {
        let t = Instant::now();
        let n = h.search(q, m, limit).len();
        (n, ms(t))
    };
    println!("## History: {} turns ({:.0} MiB of text) in {} segments ({:.0} MiB)\n",
             n_turns, mib(raw), h.segment_count(), mib(h.segment_bytes()));
    println!("Each turn appended, frozen and merged as it came: {:.0} µs a turn.\n", per_turn_us);
    println!("| search | hits | before (read every turn) | after (term index), every hit | after, the newest 20 |");
    println!("|---|---|---|---|---|");
    for (label, q, mode) in [("a rare word", rare.as_str(), SearchMode::Exact),
                             ("a common word", common.as_str(), SearchMode::Exact),
                             ("two words", pair.as_str(), SearchMode::Exact),
                             ("a prefix", prefix.as_str(), SearchMode::Prefix),
                             ("a typo", typo.as_str(), SearchMode::Fuzzy)] {
        let f: Box<dyn Fn(&str, &str) -> bool> = match mode {
            SearchMode::Exact => Box::new(|a: &str, b: &str| a == b),
            SearchMode::Prefix => Box::new(|a: &str, b: &str| b.starts_with(a)),
            SearchMode::Fuzzy => Box::new(|a: &str, b: &str| strsim(a, b) <= if a.chars().count() > 7 { 2 } else if a.chars().count() > 3 { 1 } else { 0 }),
        };
        let (n0, t0) = scan(q, f.as_ref());
        let (n1, t1) = timed(q, mode, usize::MAX);
        let (_, t2) = timed(q, mode, 20);
        assert_eq!(n0, n1, "{label}: the index finds what the scan finds");
        println!("| {} (`{}`) | {} | {:.1} ms | {:.2} ms | {:.2} ms |", label, q, n1, t0, t1, t2);
    }
}

fn strsim(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}
