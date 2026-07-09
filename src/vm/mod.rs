//! CubeLang VM ? executes .cubebin bytecode with value-level compute and
//! VSA-level hippocampal memory.
//!
//! Module layout:
//! - `hypervec`  ? MAP-Bipolar hypervector algebra (bind/bundle/permute/cosine).
//!                 Ported from opcode-vsa-rs (the Guaranteed-correctness core).
//! - `codebook`  ? deterministic FNV-1a seeded symbol -> hypervector mapping.
//! - `index`     ? HammingIndex / LshIndex nearest-neighbour cleanup memory.
//! - `engine`    ? the bytecode interpreter (registers, stack, control flow,
//!                 and the opcode dispatch loop).

pub mod hypervec;
pub mod codebook;
pub mod index;
pub mod memory;
pub mod knowledge;
pub mod engine;

// Re-export the engine's public surface so existing `use cubelang::vm::{VM, ...}`
// call sites keep working after the flat vm.rs -> vm/ split.
pub use engine::{VM, ExecResult, Suspension, Value};
pub use knowledge::{Fact, KnowledgeStore};

// Re-export the VSA memory primitives for use by the engine and externally.
pub use hypervec::{Hypervec, DEFAULT_DIM};
pub use codebook::Codebook;
pub use index::{HammingIndex, QueryResult};
