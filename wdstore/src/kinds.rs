//! `wdstore kinds <dir> <out dir>`: which properties each class's members hold, over every hot entity (CubbyLLM order
//! 0055, step 3: the kinds table its namesake rule from order 0037 reads, "does this kind carry this relation?").
//!
//! One pass over the hot chunks, in order. An entity's kinds are its best-rank `instance of` (P31) items; its
//! properties, those of its best-rank statements (never a deprecated one). Each (kind, property) pair an entity has
//! counts once for it; an entity with no kind counts nowhere (the rule's base is the typed items). `instance of`
//! itself is one of the properties, so (kind, P31) is the kind's size. Written to the out dir:
//!   kind_rels.tsv   Q-id of the kind, P-id, how many of its members hold it
//!   labels.tsv      Q-id or P-id, its English label (else `mul`), for every id in kind_rels.tsv that has one
//!   meta.tsv        the store's snapshot, items read, typed items, pairs, seconds
//! The reader (CubbyLLM) turns the ids into the words its facts use. Nothing is ranked or cut here: every pair counts.
use std::collections::{HashMap, HashSet};
use std::io::Write;

use crate::model::{Eid, NameKind, Value};
use crate::store::Store;

fn label(st: &Store, id: Eid) -> Result<Option<String>, String> {
    let Some(e) = st.get(id)? else { return Ok(None) };
    Ok(["en", "mul"].iter().find_map(|l| e.names.iter().find(|n| n.kind == NameKind::Label && n.lang == *l).map(|n| n.text.clone())))
}

pub fn kinds(st: &Store, out: &str) -> Result<String, String> {
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut pairs: HashMap<(u64, u32), u32> = HashMap::new();
    let (mut items, mut typed) = (0u64, 0u64);
    let n = st.hot_chunks();
    for k in 0..n {
        for e in st.hot_chunk(k)? {
            items += 1;
            let (mut ks, mut ps) = (Vec::new(), Vec::new());
            for s in &e.statements {
                if !s.best || s.rank as u8 == 0 { continue; }
                ps.push(s.pid);
                if s.pid == 31 { if let Value::Entity(c) = &s.value { ks.push(c.0); } }
            }
            if ks.is_empty() { continue; }
            typed += 1;
            ks.sort_unstable(); ks.dedup(); ps.sort_unstable(); ps.dedup();
            for &c in &ks { for &p in &ps { *pairs.entry((c, p)).or_insert(0) += 1; } }
        }
        if k % 50 == 0 || k + 1 == n {
            eprintln!("  kinds: chunk {}/{n}, {items} items, {typed} typed, {} pairs, {} s", k + 1, pairs.len(), t0.elapsed().as_secs());
        }
    }
    let mut rows: Vec<_> = pairs.into_iter().collect();
    rows.sort_unstable();
    let mut f = std::io::BufWriter::new(std::fs::File::create(format!("{out}/kind_rels.tsv")).map_err(|e| e.to_string())?);
    let (mut kinds, mut props) = (HashSet::new(), HashSet::new());
    for ((c, p), m) in &rows {
        writeln!(f, "{}\tP{p}\t{m}", Eid(*c).text()).map_err(|e| e.to_string())?;
        kinds.insert(*c);
        props.insert(*p);
    }
    f.flush().map_err(|e| e.to_string())?;
    let mut ids: Vec<Eid> = kinds.into_iter().map(Eid).collect();
    ids.extend(props.into_iter().filter_map(|p| Eid::parse(&format!("P{p}"))));
    ids.sort_unstable();
    let mut f = std::io::BufWriter::new(std::fs::File::create(format!("{out}/labels.tsv")).map_err(|e| e.to_string())?);
    let mut named = 0usize;
    for id in &ids {
        if let Some(l) = label(st, *id)? {
            writeln!(f, "{}\t{}", id.text(), l.replace(['\t', '\n', '\r'], " ")).map_err(|e| e.to_string())?;
            named += 1;
        }
    }
    f.flush().map_err(|e| e.to_string())?;
    let snap = crate::snapshot::at(&st.dir, None)?.hash;
    let secs = t0.elapsed().as_secs();
    std::fs::write(format!("{out}/meta.tsv"), format!("snapshot\t{snap}\nitems\t{items}\ntyped\t{typed}\npairs\t{}\nlabels\t{named}\nseconds\t{secs}\n", rows.len()))
        .map_err(|e| e.to_string())?;
    Ok(format!("{items} hot items, {typed} typed; {} (kind, property) pairs; {named} of {} ids labelled; {secs} s", rows.len(), ids.len()))
}
