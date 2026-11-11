//! `wdstore subclasses <dir> <out dir>`: every best-rank "subclass of" (P279) edge of the hot entities (CubbyLLM order
//! 0066: the class-roots table its four answer checks read -- is this class under "geographic location", "human",
//! "organization", "work", a position, a title or a rank?).
//!
//! One pass over the hot chunks, in order, as `kinds` and `cues` make. Written to the out dir:
//!   subclass.tsv   Q-id of the class, Q-id of one superclass (best rank, never deprecated), one edge a line
//!   meta.tsv       the store's snapshot, entities read, classes with an edge, edges, seconds
//! The reader (CubbyLLM) builds the closure under each root it names; nothing is cut or ranked here.
use std::io::Write;

use crate::model::{Eid, Value};
use crate::store::Store;

pub fn subclasses(st: &Store, out: &str) -> Result<String, String> {
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut f = std::io::BufWriter::new(std::fs::File::create(format!("{out}/subclass.tsv")).map_err(|e| e.to_string())?);
    let (mut items, mut classes, mut edges) = (0u64, 0u64, 0u64);
    let n = st.hot_chunks();
    for k in 0..n {
        for e in st.hot_chunk(k)? {
            items += 1;
            let Some(id) = e.id else { continue };
            let mut any = false;
            for s in &e.statements {
                if s.pid == 279 && s.best && s.rank as u8 != 0 {
                    if let Value::Entity(c) = &s.value {
                        writeln!(f, "{}\t{}", id.text(), Eid(c.0).text()).map_err(|e| e.to_string())?;
                        edges += 1;
                        any = true;
                    }
                }
            }
            if any { classes += 1; }
        }
        if k % 100 == 0 || k + 1 == n {
            eprintln!("  subclasses: chunk {}/{n}, {items} entities, {edges} edges, {} s", k + 1, t0.elapsed().as_secs());
        }
    }
    f.flush().map_err(|e| e.to_string())?;
    let snap = crate::snapshot::at(&st.dir, None)?.hash;
    let secs = t0.elapsed().as_secs();
    std::fs::write(format!("{out}/meta.tsv"), format!("snapshot\t{snap}\nitems\t{items}\nclasses\t{classes}\nedges\t{edges}\nseconds\t{secs}\n"))
        .map_err(|e| e.to_string())?;
    Ok(format!("{items} hot entities; {classes} classes with a superclass; {edges} edges; {secs} s"))
}
