//! `cubelang knowledge` and `cubelang history` (2026-10-24): build, inspect and search the layered
//! store's segments (`cubelang::vm::{knowledge, history, segment}`).

use std::path::Path;
use std::process;

use cubelang::vm::history::{History, SearchMode};
use cubelang::vm::segment::Vault;
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

/// Every value of a flag that may repeat (`--tag a --tag b`).
fn flags(args: &[String], name: &str) -> Vec<String> {
    args.iter().enumerate().filter(|(_, a)| *a == name).filter_map(|(i, _)| args.get(i + 1).cloned()).collect()
}

/// A turn as one JSON line: tags and meta only when it has them.
fn turn_json(t: &cubelang::vm::history::Turn, matched: Option<&[String]>) -> serde_json::Value {
    let mut v = serde_json::json!({ "turn": t.id, "at": t.at_ms, "speaker": t.speaker, "text": t.text });
    if let Some(m) = matched {
        v["matched"] = serde_json::json!(m);
    }
    if !t.tags.is_empty() {
        v["tags"] = serde_json::json!(t.tags);
    }
    if !t.meta.is_empty() {
        v["meta"] = serde_json::json!(t.meta);
    }
    v
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

/// The flags every `history` command takes for a sealed directory.
const KEY_FLAGS: [&str; 2] = ["--key", "--passphrase-file"];

pub fn cmd_history(args: &[String]) {
    let usage = "usage: cubelang history append DIR [--speaker S] [--at MS] [--tag T]... [--meta JSON] TEXT... \
                 | search DIR [WORDS...] [--tag T]... [--prefix|--fuzzy] [--limit N] | recent DIR [N] | tags DIR [PREFIX] \
                 | forget DIR ID | merge DIR | gc DIR | info DIR | seal DIR --key KEY.json\n\
                 a sealed directory takes --key KEY.json on every command (sealed key files: --passphrase-file P \
                 or CUBELANG_KEY_PASSPHRASE)";
    let (Some(cmd), Some(dir)) = (args.first(), args.get(1)) else { die(usage) };
    let dir = Path::new(dir);
    let rest = &args[2..];
    // --key: the directory is sealed to (and signed by) this identity
    let vault = flag(rest, "--key").map(|k| {
        cubelang::seal::SealedVault::own(crate::open_identity(&k, flag(rest, "--passphrase-file").as_deref()))
    });
    let v: Option<&dyn Vault> = vault.as_ref().map(|x| x as &dyn Vault);
    let open = || History::open_dir_in(dir, v).unwrap_or_else(|e| die(e));
    // the commands that write hold the directory's lock: one writer at a time
    let _lock = matches!(cmd.as_str(), "seal" | "append" | "forget" | "merge" | "gc")
        .then(|| History::lock_dir(dir).unwrap_or_else(|e| die(e)));
    let save = |h: &mut History| h.save_dir_in(dir, v).unwrap_or_else(|e| die(e));
    match cmd.as_str() {
        "seal" => {
            let Some(v) = v else { die("history seal needs --key KEY.json (the identity to seal the history to)") };
            let removed = History::seal_dir(dir, v).unwrap_or_else(|e| die(e));
            let h = History::open_dir_in(dir, Some(v)).unwrap_or_else(|e| die(e));
            println!("{}: sealed to {} ({} turns, {} segment(s)); removed {} plain file(s). Copies made before \
                      (backups, sync folders) are not reached: remove them yourself.",
                     dir.display(), vault.as_ref().map(|x| x.name()).unwrap_or(""), h.len(), h.segment_count(), removed.len());
        }
        "append" => {
            let speaker = flag(rest, "--speaker").unwrap_or_else(|| "user".to_string());
            let at = match flag(rest, "--at") {
                Some(t) => t.parse().unwrap_or_else(|_| die("--at takes Unix milliseconds")),
                None => cubelang::auth::now_ms(),
            };
            let text = words(rest, &["--speaker", "--at", "--tag", "--meta", KEY_FLAGS[0], KEY_FLAGS[1]]).join(" ");
            if text.trim().is_empty() {
                die(usage);
            }
            let meta = flag(rest, "--meta").unwrap_or_default();
            let mut h = open();
            let id = h.append_tagged(&speaker, &text, at, &flags(rest, "--tag"), &meta).unwrap_or_else(|e| die(e));
            // freeze on the turn, keep the segment count bounded, and persist
            h.freeze().unwrap_or_else(|e| die(e));
            h.maybe_merge(32).unwrap_or_else(|e| die(e));
            save(&mut h);
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
            let query = words(rest, &["--limit", "--tag", KEY_FLAGS[0], KEY_FLAGS[1]]).join(" ");
            let tags = flags(rest, "--tag");
            if query.trim().is_empty() && tags.is_empty() {
                die(usage);
            }
            let h = open();
            let started = std::time::Instant::now();
            let hits = h.search_in(&query, mode, &tags, limit);
            let us = started.elapsed().as_micros();
            for hit in &hits {
                println!("{}", turn_json(&hit.turn, Some(&hit.matched)));
            }
            eprintln!("{} hit(s) in {} us", hits.len(), us);
        }
        "recent" => {
            let n = words(rest, &KEY_FLAGS).first().map(|n| n.parse().unwrap_or_else(|_| die("recent takes a number"))).unwrap_or(10);
            for t in open().recent(n) {
                println!("{}", turn_json(&t, None));
            }
        }
        "tags" => {
            let prefix = words(rest, &KEY_FLAGS).first().cloned().unwrap_or_default();
            for (tag, n) in open().tags(&prefix) {
                println!("{}", serde_json::json!({ "tag": tag, "turns": n }));
            }
        }
        "forget" => {
            let id: u64 = words(rest, &KEY_FLAGS).first().and_then(|n| n.parse().ok()).unwrap_or_else(|| die(usage));
            let mut h = open();
            if !h.forget(id) {
                die(format!("no live turn {}", id));
            }
            save(&mut h);
            println!("turn {} forgotten (run `history merge` then `history gc` to take it out of the files)", id);
        }
        "merge" => {
            let mut h = open();
            h.merge_all().unwrap_or_else(|e| die(e));
            save(&mut h);
            println!("{} turns in {} segment(s)", h.len(), h.segment_count());
        }
        "gc" => {
            for name in History::gc_dir_in(dir, v).unwrap_or_else(|e| die(e)) {
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
