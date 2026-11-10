//! Cubby's hot Wikidata store (CubbyLLM order 0055).
//!
//! Built from the dump's Turtle, never from a model: each entity's statements at every rank, with their values
//! exactly as written (no floats), their qualifiers (valid time hot, the rest kept apart), their references, the
//! entity's revision (recorded time), its EN/FR/mul names and its enwiki/frwiki titles; plus the value and reference
//! nodes and the redirects. Bulk classes (scholarly articles by default) go to a cold set of the same format.
//! `FORMAT.md` describes the files.
//!
//! - `bz2par`: the dump's bzip2 decoded on every core, block by block, in order.
//! - `turtle`: the dump's Turtle, token by token.
//! - `ingest`: one entity's block -> what the store keeps.
//! - `chunk`, `nodes`, `codec`: the entity chunks and the value and reference node files, written so a rebuild
//!   gives the same bytes.
//! - `reverse`: (property, value entity) -> the entities that have it.
//! - `names`: labels and aliases -> entities, exact, accent-folded, a bounded typo path; never by fame.
//! - `spill`: rows sorted in runs on a scratch disk and merged, for the indexes.
//! - `writer`, `resume`: the store written append-only, checkpointed, resumed after a stop.
//! - `build`: the dump's text in its order -> the store and a manifest with every input's, file's and the code's hash.
//! - `store`: a built store opened: one entity by id, redirects followed, every file checked.

pub mod build;
pub mod bz2par;
pub mod chunk;
pub mod codec;
pub mod export;
pub mod ingest;
pub mod model;
pub mod names;
pub mod nodes;
pub mod resume;
pub mod reverse;
pub mod spill;
pub mod store;
pub mod turtle;
pub mod writer;

/// The code that built a store, by its hash: the manifest records it, so a cached build step re-runs when it changes.
pub fn code_hash() -> String {
    let mut h = blake3::Hasher::new();
    for src in [include_str!("build.rs"), include_str!("bz2par.rs"), include_str!("chunk.rs"), include_str!("codec.rs"),
                include_str!("ingest.rs"), include_str!("model.rs"), include_str!("names.rs"), include_str!("nodes.rs"),
                include_str!("resume.rs"), include_str!("reverse.rs"), include_str!("spill.rs"), include_str!("store.rs"),
                include_str!("turtle.rs"), include_str!("writer.rs")] {
        h.update(src.as_bytes());
    }
    h.finalize().to_hex().to_string()
}
