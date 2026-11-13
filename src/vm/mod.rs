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
//! - `registry`  ? the `use`/`override` capability/module registry (Task 3):
//!                 module name -> {fn name -> (native impl, overridable)}.
//! - `interfaces`? the standard-interface registry (Task 4): interface name
//!                 -> required {fn name, arity}. Contracts, not callable
//!                 impls — a separate type from `registry`, not a mode of it.

pub mod hypervec;
pub mod codebook;
pub mod index;
pub mod memory;
pub mod segment;
pub mod knowledge;
pub mod lev;
pub mod typo_index;
pub mod history;
pub mod engine;
pub mod key;
pub mod registry;
pub mod interfaces;
pub mod governor;

// Re-export the engine's public surface so existing `use cubelang::vm::{VM, ...}`
// call sites keep working after the flat vm.rs -> vm/ split.
pub use engine::{VM, ExecResult, Suspension, Value, VmSnapshot, VsaFrame, RecoverReason, RecoverRecord};
pub use knowledge::{Fact, KnowledgeStore};
pub use history::{History, SearchMode, Turn};
pub use registry::{ModuleFn, ModuleRegistry, NativeFn, REGISTRY_VERSION};
pub use interfaces::{InterfaceFn, InterfaceRegistry};

// Re-export the VSA memory primitives for use by the engine and externally.
pub use hypervec::{Hypervec, DEFAULT_DIM};
pub use codebook::Codebook;
pub use index::{HammingIndex, QueryResult};
