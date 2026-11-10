//! Cubby's hot Wikidata store (CubbyLLM order 0055).
//!
//! Built from the dump's Turtle, never from a model: each entity's statements at every rank, with their values
//! exactly as written (no floats), their qualifiers (valid time hot, the rest kept apart), their references, the
//! entity's revision (recorded time), its EN/FR/mul names and its enwiki/frwiki titles; plus the value and reference
//! nodes and the redirects. `FORMAT.md` describes the files.
//!
//! - `turtle`: the dump's Turtle, token by token.
//! - `ingest`: one entity's block -> what the store keeps.
//! - `chunk`, `codec`: the chunk files, written so a rebuild gives the same bytes.
//! - `build`: inputs -> chunks + a manifest with every input's, chunk's and the code's hash.

pub mod build;
pub mod chunk;
pub mod codec;
pub mod ingest;
pub mod model;
pub mod turtle;

/// The code that built a store, by its hash: the manifest records it, so a cached build step re-runs when it changes.
pub fn code_hash() -> String {
    let mut h = blake3::Hasher::new();
    for src in [include_str!("build.rs"), include_str!("chunk.rs"), include_str!("codec.rs"), include_str!("ingest.rs"),
                include_str!("model.rs"), include_str!("turtle.rs")] {
        h.update(src.as_bytes());
    }
    h.finalize().to_hex().to_string()
}
