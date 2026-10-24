//! `cubelang knowledge` and `cubelang history` (2026-10-24): build, inspect and search the layered
//! store's segments (`cubelang::vm::{knowledge, history, segment}`).

use std::path::Path;
use std::process;

use cubelang::vm::history::{History, SearchMode};
use cubelang::vm::knowledge::{KnowledgeStore, keys_with_prefix};

/// A knowledge file as a store: a `.cubeseg` is opened as its frozen segment (no parse); anything
/// else is read as facts `.jsonl`, parsed once, and frozen into one segment, so every fork of the
/// result shares it.
pub fn open_knowledge(path: &str) -> Result<KnowledgeStore, String> {
    if path.ends_with(".cubeseg") {
        // (the segment reader names the file in its errors)
        return KnowledgeStore::open(Path::new(path));
    }
    let data = std::fs::read_to_string(path).map_err(|e| format!("{}: cannot read: {}", path, e))?;
    let mut store = KnowledgeStore::new();
    store.load_jsonl(&data).map_err(|e| format!("{}: {}", path, e))?;
    store.compact().map_err(|e| format!("{}: {}", path, e))?;
    Ok(store)
}

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {}", msg);
    process::exit(1)
}

pub fn cmd_knowledge(args: &[String]) {
    let open_knowledge = |p: &str| open_knowledge(p).map_err(|e| format!("knowledge {}", e));
    let usage = "usage: cubelang knowledge build FACTS.jsonl -o OUT.cubeseg | info FILE | lookup FILE KEY | keys FILE PREFIX [--limit N]";
    match args.first().map(String::as_str) {
        Some("build") => {
            let (Some(input), Some(out)) = (args.get(1), flag(args, "-o")) else { die(usage) };
            let started = std::time::Instant::now();
            let data = std::fs::read_to_string(input).unwrap_or_else(|e| die(format!("{}: {}", input, e)));
            let mut store = KnowledgeStore::new();
            let (facts, aliases) = store.load_jsonl(&data).unwrap_or_else(|e| die(e));
            let seg = store.to_segment().unwrap_or_else(|e| die(e));
            seg.write(Path::new(&out)).unwrap_or_else(|e| die(format!("{}: {}", out, e)));
            println!("{} facts and {} aliases -> {} keys, {} bytes in {} ({} ms; the jsonl was {} bytes)",
                     facts, aliases, seg.len(), seg.size_bytes(), out,
                     started.elapsed().as_millis(), data.len());
        }
        Some("info") => {
            let Some(path) = args.get(1) else { die(usage) };
            let store = open_knowledge(path).unwrap_or_else(|e| die(e));
            println!("{}: {} keys, {} segment(s), {} bytes", path, store.len(), store.segment_count(), store.segment_bytes());
        }
        Some("lookup") => {
            let (Some(path), Some(key)) = (args.get(1), args.get(2)) else { die(usage) };
            let store = open_knowledge(path).unwrap_or_else(|e| die(e));
            for f in store.lookup(key) {
                println!("{}", serde_json::json!({ "text": f.text, "source": f.source }));
            }
        }
        Some("keys") => {
            let (Some(path), Some(prefix)) = (args.get(1), args.get(2)) else { die(usage) };
            let limit = flag(args, "--limit").map(|n| n.parse().unwrap_or_else(|_| die("--limit takes a number"))).unwrap_or(50);
            let store = open_knowledge(path).unwrap_or_else(|e| die(e));
            for k in keys_with_prefix(&store, prefix, limit) {
                println!("{}", k);
            }
        }
        _ => die(usage),
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

/// The arguments that are not flags (or flag values).
fn words(args: &[String], with_values: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if with_values.contains(&a.as_str()) {
            i += 2;
            continue;
        }
        if !a.starts_with("--") {
            out.push(a.clone());
        }
        i += 1;
    }
    out
}

pub fn cmd_history(args: &[String]) {
    let usage = "usage: cubelang history append DIR [--speaker S] [--at MS] TEXT... | search DIR WORDS... \
                 [--prefix|--fuzzy] [--limit N] | recent DIR [N] | forget DIR ID | merge DIR | gc DIR | info DIR";
    let (Some(cmd), Some(dir)) = (args.first(), args.get(1)) else { die(usage) };
    let dir = Path::new(dir);
    let rest = &args[2..];
    let open = || History::open_dir(dir).unwrap_or_else(|e| die(e));
    match cmd.as_str() {
        "append" => {
            let speaker = flag(rest, "--speaker").unwrap_or_else(|| "user".to_string());
            let at = match flag(rest, "--at") {
                Some(t) => t.parse().unwrap_or_else(|_| die("--at takes Unix milliseconds")),
                None => cubelang::auth::now_ms(),
            };
            let text = words(rest, &["--speaker", "--at"]).join(" ");
            if text.trim().is_empty() {
                die(usage);
            }
            let mut h = open();
            let id = h.append(&speaker, &text, at);
            // freeze on the turn, keep the segment count bounded, and persist
            h.freeze().unwrap_or_else(|e| die(e));
            h.maybe_merge(32).unwrap_or_else(|e| die(e));
            h.save_dir(dir).unwrap_or_else(|e| die(e));
            println!("{}", id);
        }
        "search" => {
            let mode = if rest.iter().any(|a| a == "--fuzzy") {
                SearchMode::Fuzzy
            } else if rest.iter().any(|a| a == "--prefix") {
                SearchMode::Prefix
            } else {
                SearchMode::Exact
            };
            let limit = flag(rest, "--limit").map(|n| n.parse().unwrap_or_else(|_| die("--limit takes a number"))).unwrap_or(20);
            let query = words(rest, &["--limit"]).join(" ");
            let h = open();
            let started = std::time::Instant::now();
            let hits = h.search(&query, mode, limit);
            let us = started.elapsed().as_micros();
            for hit in &hits {
                println!("{}", serde_json::json!({
                    "turn": hit.turn.id, "at": hit.turn.at_ms, "speaker": hit.turn.speaker,
                    "text": hit.turn.text, "matched": hit.matched,
                }));
            }
            eprintln!("{} hit(s) in {} us", hits.len(), us);
        }
        "recent" => {
            let n = rest.first().map(|n| n.parse().unwrap_or_else(|_| die("recent takes a number"))).unwrap_or(10);
            for t in open().recent(n) {
                println!("{}", serde_json::json!({ "turn": t.id, "at": t.at_ms, "speaker": t.speaker, "text": t.text }));
            }
        }
        "forget" => {
            let id: u64 = rest.first().and_then(|n| n.parse().ok()).unwrap_or_else(|| die(usage));
            let mut h = open();
            if !h.forget(id) {
                die(format!("no live turn {}", id));
            }
            h.save_dir(dir).unwrap_or_else(|e| die(e));
            println!("turn {} forgotten (run `history merge` then `history gc` to take it out of the files)", id);
        }
        "merge" => {
            let mut h = open();
            h.merge_all().unwrap_or_else(|e| die(e));
            h.save_dir(dir).unwrap_or_else(|e| die(e));
            println!("{} turns in {} segment(s)", h.len(), h.segment_count());
        }
        "gc" => {
            for name in History::gc_dir(dir).unwrap_or_else(|e| die(e)) {
                println!("removed {}", name);
            }
        }
        "info" => {
            let h = open();
            println!("{}: {} turns, {} segment(s), {} bytes", dir.display(), h.len(), h.segment_count(), h.segment_bytes());
        }
        _ => die(usage),
    }
}
