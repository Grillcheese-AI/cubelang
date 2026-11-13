//! CubeLang bytecode compiler — AST → CubeMind VM bytecodes.
//!
//! Walks the AST and emits binary bytecodes (0x00-0xFF) for each instruction.
//! The output is a `CompiledProgram` containing the bytecode blob, metadata,
//! and a symbol table for debugging.

use std::collections::HashMap;

use crate::ast::*;
use crate::parser::ParseError;
use crate::token::Span;
// The registry is a static, deterministic capability table (see its module
// doc), not live VM execution state -- the compiler needs read-only access
// to it purely to validate `override` declarations at compile time.
use crate::vm::registry::ModuleRegistry;
// Task 4: same "static, deterministic table" reasoning as ModuleRegistry
// above, but for standard interfaces -- a separate registry (contracts, not
// callable impls), consulted to validate `implements` declarations.
use crate::vm::interfaces::InterfaceRegistry;

// ── Opcode constants (match opcode-vsa-rs ir.rs) ────────────────────────────

/// The most operands one instruction carries: its count is one byte (P6-8).
pub const MAX_OPERANDS: usize = 255;

pub mod op {
    pub const CREATE: u8    = 0x00;
    pub const DESTROY: u8   = 0x01;
    pub const ASSIGN: u8    = 0x02;
    pub const ADD: u8       = 0x03;
    pub const SUB: u8       = 0x04;
    pub const MUL: u8       = 0x05;
    pub const DIV: u8       = 0x06;
    pub const TRANSFER: u8  = 0x07;
    pub const COPY: u8      = 0x08;
    pub const PUSH: u8      = 0x09;
    pub const POP: u8       = 0x0A;
    pub const COMPARE: u8   = 0x0B;
    pub const QUERY: u8     = 0x0C;
    pub const STORE: u8     = 0x0D;
    pub const RECALL: u8    = 0x0E;
    pub const BIND_ROLE: u8 = 0x0F;
    pub const UNBIND: u8    = 0x10;
    pub const COND: u8      = 0x11;
    pub const LOOP: u8      = 0x12;
    pub const CALL: u8      = 0x13;
    pub const JMP: u8       = 0x14;
    pub const LABEL: u8     = 0x15;
    pub const REMEMBER: u8  = 0x21;
    pub const SUM: u8       = 0x34;
    pub const UNIFY: u8     = 0x35;
    pub const INST: u8      = 0x36;
    pub const GEN: u8       = 0x37;
    pub const NEWVAR: u8    = 0x38;
    /// Early-return opcode (Plan-1). 0 operands: `RETURN` yields the current
    /// accumulator. 1 operand: `RETURN <v>` resolves v (Imm/Float/Named) and
    /// sets the accumulator before exiting. Lifts `Stmt::Return` out of
    /// the SKIP-fallback that lets execution silently run past it.
    pub const RETURN: u8    = 0x39;
    /// Plan-3 array opcodes. MAKE_ARRAY builds a Value::Array in `dst` from N
    /// element operands; LEN/INDEX inspect/extract from it. Pre-requisites for
    /// real for-each lowering (Plan-4).
    pub const MAKE_ARRAY: u8 = 0x3A;
    pub const LEN: u8        = 0x3B;
    pub const INDEX: u8      = 0x3C;
    /// DRIFT.md §C4 fix (2026-09-14): `container[key] = value`.
    /// Operands: [container, key, value]. Works on both `Value::Array`
    /// (integer key) and `Value::Map` (string key). Before this existed,
    /// an index assignment target compiled to ZERO bytecode, silently --
    /// the single most common violation across the complex examples.
    ///
    /// Not yet present in the sibling `opcode-vsa-rs` / Python VM tables;
    /// `tests/opcode_sync.rs` only enforces bytes for names present in
    /// BOTH, so it is listed there and will be enforced once ported.
    pub const SET_INDEX: u8  = 0x3D;
    pub const SKIP: u8       = 0xFF;

    // Extended reasoning opcodes (canonical values from opcode-vsa-rs ir.rs).
    pub const SEQ: u8            = 0x16;
    pub const DIFF: u8           = 0x18;
    pub const DETECT_PATTERN: u8 = 0x19;
    pub const PREDICT: u8        = 0x1A;
    pub const MATCH: u8          = 0x1B;
    pub const DEBATE: u8         = 0x1C;
    pub const DISCOVER: u8       = 0x1E;
    pub const DECODE: u8         = 0x23;
    pub const SCORE: u8          = 0x24;
    pub const SPECIALIZE: u8     = 0x25;
    pub const REWARD: u8         = 0x27;
    pub const INFER: u8          = 0x2A;
    pub const MERGE: u8          = 0x2D;
    pub const SPLIT: u8          = 0x2E;
    pub const FILTER: u8         = 0x2F;
    pub const MAP_ROLES: u8      = 0x30;
    pub const REDUCE: u8         = 0x31;
    pub const TEMPORAL_BIND: u8  = 0x32;
    pub const ANALOGY: u8     = 0x33;
    pub const BROADCAST: u8   = 0x2B;
    pub const EXPLORE: u8     = 0x26;
    pub const FORGE: u8       = 0x28;
    pub const ASK: u8         = 0x1D;
    pub const SYNC: u8        = 0x2C;
    pub const FORGET: u8      = 0x22;
}

// ── Operand encoding ────────────────────────────────────────────────────────

const OP_NAMED: u8  = 0x01;
const OP_IMM: u8    = 0x02;
const OP_TYPE: u8   = 0x03;
const OP_ROLE: u8   = 0x04;
const OP_GLOBAL: u8 = 0x05;
const OP_FLOAT: u8  = 0x06;
/// A field read, `base.f1.f2` (2026-10-24): the base register's hash, the path
/// length, then each key as a length-prefixed UTF-8 string.
const OP_FIELD: u8  = 0x07;
/// The `null` literal (2026-10-24). It used to compile to the integer 0.
const OP_NULL: u8   = 0x08;
// P6-9: Named / Type / Role / Global / Field with an 8-byte key (the 2-byte tags above still decode)
const OP_NAMED8: u8  = 0x09;
const OP_TYPE8: u8   = 0x0A;
const OP_ROLE8: u8   = 0x0B;
const OP_GLOBAL8: u8 = 0x0C;
const OP_FIELD8: u8  = 0x0D;
const OP_NONE: u8   = 0x00;

/// BLAKE3 2-byte prefix of a string (deterministic hash).
/// A name's key (P6-9: BLAKE3's first 8 bytes; it was 2, and two spellings shared one). See `vm::key`.
fn name_hash(s: &str) -> crate::vm::key::Key {
    crate::vm::key::Key::wide(s)
}

/// Map an extended reasoning opcode to its canonical bytecode (opcode-vsa-rs ir.rs).
fn ext_bytecode(o: ExtOp) -> u8 {
    match o {
        ExtOp::Seq           => op::SEQ,
        ExtOp::Diff          => op::DIFF,
        ExtOp::DetectPattern => op::DETECT_PATTERN,
        ExtOp::Predict       => op::PREDICT,
        ExtOp::Match         => op::MATCH,
        ExtOp::Debate        => op::DEBATE,
        ExtOp::Discover      => op::DISCOVER,
        ExtOp::Decode        => op::DECODE,
        ExtOp::Score         => op::SCORE,
        ExtOp::Specialize    => op::SPECIALIZE,
        ExtOp::Reward        => op::REWARD,
        ExtOp::Infer         => op::INFER,
        ExtOp::Merge         => op::MERGE,
        ExtOp::Split         => op::SPLIT,
        ExtOp::Filter        => op::FILTER,
        ExtOp::MapRoles      => op::MAP_ROLES,
        ExtOp::Reduce        => op::REDUCE,
        ExtOp::Push          => op::PUSH,
        ExtOp::Sum           => op::SUM,
        ExtOp::Compare       => op::COMPARE,
        ExtOp::TemporalBind  => op::TEMPORAL_BIND,
        ExtOp::Analogy      => op::ANALOGY,
        ExtOp::Gen          => op::GEN,
        ExtOp::Inst         => op::INST,
        ExtOp::Broadcast    => op::BROADCAST,
        ExtOp::Explore      => op::EXPLORE,
        ExtOp::Forge        => op::FORGE,
        ExtOp::Ask          => op::ASK,
        ExtOp::Sync         => op::SYNC,
        ExtOp::Forget       => op::FORGET,
    }
}

/// Task 6: map an `asm { ... }` mnemonic (a raw source-text identifier,
/// e.g. `"BIND_ROLE"`) to its `op` byte. Exhaustive over the FULL `op`
/// table -- not narrowed to BIND_ROLE/UNBIND -- because `asm` is a general
/// escape hatch (the brief: "compile each by mapping mnemonic->op byte via
/// THE op table"), and every entry here just names an EXISTING `op::`
/// constant rather than inventing a new byte value, so this carries no
/// `opcode_sync` risk of its own (see `tests/opcode_sync.rs`): a mismatch
/// there would mean `op`'s own constants disagree with the sibling repo,
/// which is a pre-existing concern this table doesn't add to.
fn asm_mnemonic_to_op(mnemonic: &str) -> Option<u8> {
    Some(match mnemonic {
        "CREATE" => op::CREATE,
        "DESTROY" => op::DESTROY,
        "ASSIGN" => op::ASSIGN,
        "ADD" => op::ADD,
        "SUB" => op::SUB,
        "MUL" => op::MUL,
        "DIV" => op::DIV,
        "TRANSFER" => op::TRANSFER,
        "COPY" => op::COPY,
        "PUSH" => op::PUSH,
        "POP" => op::POP,
        "COMPARE" => op::COMPARE,
        "QUERY" => op::QUERY,
        "STORE" => op::STORE,
        "RECALL" => op::RECALL,
        "BIND_ROLE" => op::BIND_ROLE,
        "UNBIND" => op::UNBIND,
        "COND" => op::COND,
        "LOOP" => op::LOOP,
        "CALL" => op::CALL,
        "JMP" => op::JMP,
        "LABEL" => op::LABEL,
        "REMEMBER" => op::REMEMBER,
        "SUM" => op::SUM,
        "UNIFY" => op::UNIFY,
        "INST" => op::INST,
        "GEN" => op::GEN,
        "NEWVAR" => op::NEWVAR,
        "RETURN" => op::RETURN,
        "MAKE_ARRAY" => op::MAKE_ARRAY,
        "LEN" => op::LEN,
        "INDEX" => op::INDEX,
        "SET_INDEX" => op::SET_INDEX,
        "SKIP" => op::SKIP,
        "SEQ" => op::SEQ,
        "DIFF" => op::DIFF,
        "DETECT_PATTERN" => op::DETECT_PATTERN,
        "PREDICT" => op::PREDICT,
        "MATCH" => op::MATCH,
        "DEBATE" => op::DEBATE,
        "DISCOVER" => op::DISCOVER,
        "DECODE" => op::DECODE,
        "SCORE" => op::SCORE,
        "SPECIALIZE" => op::SPECIALIZE,
        "REWARD" => op::REWARD,
        "INFER" => op::INFER,
        "MERGE" => op::MERGE,
        "SPLIT" => op::SPLIT,
        "FILTER" => op::FILTER,
        "MAP_ROLES" => op::MAP_ROLES,
        "REDUCE" => op::REDUCE,
        "TEMPORAL_BIND" => op::TEMPORAL_BIND,
        "ANALOGY" => op::ANALOGY,
        "BROADCAST" => op::BROADCAST,
        "EXPLORE" => op::EXPLORE,
        "FORGE" => op::FORGE,
        "ASK" => op::ASK,
        "SYNC" => op::SYNC,
        "FORGET" => op::FORGET,
        _ => return None,
    })
}

/// The opcode an opcode name spells, in any case (`"ask"`, `"QUERY"`):
/// how a container's `safety { deny: [...] }` names what it refuses.
pub fn opcode_of(mnemonic: &str) -> Option<u8> {
    asm_mnemonic_to_op(&mnemonic.to_uppercase())
}

// ── Compiled output ─────────────────────────────────────────────────────────

/// Magic bytes for .cubebin files.
pub const CUBEBIN_MAGIC: &[u8; 4] = b"CUBE";
/// Current .cubebin format version.
///
/// v2 appends a trailing symbol-name table (role + filler names used by
/// BIND_ROLE/UNBIND) after the functions block. v1 readers stop after the
/// functions; v2 readers parse the table when present. The reader treats a
/// missing table (older/truncated input) as an empty table for compatibility.
///
/// v3 carries each function's DECLARED PARAMETER NAMES, between the function
/// name and its bytecode. Without them `VM::call` had nothing to bind host
/// arguments to and synthesized `arg0`/`arg1`, so a function declared
/// `solve(mention: str)` read an unbound register. v2 files still load: their
/// `params` is empty and `call` falls back to the old synthesis.
///
/// v4 (2026-10-24) appends the contract trailer after the symbol table: a
/// u32 LE length, then JSON (`ProgramMeta`): `used_modules`, the file's
/// types, and per function its override flag, access, `@once`,
/// `@restricted`, `@ratelimit`, `@cron` and declared types.
/// v5 (P6-9): names and literals keyed by 8 bytes in the bytecode (operand tags 0x09..0x0D).
pub const CUBEBIN_VERSION: u8 = 5;

/// The v4 contract trailer (see `CUBEBIN_VERSION`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ProgramMeta {
    used_modules: Vec<String>,
    types: crate::types::TypeTable,
    functions: Vec<FunctionMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    container: Option<ContainerMeta>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    storage_defaults: Vec<(String, serde_json::Value)>,
}

/// What a container (or world, modal, robot) declares beyond its functions
/// (2026-10-24). Its config, global storage and world state become storage
/// fields with constant starting values (`CompiledProgram::storage_defaults`);
/// these parts are the VM's to apply: the bindings it boots, the grants it
/// installs on load, and the limits every run entered through it gets.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ContainerMeta {
    /// `container`, `world`, `modal` or `robot`.
    pub kind: String,
    /// `programs { solver: deploy GSM8K(); }`: binding -> program name.
    pub programs: Vec<(String, String)>,
    /// `permissions { grant P.f to Q; revoke P.f from Q; }`, in order:
    /// (program, function, caller, true for grant).
    pub permissions: Vec<(String, String, String, bool)>,
    /// `safety { ... }`: the limits every run entered through it gets.
    pub safety: SafetyLimits,
}

/// A container's `safety { }` block. A zero is "no limit of its own".
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SafetyLimits {
    pub max_jumps: u64,
    pub max_ops: u64,
    pub max_queries: u64,
    pub max_wall_ms: u64,
    pub max_call_depth: u32,
    /// Opcode mnemonics no function of the container, or of a program it
    /// binds, may contain (checked at compile time).
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FunctionMeta {
    is_override: bool,
    access: u8,
    once: bool,
    restricted: Option<Vec<String>>,
    ratelimit: Option<(u32, u64)>,
    cron: Option<u64>,
    param_types: Vec<crate::types::TypeSpec>,
    return_type: crate::types::TypeSpec,
}

impl FunctionMeta {
    fn of(f: &CompiledFunction) -> Self {
        FunctionMeta {
            is_override: f.is_override,
            access: f.access.code(),
            once: f.once,
            restricted: f.restricted.clone(),
            ratelimit: f.ratelimit,
            cron: f.cron,
            param_types: f.param_types.clone(),
            return_type: f.return_type.clone(),
        }
    }

    fn apply(self, f: &mut CompiledFunction) {
        f.is_override = self.is_override;
        f.access = Access::from_code(self.access);
        f.once = self.once;
        f.restricted = self.restricted;
        f.ratelimit = self.ratelimit;
        f.cron = self.cron;
        f.param_types = self.param_types;
        f.return_type = self.return_type;
    }
}

/// A compiled CubeLang program — serializable to `.cubebin` format.
///
/// ## .cubebin binary format
///
/// ```text
/// [4 bytes]  magic: "CUBE"
/// [1 byte]   version: 0x01
/// [1 byte]   n_programs: u8
/// For each program:
///   [1 byte]   name_len: u8
///   [N bytes]  name: utf-8
///   [1 byte]   n_implements: u8
///   For each implements:
///     [1 byte]   iface_name_len: u8
///     [N bytes]  iface_name: utf-8
///   [1 byte]   n_storage_fields: u8
///   For each storage field:
///     [1 byte]   field_name_len: u8
///     [N bytes]  field_name: utf-8
///   [1 byte]   n_functions: u8
///   For each function:
///     [1 byte]   fn_name_len: u8
///     [N bytes]  fn_name: utf-8
///     [4 bytes]  bytecode_len: u32 LE
///     [N bytes]  bytecode
/// ```
#[derive(Debug, Clone)]
pub struct CompiledProgram {
    pub name: String,
    pub implements: Vec<String>,
    pub functions: Vec<CompiledFunction>,
    pub storage_fields: Vec<String>,
    /// Role and string-literal filler symbols that appear in `bind` statements.
    /// The bytecode carries only 2-byte blake3 hashes of these names; the VM
    /// reverses the hashes via this table so BIND_ROLE/UNBIND can drive the
    /// real VSA codebook (which keys on the symbol strings). Empty for programs
    /// with no bindings; loaded-from-disk programs repopulate it from .cubebin.
    pub symbol_names: Vec<String>,
    /// Task 3: VM registry module names this program's `SourceFile` brought
    /// into scope with top-level `use <name>;` declarations, in declaration
    /// order. Consulted by `op::CALL`'s override/registry/error resolution
    /// (`vm::registry::ModuleRegistry::resolve`). Deny-by-default: a module
    /// absent from this list is invisible to that program's calls even if
    /// the registry itself knows it.
    ///
    /// NOT YET carried through the `.cubebin` binary format -- `to_cubebin`/
    /// `from_cubebin` don't serialize it, so a save+load round trip silently
    /// drops it (same forward-compatible-gap shape as `params` before
    /// CUBEBIN_VERSION 3). Fine for in-memory compile+run, which is all
    /// Task 3 exercises; a future task should decide whether shipping
    /// `use`-dependent programs as `.cubebin` needs a version bump first.
    ///
    /// 2026-10-24: carried since `.cubebin` v4, with every function's
    /// contract and types, so a saved program keeps its permissions.
    pub used_modules: Vec<String>,
    /// The struct, enum and alias declarations of the source file, for the
    /// run-time type checks and struct values (2026-10-24, `.cubebin` v4).
    pub types: crate::types::TypeTable,
    /// Set when this program was compiled from a container (2026-10-24).
    pub container: Option<ContainerMeta>,
    /// Each storage field's starting value (2026-10-24, DRIFT B11): its
    /// declared default, which must be a constant, or an empty map / array
    /// for a field of that type. The VM sets them when it loads the program,
    /// before any constructor runs. A container's `config` and `world` are
    /// two of these.
    pub storage_defaults: Vec<(String, serde_json::Value)>,
    /// P6-9: the bytecode keys names by 2 bytes (a `.cubebin` before version 5). Everything compiled now keys by 8.
    pub narrow_keys: bool,
}

impl CompiledProgram {
    /// Serialize to `.cubebin` binary format.
    pub fn to_cubebin(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(CUBEBIN_MAGIC);
        buf.push(CUBEBIN_VERSION);
        buf.push(1); // n_programs (single program per file for now)

        // Program name
        buf.push(self.name.len() as u8);
        buf.extend_from_slice(self.name.as_bytes());

        // Implements
        buf.push(self.implements.len() as u8);
        for iface in &self.implements {
            buf.push(iface.len() as u8);
            buf.extend_from_slice(iface.as_bytes());
        }

        // Storage fields
        buf.push(self.storage_fields.len() as u8);
        for field in &self.storage_fields {
            buf.push(field.len() as u8);
            buf.extend_from_slice(field.as_bytes());
        }

        // Functions
        buf.push(self.functions.len() as u8);
        for func in &self.functions {
            buf.push(func.name.len() as u8);
            buf.extend_from_slice(func.name.as_bytes());
            // v3: declared parameter names, so a host can bind arguments by name
            // rather than by the synthesized `arg0`/`arg1` fallback.
            buf.push(func.params.len() as u8);
            for p in &func.params {
                buf.push(p.len() as u8);
                buf.extend_from_slice(p.as_bytes());
            }
            let bc_len = func.bytecode.len() as u32;
            buf.extend_from_slice(&bc_len.to_le_bytes());
            buf.extend_from_slice(&func.bytecode);
        }

        // Symbol-name table (v2+). u16 LE count, then each name as u8-len + utf8.
        // These are the role/filler symbols BIND_ROLE/UNBIND need to reverse
        // the 2-byte hashes carried in the bytecode.
        buf.extend_from_slice(&(self.symbol_names.len() as u16).to_le_bytes());
        for sym in &self.symbol_names {
            buf.push(sym.len() as u8);
            buf.extend_from_slice(sym.as_bytes());
        }

        // v4 (2026-10-24): the contract trailer -- `use`d modules, the file's
        // types, and per function its override flag, access, `@once`,
        // `@restricted`, `@ratelimit`, `@cron` and declared types. Before v4 a
        // saved program came back with every permission dropped, so the
        // access rules were one `compile -o` away from not existing.
        let meta = ProgramMeta {
            used_modules: self.used_modules.clone(),
            types: self.types.clone(),
            functions: self.functions.iter().map(FunctionMeta::of).collect(),
            container: self.container.clone(),
            storage_defaults: self.storage_defaults.clone(),
        };
        let json = serde_json::to_vec(&meta).expect("the contract trailer always serializes");
        buf.extend_from_slice(&(json.len() as u32).to_le_bytes());
        buf.extend_from_slice(&json);

        buf
    }

    /// Deserialize from `.cubebin` binary format.
    pub fn from_cubebin(data: &[u8]) -> Result<Self, String> {
        let mut pos = 0;

        // Magic
        if data.len() < 6 || &data[0..4] != CUBEBIN_MAGIC {
            return Err("invalid .cubebin: bad magic".into());
        }
        pos = 4;

        // Version
        let version = data[pos]; pos += 1;
        // v1: functions only. v2: + trailing symbol-name table.
        // v3: + per-function declared parameter names. All three still load;
        // older files simply carry no params and fall back to `argN` binding.
        // v4: + the contract trailer (see `to_cubebin`).
        // v5 (P6-9): the same layout, names keyed by 8 bytes in the bytecode.
        if !(1..=5).contains(&version) {
            return Err(format!("unsupported .cubebin version: {}", version));
        }

        // n_programs (we read 1)
        let _n_programs = data[pos]; pos += 1;

        // Program name
        let name_len = data[pos] as usize; pos += 1;
        let name = String::from_utf8_lossy(&data[pos..pos + name_len]).to_string();
        pos += name_len;

        // Implements
        let n_impl = data[pos] as usize; pos += 1;
        let mut implements = Vec::new();
        for _ in 0..n_impl {
            let len = data[pos] as usize; pos += 1;
            implements.push(String::from_utf8_lossy(&data[pos..pos + len]).to_string());
            pos += len;
        }

        // Storage fields
        let n_storage = data[pos] as usize; pos += 1;
        let mut storage_fields = Vec::new();
        for _ in 0..n_storage {
            let len = data[pos] as usize; pos += 1;
            storage_fields.push(String::from_utf8_lossy(&data[pos..pos + len]).to_string());
            pos += len;
        }

        // Functions
        let n_funcs = data[pos] as usize; pos += 1;
        let mut functions = Vec::new();
        for _ in 0..n_funcs {
            let fn_name_len = data[pos] as usize; pos += 1;
            let fn_name = String::from_utf8_lossy(&data[pos..pos + fn_name_len]).to_string();
            pos += fn_name_len;

            // v3 added declared parameter names here. v2 files have none: they
            // load with an empty `params`, and `VM::call` falls back to the old
            // `arg0`/`arg1` synthesis for them. Forward-compatible, not silent.
            let mut params: Vec<String> = Vec::new();
            if version >= 3 {
                let n_params = data[pos] as usize; pos += 1;
                for _ in 0..n_params {
                    let l = data[pos] as usize; pos += 1;
                    params.push(String::from_utf8_lossy(&data[pos..pos + l]).to_string());
                    pos += l;
                }
            }

            let bc_len = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
            pos += 4;
            let bytecode = data[pos..pos + bc_len].to_vec();
            pos += bc_len;

            let n_params = params.len();
            functions.push(CompiledFunction {
                name: fn_name,
                params,
                bytecode,
                labels: Vec::new(),
                debug_lines: Vec::new(),
                // Before v4 none of this was in the file; the v4 trailer below
                // overwrites it. A v1-v3 file loads as it always did.
                is_override: false,
                access: Access::External,
                once: false,
                restricted: None,
                ratelimit: None,
                cron: None,
                param_types: vec![crate::types::TypeSpec::Any; n_params],
                return_type: crate::types::TypeSpec::Any,
            });
        }

        // Symbol-name table (v2+). Present when at least the u16 count remains.
        // v1 input (or truncated) leaves this empty for backward compatibility.
        let mut symbol_names = Vec::new();
        if version >= 2 && pos + 2 <= data.len() {
            let n_syms = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            for _ in 0..n_syms {
                if pos >= data.len() { break; }
                let len = data[pos] as usize; pos += 1;
                if pos + len > data.len() { break; }
                symbol_names.push(String::from_utf8_lossy(&data[pos..pos + len]).to_string());
                pos += len;
            }
        }

        // v4: the contract trailer. A v4 file without one (truncated) is an
        // error, not a program with its permissions silently dropped.
        let mut used_modules = Vec::new();
        let mut types = crate::types::TypeTable::default();
        let mut container = None;
        let mut storage_defaults = Vec::new();
        if version >= 4 {
            if pos + 4 > data.len() {
                return Err("invalid .cubebin v4: missing the contract trailer".into());
            }
            let n = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
            pos += 4;
            if pos + n > data.len() {
                return Err("invalid .cubebin v4: truncated contract trailer".into());
            }
            let meta: ProgramMeta = serde_json::from_slice(&data[pos..pos + n])
                .map_err(|e| format!("invalid .cubebin v4 contract trailer: {}", e))?;
            if meta.functions.len() != functions.len() {
                return Err(format!(
                    "invalid .cubebin v4: the trailer describes {} function(s), the file has {}",
                    meta.functions.len(), functions.len()));
            }
            for (f, m) in functions.iter_mut().zip(meta.functions) {
                m.apply(f);
            }
            used_modules = meta.used_modules;
            types = meta.types;
            container = meta.container;
            storage_defaults = meta.storage_defaults;
        }
        Ok(CompiledProgram { name, implements, functions, storage_fields, symbol_names, used_modules, types, container,
                             storage_defaults, narrow_keys: version < 5 })
    }

    /// Save to a `.cubebin` file.
    ///
    /// Fix-round-1 (Task 3): the binary format doesn't carry `used_modules`/
    /// `is_override` (see those fields' docs on `CompiledProgram`/
    /// `CompiledFunction`) -- serializing a program that uses either would
    /// silently round-trip into a capability-free program with NO warning:
    /// `use demo; return greet();` returns 42 run from source, but a
    /// `compile foo.cube -o foo.cubebin` + `run foo.cubebin` of the exact
    /// same program would error, because the reloaded program `use`s
    /// nothing. Refuse instead of producing a binary that silently
    /// disagrees with its own source.
    ///
    /// 2026-10-24: `.cubebin` v4 carries `used_modules`, `is_override` and
    /// every function's contract and types, so a saved program is the same
    /// program; the refusal is gone.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_cubebin())
    }

    /// Load from a `.cubebin` file.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::from_cubebin(&data)
    }
}

/// A compiled function.
#[derive(Debug, Clone)]
pub struct CompiledFunction {
    pub name: String,
    /// Declared parameter names, in order.
    ///
    /// These used to be discarded at compile time, which left `VM::call` with
    /// nothing to bind host arguments to. It fell back to synthesizing `arg0`,
    /// `arg1`, ... -- so a `.cube` function declared `solve(mention: str)` read
    /// an UNBOUND register when called from a host. Every example in the SPEC
    /// hits this. Carrying the names fixes it at the source.
    pub params: Vec<String>,
    pub bytecode: Vec<u8>,
    pub labels: Vec<(String, usize)>,   // label name → byte offset
    pub debug_lines: Vec<DebugLine>,    // source mapping
    /// Task 3: was this function declared with the postfix `override`
    /// marker (`ast::FunctionSig::is_override`)? Compile-time validation
    /// (`Compiler::check_override_validity`) already confirmed the name is
    /// legitimately overridable in a `use`'d module before this is ever
    /// `true`, so `op::CALL` can trust it directly at resolution time
    /// instead of re-checking. NOT carried through `.cubebin` yet (see
    /// `CompiledProgram::used_modules`'s doc — same gap, same reason).
    pub is_override: bool,
    /// VM step 0 (2026-09-29): who may call this function, from the
    /// `@external` / `@internal` / `@system` attribute (External when none is
    /// written -- unchanged behaviour for every unmarked function). Enforced
    /// by `VM::call_entry`, the host's entry: `@internal` and `@system` are
    /// refused there. Not carried through `.cubebin` (same gap as
    /// `is_override`).
    pub access: Access,
    /// `@once`: the VM refuses a second run of this function
    /// (`VM::exec_function`). Carried through `.cubebin` since v4.
    pub once: bool,
    /// `@restricted(...)` (2026-10-24): the programs allowed to call this
    /// function from outside, named by the host as `VM::caller`; a
    /// container's `grant P.f to Q` adds to it (`VM::grants`). `None`: no
    /// restriction. `Some([])` (a bare `@restricted`): only granted callers.
    pub restricted: Option<Vec<String>>,
    /// `@ratelimit(n, period)`: at most `n` calls from outside per `period`
    /// milliseconds, counted by `VM::governor` (which a resident host keeps
    /// across requests). `None`: unlimited.
    pub ratelimit: Option<(u32, u64)>,
    /// `@cron(interval)`: run by the VM's scheduler (`VM::tick`) every
    /// `interval` milliseconds, and refused as an entry from outside.
    pub cron: Option<u64>,
    /// The declared parameter and return types (2026-10-24). Checked at run
    /// time on what crosses the function's boundary: the arguments on entry,
    /// the value on return (`VM::check_boundary`). `Any` when undeclared.
    pub param_types: Vec<crate::types::TypeSpec>,
    pub return_type: crate::types::TypeSpec,
}

/// Who may call a function, from its access attribute. The three the SPEC
/// names; every unmarked function is `External`, as it always effectively was.
/// `private` (the modifier) is `Internal` when no access attribute says
/// otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    External,
    Internal,
    System,
}

impl Access {
    fn code(self) -> u8 {
        match self { Access::External => 0, Access::Internal => 1, Access::System => 2 }
    }
    fn from_code(c: u8) -> Self {
        match c { 1 => Access::Internal, 2 => Access::System, _ => Access::External }
    }
}

/// Everything a signature's attributes promise about who may call it, how
/// often and when (2026-10-24: all enforced; `@hook`/`@before`/`@after` are
/// still strict errors -- the VM has no event bus or middleware).
#[derive(Debug, Clone, PartialEq)]
pub struct FnContract {
    pub access: Access,
    pub once: bool,
    pub restricted: Option<Vec<String>>,
    pub ratelimit: Option<(u32, u64)>,
    pub cron: Option<u64>,
}

/// The access attribute a signature declares, and the `@once` flag.
pub fn access_of(sig: &FunctionSig) -> (Access, bool) {
    let (c, _) = contract_of(sig);
    (c.access, c.once)
}

/// The contract a signature's attributes declare, with every attribute that
/// cannot mean anything (a malformed period, a zero rate, `private` beside
/// `@external`) reported as an error -- an attribute the VM cannot honour is
/// worse than none.
pub fn contract_of(sig: &FunctionSig) -> (FnContract, Vec<String>) {
    let mut c = FnContract { access: Access::External, once: false, restricted: None, ratelimit: None, cron: None };
    let mut errors = Vec::new();
    let mut explicit_access = None;
    for p in &sig.permissions {
        match p {
            Permission::External => { c.access = Access::External; explicit_access = Some(Access::External); }
            Permission::Internal => { c.access = Access::Internal; explicit_access = Some(Access::Internal); }
            Permission::System => { c.access = Access::System; explicit_access = Some(Access::System); }
            Permission::Once => c.once = true,
            Permission::Restricted(names) => {
                c.restricted.get_or_insert_with(Vec::new).extend(names.iter().cloned());
            }
            Permission::Ratelimit(n, period) => {
                if *n <= 0 || *n > u32::MAX as i64 {
                    errors.push(format!("@ratelimit({}, {:?}) on {}: the count must be at least 1", n, period, sig.name));
                } else {
                    match crate::vm::governor::parse_period(period) {
                        Ok(d) => c.ratelimit = Some((*n as u32, d.as_millis() as u64)),
                        Err(e) => errors.push(format!("@ratelimit on {}: {}", sig.name, e)),
                    }
                }
            }
            Permission::Cron(interval) => match crate::vm::governor::parse_period(interval) {
                Ok(d) => c.cron = Some(d.as_millis() as u64),
                Err(e) => errors.push(format!("@cron on {}: {}", sig.name, e)),
            },
            Permission::Hook(_) | Permission::Before(_) | Permission::After(_) | Permission::Inputs(_) => {}
        }
    }
    if sig.modifiers.contains(&Modifier::Private) {
        match explicit_access {
            Some(Access::External) => errors.push(format!(
                "{} is both `private` and `@external`: one says no outside caller, the other says any", sig.name)),
            None => c.access = Access::Internal,
            Some(_) => {}
        }
    }
    (c, errors)
}

impl CompiledFunction {
    pub fn sig_name(&self) -> &str {
        &self.name
    }

    pub fn hex(&self) -> String {
        self.bytecode.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")
    }
}

/// Source-level debug info per instruction.
#[derive(Debug, Clone)]
pub struct DebugLine {
    pub offset: usize,
    pub opcode: u8,
    pub description: String,
}

// ── Compiler ────────────────────────────────────────────────────────────────

pub struct Compiler {
    bytecode: Vec<u8>,
    labels: Vec<(String, usize)>,
    debug_lines: Vec<DebugLine>,
    label_counter: u32,
    /// Strict mode: rejects non-executing constructs (P0-1). Errors accumulate
    /// in `strict_errors`; the public `compile_strict` API surfaces them.
    strict: bool,
    strict_errors: Vec<String>,
    current_program: String,
    current_function: String,
    /// Role + string-literal filler names seen in `bind` statements, collected
    /// so the VM can reverse the bytecode's 2-byte hashes for genuine VSA
    /// binding. Deduplicated on insertion; attached to each CompiledProgram.
    symbol_names: Vec<String>,
    /// Task 3: module names the current `SourceFile` `use`s, gathered once
    /// per `compile()` call (from its `TopLevel::Use` items) before any
    /// program is compiled. Attached verbatim to every `CompiledProgram`
    /// this call produces, and consulted by `check_override_validity`.
    used_modules: Vec<String>,
    /// Task 3: `use`/`override`/registry misuse, two shapes: (1) an
    /// `override`-marked function whose name isn't genuinely overridable in
    /// a `use`'d module (`check_override_validity`); (2) a PLAIN
    /// (non-`override`) function whose name collides with a SEALED
    /// (non-overridable) export of a `use`'d module -- silently unreachable
    /// otherwise, since the registry always wins there and a sealed name has
    /// no `override` escape hatch (`check_sealed_collision`, fix round 1).
    /// An overridable collision is deliberately NOT an error here -- the
    /// registry winning is correct in that case, and `override` is the
    /// discoverable escape.
    ///
    /// Unlike `strict_errors` (only checked under `--strict`), these are
    /// always checked -- both are bugs regardless of strict mode. Reuses
    /// `parser::ParseError`'s shape (message + span) rather than introducing
    /// a new error type for a couple more compile-time checks; both
    /// free-function entry points (`compile`, `compile_strict`) surface it.
    capability_errors: Vec<ParseError>,
    /// Task 4: every in-file `interface X { ... }` declaration
    /// (`TopLevel::Interface`) this `SourceFile` carries, gathered once per
    /// `compile()` call the same way `used_modules` is -- keyed by name (an
    /// interface name repeated in one file overwrites the earlier entry; the
    /// parser doesn't reject a duplicate name and no current program hits
    /// this). Consulted by `check_implements` as the FALLBACK resolution
    /// path for an `implements` name the VM `InterfaceRegistry` doesn't
    /// recognize -- see that method's doc for why the registry is always
    /// checked first, not the reverse.
    inline_interfaces: HashMap<String, InterfaceDecl>,
    /// Task 4: `implements X` misuse -- `X` resolves to neither the VM
    /// interface registry nor an in-file inline `interface` decl, or it
    /// resolves to one but the program is missing a required (abstract,
    /// non-optional) same-name/same-arity member. Kept separate from
    /// `capability_errors` even though both ultimately fold into the same
    /// `compile`/`compile_strict` error surface -- interfaces are a
    /// conceptually distinct, separate registry (contracts, not callable
    /// impls) per the Task 4 brief, and keeping the accumulators apart keeps
    /// it obvious at a glance which check produced which error. Checked
    /// unconditionally, like `capability_errors`, not just under `--strict`.
    interface_errors: Vec<ParseError>,
    /// Task 6: an `asm { ... }` instruction naming a mnemonic absent from
    /// `asm_mnemonic_to_op`'s table. Kept separate from `capability_errors`/
    /// `interface_errors` for the same reason those two are kept apart from
    /// each other -- a distinct check, obvious at a glance which one fired
    /// -- but folded into the exact same `compile`/`compile_strict` error
    /// surface, checked unconditionally like its siblings, not just under
    /// `--strict`. A raw mnemonic typo is source-level user error (unlike a
    /// truly unknown *byte* reaching the VM, which is a bytecode invariant
    /// violation the engine itself rejects) -- it belongs at compile time.
    asm_errors: Vec<ParseError>,
    /// DRIFT.md §C11 fix: `compile_expr_operand` only has a real encoding
    /// for a literal, a bare identifier, `self.field`, and a simple
    /// `expr.field`. Every other expression shape (`BinOp`, `UnaryOp`,
    /// `StructLit`, `MapLit`, `Lambda`, `Await`, `Deploy`, `Proxy`, a nested
    /// `Call`/`MethodCall`, `Index`, ...) used to silently fall through to
    /// an opaque `OP_NONE` placeholder -- e.g. `double_it(x - y)` compiled
    /// clean and ran as `double_it(<garbage>)`, and `--strict` never
    /// noticed because it checks statement SHAPES, never descends into an
    /// individual operand. Pushed to whenever that catch-all fires, and
    /// checked UNCONDITIONALLY (like `capability_errors`/`interface_errors`/
    /// `asm_errors` above) -- not just under `--strict` -- since this is the
    /// one silent-garbage path strict mode could not catch, so a mode flag
    /// is not an acceptable gate for it.
    operand_errors: Vec<ParseError>,
    /// Best-effort source location for `operand_errors` messages. Updated at
    /// the top of `compile_stmt` from whichever `Stmt` variants carry a
    /// `Span` (see `stmt_span`); left at its previous value for statements
    /// that don't (e.g. `Stmt::Return`), so an operand error deep inside one
    /// of those still reports the nearest enclosing statement's line rather
    /// than nothing. Reset to `None` at the start of every function so a
    /// span never leaks from one function into the next.
    current_span: Option<Span>,
    /// 2026-10-24: the source file's struct / enum / alias declarations
    /// (`types::table_of`), gathered once per `compile()` like `used_modules`.
    types: crate::types::TypeTable,
    /// The struct declarations themselves (with their default expressions),
    /// for struct literals.
    struct_decls: HashMap<String, StructDecl>,
    /// The current program's functions' parameter types, for checking the
    /// literal arguments of a call at compile time.
    signatures: HashMap<String, Vec<crate::types::TypeSpec>>,
    /// The declared return type of the function being compiled.
    current_return: crate::types::TypeSpec,
    /// Every program and container of the file, by name, with its
    /// functions' signatures (2026-10-24): what a call into another program
    /// (`Other.f(x)`, a container's `self.programs.b.f(x)`) is checked
    /// against at compile time.
    units: HashMap<String, Vec<FunctionSig>>,
    /// The current container's `programs { b: deploy P(); }`: binding -> P.
    bindings: HashMap<String, String>,
    /// P6-1: the names the current program's functions may read without declaring them: its storage, the
    /// file's functions, programs, containers, structs, enums, aliases, `use`d modules, the container's bindings.
    known_names: std::collections::HashSet<String>,
    /// P6-9: every key this compilation emitted, with its spelling (a second spelling on one key is an error).
    keyed: HashMap<crate::vm::key::Key, String>,
    /// The current program's immutable storage: `(field, None)` for a whole
    /// field, `(field, Some(sub))` for one field of a container's `world`.
    /// Only the constructor (and a container's `on_start`) may write them.
    immutable: Vec<(String, Option<String>)>,
    /// The current program's storage starting values (`storage_defaults`).
    storage_defaults: Vec<(String, serde_json::Value)>,
    /// The program being compiled is a container's.
    in_container: bool,
}

impl Compiler {
    pub fn new() -> Self {
        Self {
            bytecode: Vec::new(),
            labels: Vec::new(),
            debug_lines: Vec::new(),
            label_counter: 0,
            strict: false,
            strict_errors: Vec::new(),
            current_program: String::new(),
            current_function: String::new(),
            symbol_names: Vec::new(),
            used_modules: Vec::new(),
            capability_errors: Vec::new(),
            inline_interfaces: HashMap::new(),
            interface_errors: Vec::new(),
            asm_errors: Vec::new(),
            operand_errors: Vec::new(),
            current_span: None,
            types: crate::types::TypeTable::default(),
            struct_decls: HashMap::new(),
            signatures: HashMap::new(),
            current_return: crate::types::TypeSpec::Any,
            units: HashMap::new(),
            bindings: HashMap::new(),
            known_names: std::collections::HashSet::new(),
            keyed: HashMap::new(),
            immutable: Vec::new(),
            storage_defaults: Vec::new(),
            in_container: false,
        }
    }

    /// A literal against a declared type, at compile time (2026-10-24): `null`
    /// and non-literals pass (the VM checks those as they cross a call).
    fn check_literal(&mut self, spec: &crate::types::TypeSpec, value: &Expr, what: &str) {
        if let Some(lit) = crate::types::TypeSpec::of_literal(value) {
            if !spec.accepts_literal(&lit, &self.types) {
                self.literal_error(format!("{} is declared {} but is given {}", what, spec.spell(), lit.spell()));
            }
        }
    }

    pub fn new_strict() -> Self {
        let mut c = Self::new();
        c.strict = true;
        c
    }

    fn strict_violation(&mut self, construct: &str, line: Option<u32>) {
        let where_ = format!("{}::{}", self.current_program, self.current_function);
        let msg = match line {
            Some(l) => format!("strict: {} at line {} in {} (non-executing in safe-subset VM)",
                construct, l, where_),
            None => format!("strict: {} in {} (non-executing in safe-subset VM)",
                construct, where_),
        };
        self.strict_errors.push(msg);
    }

    /// Compile a full source file.
    pub fn compile(&mut self, source: &SourceFile) -> Vec<CompiledProgram> {
        // Task 3: gather every top-level `use` BEFORE compiling any program,
        // so `compile_program`/`compile_function` can see the full set
        // regardless of where a `use` sits relative to a `program` block in
        // source order. "That compilation unit" (design doc) is the whole
        // `SourceFile`: every program compiled from it shares one `use` list.
        self.used_modules = source.items.iter()
            .filter_map(|item| match item {
                TopLevel::Use(name) => Some(name.clone()),
                _ => None,
            })
            .collect();

        // Task 4: gather every in-file `interface` decl BEFORE compiling any
        // program, mirroring `used_modules` just above -- an inline decl may
        // sit anywhere in source order relative to the `program` block(s)
        // that `implements` it (every current example declares it first, but
        // nothing requires that).
        self.inline_interfaces = source.items.iter()
            .filter_map(|item| match item {
                TopLevel::Interface(decl) => Some((decl.name.clone(), decl.clone())),
                _ => None,
            })
            .collect();

        // 2026-10-24: the file's struct / enum / alias declarations, before
        // any program compiles -- signatures, struct literals and the
        // run-time checks all resolve type names through it.
        self.types = crate::types::table_of(source);
        self.struct_decls = crate::types::all_structs(source).into_iter()
            .map(|decl| (decl.name.clone(), decl.clone()))
            .collect();

        // 2026-10-24: every program and container of the file with its
        // functions, before any compiles: a call into another program is
        // checked against it, wherever that program sits in the file.
        self.units = source.items.iter()
            .filter_map(|item| match item {
                TopLevel::Program(p) => Some((p.name.clone(), p.body.iter().filter_map(|i| match i {
                    ProgramItem::Function(f) | ProgramItem::Constructor(f) => Some(f.sig.clone()),
                    _ => None,
                }).collect())),
                TopLevel::Container(c) => Some((c.name.clone(), c.body.iter().filter_map(|i| match i {
                    ContainerItem::Function(f) => Some(f.sig.clone()),
                    _ => None,
                }).collect())),
                _ => None,
            })
            .collect();

        let mut programs = Vec::new();
        for item in &source.items {
            match item {
                TopLevel::Program(p) => programs.push(self.compile_program(p)),
                // 2026-10-24: a container compiles to a program of its own
                // (DRIFT C10): its functions, its storage, its config and world
                // as storage, and the meta the VM applies.
                TopLevel::Container(c) => programs.push(self.compile_container(c)),
                TopLevel::EventDecl(e) if self.strict => self.strict_errors.push(format!(
                    "strict: event {} at line {} (events are declared but the VM has no event bus)",
                    e.name, e.span.line)),
                TopLevel::ExtendBlock(x) if self.strict => self.strict_errors.push(format!(
                    "strict: extend {} at line {} (extend blocks are never compiled)",
                    x.target, x.span.line)),
                _ => {}
            }
        }
        self.check_denied(&programs);
        programs
    }

    fn compile_program(&mut self, prog: &ProgramDecl) -> CompiledProgram {
        self.compile_program_as(prog, None)
    }

    /// Compile one program, or (with `container`) the program a container
    /// desugars to.
    fn compile_program_as(&mut self, prog: &ProgramDecl, container: Option<ContainerMeta>) -> CompiledProgram {
        let mut functions = Vec::new();
        let mut storage_fields = Vec::new();
        self.current_program = prog.name.clone();
        // Reset per-program symbol collection so bindings don't leak across
        // programs compiled in the same pass.
        self.symbol_names.clear();
        let is_container = container.is_some();
        self.in_container = is_container;
        if !is_container {
            self.bindings.clear();
            self.immutable.clear();
            self.storage_defaults.clear();
        }

        // Task 4: validate `implements` against the VM interface registry or
        // an in-file inline decl before compiling the body -- see
        // check_implements's doc for the registry-first resolution order.
        // A container needs no interface of its own (it is the deployment
        // unit), but one it names is checked like a program's.
        if !(is_container && prog.implements.is_empty()) {
            self.check_implements(prog);
        }

        // A program's own `type Input = number;` aliases join the file's
        // types for this program (and travel with it, for the VM's checks).
        let file_types = self.types.clone();
        for item in &prog.body {
            if let ProgramItem::TypeAlias(a) = item {
                self.types.aliases.insert(a.name.clone(), crate::types::TypeSpec::Any);
            }
        }
        for item in &prog.body {
            if let ProgramItem::TypeAlias(a) = item {
                let spec = crate::types::TypeSpec::from_expr(&a.ty, &self.types);
                self.types.aliases.insert(a.name.clone(), spec);
            }
        }

        self.signatures = prog.body.iter()
            .filter_map(|item| match item {
                ProgramItem::Function(f) | ProgramItem::Constructor(f) => Some((
                    f.sig.name.clone(),
                    f.sig.params.iter().map(|p| crate::types::TypeSpec::from_expr(&p.ty, &self.types)).collect(),
                )),
                _ => None,
            })
            .collect();

        for item in &prog.body {
            if let ProgramItem::Storage(s) = item {
                for f in &s.fields {
                    if storage_fields.contains(&f.name) {
                        self.container_error(format!("storage field `{}` is declared twice in {}", f.name, prog.name), f.span);
                    }
                    storage_fields.push(f.name.clone());
                    self.storage_field(f);
                }
            }
        }
        // A container's `config` and `world` are storage the desugaring
        // already recorded (with their starting values).
        for (name, _) in &self.storage_defaults {
            if !storage_fields.contains(name) {
                storage_fields.push(name.clone());
            }
        }
        // P6-1: what a function may read without declaring it (strict mode's undeclared-name check).
        self.known_names = storage_fields.iter().cloned()
            .chain(self.signatures.keys().cloned())
            .chain(self.units.keys().cloned())
            .chain(self.used_modules.iter().cloned())
            .chain(self.struct_decls.keys().cloned())
            .chain(self.types.structs.keys().cloned())
            .chain(self.types.enums.keys().cloned())
            .chain(self.types.aliases.keys().cloned())
            .chain(self.bindings.keys().cloned())
            .collect();
        for item in &prog.body {
            if let ProgramItem::Function(f) | ProgramItem::Constructor(f) = item {
                functions.push(self.compile_function(f));
            }
        }

        let types = std::mem::replace(&mut self.types, file_types);
        self.bindings.clear();
        self.immutable.clear();
        self.in_container = false;
        CompiledProgram {
            name: prog.name.clone(),
            implements: prog.implements.clone(),
            functions,
            storage_fields,
            symbol_names: std::mem::take(&mut self.symbol_names),
            used_modules: self.used_modules.clone(),
            types,
            container,
            storage_defaults: std::mem::take(&mut self.storage_defaults),
            narrow_keys: false,
        }
    }

    /// A storage field's starting value and mutability (2026-10-24, DRIFT
    /// B11). A declared default must be a constant -- a literal, or an array,
    /// map or struct of literals -- because the VM sets it when it loads the
    /// program, before anything runs; anything else is set in the constructor.
    /// A field of a map or array type with no default starts empty.
    fn storage_field(&mut self, f: &StorageField) {
        let spec = crate::types::TypeSpec::from_expr(&f.ty, &self.types);
        if f.immutable {
            self.immutable.push((f.name.clone(), None));
        }
        match &f.default {
            Some(d) => {
                self.check_literal(&spec, d, &format!("storage field `{}`", f.name));
                match self.const_json(d) {
                    Ok(v) => self.storage_defaults.push((f.name.clone(), v)),
                    Err(why) => self.container_error(format!(
                        "storage field `{}`: its default {}; set it in the constructor instead", f.name, why), f.span),
                }
            }
            None => {
                if let Some(v) = empty_of(&spec, &self.types, 0) {
                    self.storage_defaults.push((f.name.clone(), v));
                }
            }
        }
    }

    /// A constant expression as the JSON the VM loads it from.
    fn const_json(&self, e: &Expr) -> Result<serde_json::Value, String> {
        use serde_json::Value as J;
        let finite = |f: f64| serde_json::Number::from_f64(f).map(J::Number)
            .ok_or_else(|| "is not a finite number".to_string());
        Ok(match e {
            Expr::IntLit(n) => J::from(*n),
            Expr::FloatLit(f) => finite(*f)?,
            Expr::StringLit(s) => J::String(s.clone()),
            Expr::BoolLit(b) => J::Bool(*b),
            Expr::Null => J::Null,
            Expr::UnaryOp(UnaryOp::Neg, inner) => match inner.as_ref() {
                Expr::IntLit(n) => J::from(n.checked_neg().ok_or_else(|| "overflows an i64".to_string())?),
                Expr::FloatLit(f) => finite(-*f)?,
                _ => return Err("is not a constant".into()),
            },
            Expr::ArrayLit(xs) => J::Array(xs.iter().map(|x| self.const_json(x)).collect::<Result<_, _>>()?),
            Expr::MapLit(pairs) => {
                let mut m = serde_json::Map::new();
                for (k, v) in pairs {
                    let key = match k {
                        Expr::Ident(w) | Expr::StringLit(w) => w.clone(),
                        _ => return Err("has a map key that is not a word or a string".into()),
                    };
                    m.insert(key, self.const_json(v)?);
                }
                J::Object(m)
            }
            Expr::StructLit(name, given) => {
                let decl = self.struct_decls.get(name)
                    .ok_or_else(|| format!("names {}, which is not a declared struct", name))?;
                for (i, (field, _)) in given.iter().enumerate() {
                    if !decl.fields.iter().any(|f| &f.name == field) {
                        return Err(format!("gives struct {} a field `{}` it does not declare", name, field));
                    }
                    if given[..i].iter().any(|(g, _)| g == field) {
                        return Err(format!("gives `{}.{}` twice", name, field));
                    }
                }
                let mut m = serde_json::Map::new();
                for f in &decl.fields {
                    let v = match given.iter().find(|(n, _)| n == &f.name) {
                        Some((_, v)) => v,
                        None => f.default.as_ref()
                            .ok_or_else(|| format!("leaves out `{}.{}`, which has no default", name, f.name))?,
                    };
                    let spec = crate::types::TypeSpec::from_expr(&f.ty, &self.types);
                    if let Some(lit) = crate::types::TypeSpec::of_literal(v) {
                        if !spec.accepts_literal(&lit, &self.types) {
                            return Err(format!("gives `{}.{}` (declared {}) a {}", name, f.name, spec.spell(), lit.spell()));
                        }
                    }
                    m.insert(f.name.clone(), self.const_json(v)?);
                }
                J::Object(m)
            }
            _ => return Err("is not a constant (a literal, or an array, map or struct of literals)".into()),
        })
    }

    /// A container-level error (always checked, like an operand error).
    fn container_error(&mut self, message: String, span: Span) {
        self.operand_errors.push(ParseError { message, span });
    }

    /// A container (or `world`, `modal`, `robot`) compiles to a program of
    /// the same name (2026-10-24, DRIFT C10):
    /// - its functions are the program's; the VM boots it by running its
    ///   bound programs' constructors, its own `constructor`, then `on_start`;
    /// - `config { k: v; }` is an immutable storage field `config`, a map of
    ///   constants (`self.config.k`);
    /// - `storage { }` / `global storage { }` are its storage;
    /// - `world { }` is a storage field `world`, a map of the world's fields
    ///   (`self.world.time`); an `immutable` one is written only by the
    ///   constructor and `on_start`;
    /// - `programs { b: deploy P(); }` binds the programs it runs, which its
    ///   code calls as `self.programs.b.f(x)` or `b.f(x)`;
    /// - `permissions { grant b.f to c; }` is installed when it loads;
    /// - `safety { }` caps every run entered through it.
    /// What the VM cannot run -- `io`, `agents`, `modalities`, `sensors`,
    /// `actuators`, `fusion`, inheriting another container -- is a strict
    /// error; a safety setting it cannot enforce is an error in every mode.
    fn compile_container(&mut self, c: &ContainerDecl) -> CompiledProgram {
        use serde_json::Value as J;
        self.current_program = c.name.clone();
        self.current_function.clear();
        self.current_span = Some(c.span);
        self.bindings.clear();
        self.immutable.clear();
        self.storage_defaults.clear();
        let kind = match c.kind {
            ContainerKind::Container => "container",
            ContainerKind::World => "world",
            ContainerKind::Modal => "modal",
            ContainerKind::Robot => "robot",
        };
        let mut meta = ContainerMeta { kind: kind.to_string(), ..Default::default() };
        let strict = self.strict;
        let not_run = |errors: &mut Vec<String>, what: &str, line: u32| {
            if strict {
                errors.push(format!("strict: {} in {} at line {} (non-executing in safe-subset VM)", what, c.name, line));
            }
        };
        if let Some(base) = &c.extends {
            if base != "Container" {
                not_run(&mut self.strict_errors, &format!("`extends {}` (a container cannot inherit another; only the base `Container` exists)", base), c.span.line);
            }
        }

        // The bindings first: permissions and calls refer to them.
        for item in &c.body {
            let ContainerItem::Programs(bindings) = item else { continue };
            for b in bindings {
                let named = match &b.expr {
                    Expr::Deploy(p, args) if args.is_empty() => Some(p.clone()),
                    Expr::Deploy(p, _) => {
                        self.container_error(format!(
                            "{}: `{}: deploy {}(...)` passes arguments, but a program's constructor takes none \
                             (set them through its functions after boot)", c.name, b.name, p), b.span);
                        None
                    }
                    Expr::Ident(p) => Some(p.clone()),
                    Expr::MethodCall(obj, m, args) if m == "new" && args.is_empty() => match obj.as_ref() {
                        Expr::Ident(p) => Some(p.clone()),
                        _ => None,
                    },
                    Expr::Proxy(p, _) => {
                        self.container_error(format!(
                            "{}: `{}: proxy {}(...)`: proxies are not implemented; bind the program itself",
                            c.name, b.name, p), b.span);
                        continue;
                    }
                    _ => None,
                };
                let Some(p) = named else {
                    if !matches!(b.expr, Expr::Deploy(..)) {
                        self.container_error(format!(
                            "{}: the binding `{}` must name a program (`{}: deploy P();`)", c.name, b.name, b.name), b.span);
                    }
                    continue;
                };
                if p == c.name {
                    self.container_error(format!("{} binds itself as `{}`", c.name, b.name), b.span);
                } else if !self.units.contains_key(&p) {
                    self.container_error(format!(
                        "{} binds `{}` to {}, which is no program of this file (import the file that declares it)",
                        c.name, b.name, p), b.span);
                } else if self.bindings.insert(b.name.clone(), p.clone()).is_some() {
                    self.container_error(format!("{} binds `{}` twice", c.name, b.name), b.span);
                } else {
                    meta.programs.push((b.name.clone(), p));
                }
            }
        }

        let mut body: Vec<ProgramItem> = Vec::new();
        let mut safety: Vec<ConfigField> = Vec::new();
        for item in &c.body {
            match item {
                ContainerItem::Config(fields) => {
                    if self.storage_defaults.iter().any(|(n, _)| n == "config") {
                        self.container_error(format!("{} declares `config` twice", c.name), c.span);
                        continue;
                    }
                    let mut m = serde_json::Map::new();
                    for f in fields {
                        if m.contains_key(&f.name) {
                            self.container_error(format!("{}: config `{}` is set twice", c.name, f.name), f.span);
                            continue;
                        }
                        match self.const_json(&f.value) {
                            Ok(v) => { m.insert(f.name.clone(), v); }
                            Err(why) => self.container_error(format!("{}: config `{}`: its value {}", c.name, f.name, why), f.span),
                        }
                    }
                    self.storage_defaults.push(("config".into(), J::Object(m)));
                    self.immutable.push(("config".into(), None));
                }
                ContainerItem::Storage(s) => body.push(ProgramItem::Storage(s.clone())),
                ContainerItem::World(fields) => {
                    if self.storage_defaults.iter().any(|(n, _)| n == "world") {
                        self.container_error(format!("{} declares `world` twice", c.name), c.span);
                        continue;
                    }
                    let mut m = serde_json::Map::new();
                    for f in fields {
                        if m.contains_key(&f.name) {
                            self.container_error(format!("{}: world field `{}` is declared twice", c.name, f.name), f.span);
                            continue;
                        }
                        let spec = crate::types::TypeSpec::from_expr(&f.ty, &self.types);
                        let v = match &f.default {
                            Some(d) => {
                                self.check_literal(&spec, d, &format!("world field `{}`", f.name));
                                match self.const_json(d) {
                                    Ok(v) => v,
                                    Err(why) => {
                                        self.container_error(format!(
                                            "{}: world field `{}`: its default {}; set it in on_start instead",
                                            c.name, f.name, why), f.span);
                                        continue;
                                    }
                                }
                            }
                            None => empty_of(&spec, &self.types, 0).unwrap_or(J::Null),
                        };
                        m.insert(f.name.clone(), v);
                        if f.immutable {
                            self.immutable.push(("world".into(), Some(f.name.clone())));
                        }
                    }
                    self.storage_defaults.push(("world".into(), J::Object(m)));
                }
                ContainerItem::Permissions(stmts) => {
                    for p in stmts {
                        self.container_permission(&c.name, p, &mut meta);
                    }
                }
                ContainerItem::Safety(fields) => safety.extend(fields.iter().cloned()),
                ContainerItem::Function(f) => body.push(if f.sig.name == "constructor" {
                    ProgramItem::Constructor(f.clone())
                } else {
                    ProgramItem::Function(f.clone())
                }),
                ContainerItem::Programs(_) | ContainerItem::Struct(_) => {}
                ContainerItem::Io(io) => not_run(&mut self.strict_errors,
                    "an `io` block (its inputs and outputs are declared; nothing routes them)", io.span.line),
                ContainerItem::Agents(a) => not_run(&mut self.strict_errors,
                    "an `agents` block (the VM has no agent runtime)", a.first().map_or(c.span.line, |b| b.span.line)),
                ContainerItem::Modalities(_) => not_run(&mut self.strict_errors,
                    "a `modalities` block (the VM has no encoders)", c.span.line),
                ContainerItem::Sensors(_) => not_run(&mut self.strict_errors,
                    "a `sensors` block (the VM has no hardware)", c.span.line),
                ContainerItem::Actuators(_) => not_run(&mut self.strict_errors,
                    "an `actuators` block (the VM has no hardware)", c.span.line),
                ContainerItem::Fusion(_) => not_run(&mut self.strict_errors,
                    "a `fusion` block (the VM has no encoders)", c.span.line),
            }
        }
        self.container_safety(&c.name, &safety, &mut meta.safety);
        for item in &body {
            if let ProgramItem::Storage(s) = item {
                for f in &s.fields {
                    if matches!(f.name.as_str(), "config" | "world" | "programs") {
                        self.container_error(format!(
                            "{}: `{}` is the container's own; name the storage field something else", c.name, f.name), f.span);
                    }
                }
            }
        }
        let decl = ProgramDecl { name: c.name.clone(), implements: c.implements.clone(), body, span: c.span };
        self.compile_program_as(&decl, Some(meta))
    }

    /// `grant b.f to c;` / `revoke b.f from c;`: `b` is a binding (or a
    /// program of the file) and must have `f`; `c`, when it is a binding,
    /// is the program it names (the caller the VM sees).
    fn container_permission(&mut self, container: &str, p: &PermissionStmt, meta: &mut ContainerMeta) {
        let verb = if matches!(p.action, PermAction::Grant) { "grant" } else { "revoke" };
        let Some((b, f)) = p.subject.split_once('.') else {
            self.container_error(format!(
                "{}: `{} {}` names no function; write `{} binding.function ...`", container, verb, p.subject, verb), p.span);
            return;
        };
        let program = match self.bindings.get(b) {
            Some(prog) => prog.clone(),
            None if self.units.contains_key(b) => b.to_string(),
            None => {
                self.container_error(format!(
                    "{}: `{} {}`: `{}` is neither a binding of {} nor a program of this file",
                    container, verb, p.subject, b, container), p.span);
                return;
            }
        };
        match self.units.get(&program).and_then(|fs| fs.iter().find(|s| s.name == f)).cloned() {
            None => {
                self.container_error(format!("{}: `{} {}`: {} has no function {}", container, verb, p.subject, program, f), p.span);
                return;
            }
            Some(sig) if contract_of(&sig).0.access == Access::System => {
                self.container_error(format!(
                    "{}: `{} {}`: {}.{} is @system, which no grant opens", container, verb, p.subject, program, f), p.span);
                return;
            }
            Some(_) => {}
        }
        let target = self.bindings.get(&p.target).cloned().unwrap_or_else(|| p.target.clone());
        meta.permissions.push((program, f.to_string(), target, verb == "grant"));
    }

    /// A container's `safety { }`: the limits the VM enforces, and nothing it
    /// cannot (a safety setting that is not enforced is worse than none).
    fn container_safety(&mut self, container: &str, fields: &[ConfigField], s: &mut SafetyLimits) {
        let mut seen = std::collections::HashSet::new();
        for f in fields {
            if !seen.insert(f.name.clone()) {
                self.container_error(format!("{}: safety `{}` is set twice", container, f.name), f.span);
                continue;
            }
            match f.name.as_str() {
                "max_jumps" | "max_ops" | "max_queries" | "max_wall_ms" | "max_call_depth" => {
                    let n = match &f.value {
                        Expr::IntLit(n) if *n > 0 => *n as u64,
                        _ => {
                            self.container_error(format!(
                                "{}: safety `{}` must be a positive integer (leave it out for no limit of its own)",
                                container, f.name), f.span);
                            continue;
                        }
                    };
                    match f.name.as_str() {
                        "max_jumps" => s.max_jumps = n,
                        "max_ops" => s.max_ops = n,
                        "max_queries" => s.max_queries = n,
                        "max_wall_ms" => s.max_wall_ms = n,
                        _ => s.max_call_depth = n.min(u32::MAX as u64) as u32,
                    }
                }
                "deny" => {
                    let Expr::ArrayLit(names) = &f.value else {
                        self.container_error(format!(
                            "{}: safety `deny` takes an array of opcode names (`deny: [\"ask\", \"query\"]`)", container), f.span);
                        continue;
                    };
                    for n in names {
                        match n {
                            Expr::StringLit(m) if asm_mnemonic_to_op(&m.to_uppercase()).is_some() => {
                                let m = m.to_uppercase();
                                if !s.deny.contains(&m) {
                                    s.deny.push(m);
                                }
                            }
                            Expr::StringLit(m) => self.container_error(format!(
                                "{}: safety `deny` names `{}`, which is no opcode", container, m), f.span),
                            _ => self.container_error(format!(
                                "{}: safety `deny` takes opcode names as strings", container), f.span),
                        }
                    }
                }
                other => self.container_error(format!(
                    "{}: safety `{}` is not a limit this VM can enforce (it enforces max_jumps, max_ops, \
                     max_queries, max_wall_ms, max_call_depth and deny); a safety setting that is not \
                     enforced is worse than none", container, other), f.span),
            }
        }
    }

    /// A container's `deny` list against the code its runs can reach: its
    /// own functions, its bound programs', and every program those call by
    /// name, transitively. (The VM refuses a denied opcode at run time too:
    /// this is the early, readable error.)
    fn check_denied(&mut self, programs: &[CompiledProgram]) {
        let mut names: HashMap<crate::vm::key::Key, String> = HashMap::new();
        for p in programs {
            for s in &p.symbol_names {
                names.insert(name_hash(s), s.clone());
            }
        }
        for c in programs {
            let Some(meta) = &c.container else { continue };
            if meta.safety.deny.is_empty() {
                continue;
            }
            let denied: Vec<(u8, &str)> = meta.safety.deny.iter()
                .filter_map(|m| asm_mnemonic_to_op(m).map(|o| (o, m.as_str())))
                .collect();
            let mut todo: Vec<String> = vec![c.name.clone()];
            todo.extend(meta.programs.iter().map(|(_, p)| p.clone()));
            let mut done = std::collections::HashSet::new();
            while let Some(name) = todo.pop() {
                if !done.insert(name.clone()) {
                    continue;
                }
                let Some(p) = programs.iter().find(|p| p.name == name) else { continue };
                for f in &p.functions {
                    for (opcode, first) in crate::vm::engine::instructions(&f.bytecode) {
                        if let Some((_, m)) = denied.iter().find(|(o, _)| *o == opcode) {
                            self.container_error(format!(
                                "{} denies {}, but {}.{} uses it", c.name, m, p.name, f.name),
                                Span { line: 0, col: 0, offset: 0 });
                        }
                        if opcode == op::CALL {
                            if let Some(target) = first.and_then(|h| names.get(&h))
                                .and_then(|n| n.split_once('.')).map(|(prog, _)| prog.to_string()) {
                                todo.push(target);
                            }
                        }
                    }
                }
            }
        }
    }

    fn compile_function(&mut self, func: &FunctionDecl) -> CompiledFunction {
        self.bytecode.clear();
        self.labels.clear();
        self.debug_lines.clear();
        self.current_function = func.sig.name.clone();
        // DRIFT.md §C11: don't let an operand-error span from a PREVIOUS
        // function leak into this one's error messages.
        self.current_span = None;

        // Task 3: a postfix `override` is only meaningful -- and only
        // compiles cleanly -- if the name it claims is genuinely overridable
        // in a module this SourceFile `use`s. Check it before compiling the
        // body: the function still compiles either way (so a single source
        // file with multiple violations reports all of them, matching how
        // `strict_errors` accumulates), but an invalid `override` always
        // surfaces as a compile error from the free-function entry points.
        //
        // Fix round 1: a PLAIN (non-`override`) function gets the opposite
        // check -- does its name collide with a SEALED capability the
        // program can never reach (registry-wins, no override escape)? An
        // overridable collision is left alone (registry-wins is correct
        // there; `override` is the discoverable fix).
        if func.sig.is_override {
            self.check_override_validity(&func.sig);
        } else {
            self.check_sealed_collision(&func.sig);
        }

        // VM step 0: an attribute the VM cannot honour is a strict error,
        // not a decoration.
        if self.strict {
            self.check_permissions(&func.sig);
        }

        // 2026-10-24: the declared types, before the body, so a `return` of a
        // literal is checked against them. Under strict (the default), a type
        // name nothing declares is an error: it would check nothing at run time.
        let param_types: Vec<crate::types::TypeSpec> = func.sig.params.iter()
            .map(|p| crate::types::TypeSpec::from_expr(&p.ty, &self.types))
            .collect();
        let return_type = crate::types::TypeSpec::from_expr(&func.sig.return_type, &self.types);
        if self.strict {
            let mut unknown = Vec::new();
            for t in param_types.iter().chain(std::iter::once(&return_type)) {
                t.opaque_names(&mut unknown);
            }
            unknown.sort();
            unknown.dedup();
            for name in unknown {
                self.strict_errors.push(format!(
                    "strict: unknown type `{}` in {}::{} -- declare a struct, enum or `type` alias, or use a \
                     built-in type (it would check nothing at run time)",
                    name, self.current_program, self.current_function));
            }
        }
        self.current_return = return_type.clone();

        // P6-1: a name read as a value that nothing declares is a compile error under strict: it would read a
        // register nothing wrote (null), and a null used to compare equal to any string.
        if self.strict {
            let mut known = self.known_names.clone();
            for p in &func.sig.permissions {
                if let Permission::Inputs(names) = p {
                    known.extend(names.iter().cloned());
                }
            }
            let registry = ModuleRegistry::new();
            let used = self.used_modules.clone();
            let own: std::collections::HashSet<String> = self.signatures.keys().cloned().collect();
            let symbolic = |n: &str| !own.contains(n) && registry.resolve(&used, n).is_some();
            for name in crate::names::undeclared(func, &known, &symbolic) {
                self.strict_errors.push(format!(
                    "strict: `{}` is read in {}::{} but never declared (UndeclaredName) -- declare it with `let`, \
                     `create` or `@inputs(...)`, or check its spelling",
                    name, self.current_program, self.current_function));
            }
        }

        for stmt in &func.body {
            self.compile_stmt(stmt);
        }

        // 2026-10-24: the whole contract, enforced by the VM. An attribute
        // that cannot mean anything (`@ratelimit(0, ...)`, `@cron("fast")`,
        // `private` beside `@external`) is a compile error in every mode.
        let (contract, contract_errors) = contract_of(&func.sig);
        for message in contract_errors {
            self.capability_errors.push(ParseError { message, span: func.sig.span });
        }
        CompiledFunction {
            name: func.sig.name.clone(),
            params: func.sig.params.iter().map(|p| p.name.clone()).collect(),
            bytecode: self.bytecode.clone(),
            labels: self.labels.clone(),
            debug_lines: self.debug_lines.clone(),
            is_override: func.sig.is_override,
            access: contract.access,
            once: contract.once,
            restricted: contract.restricted,
            ratelimit: contract.ratelimit,
            cron: contract.cron,
            param_types,
            return_type,
        }
    }

    /// VM step 0 (2026-09-29): the permission attributes the VM does not
    /// enforce are strict violations. `@external`/`@internal`/`@system` are
    /// enforced at the host's entry (`VM::call_entry`) and `@once` in the VM,
    /// so they pass; `@hook`, `@before`, `@after` and `@cron` promise an
    /// event bus, middleware and a scheduler the VM does not have, and a
    /// program that compiles with them advertises behaviour it will not get.
    /// Advertised-and-unenforced is worse than absent.
    ///
    /// 2026-10-24: `@cron` (the VM's scheduler, `VM::tick`), `@restricted`
    /// and `@ratelimit` are enforced now and pass; the three left here are
    /// the ones still without a mechanism.
    fn check_permissions(&mut self, sig: &FunctionSig) {
        for p in &sig.permissions {
            let what = match p {
                Permission::Hook(t) => format!("@hook({})", t),
                Permission::Before(f) => format!("@before({})", f),
                Permission::After(f) => format!("@after({})", f),
                Permission::Cron(_) | Permission::Restricted(_) | Permission::Ratelimit(..)
                | Permission::External | Permission::Internal
                | Permission::System | Permission::Once | Permission::Inputs(_) => continue,
            };
            self.strict_violation(
                &format!("attribute {} on function {} (parsed, not enforced by the VM)", what, sig.name),
                Some(sig.span.line));
        }
    }

    /// Task 3: `function name() override { ... }` is only valid when `name`
    /// is exposed by SOME module in `self.used_modules` and marked
    /// `overridable` there. Anything else -- the name absent from every
    /// `use`'d module, present but sealed (`overridable: false`), or no
    /// `use` at all -- is a compile error, pushed onto `self.capability_errors`
    /// (checked regardless of `--strict`; see that field's doc).
    fn check_override_validity(&mut self, sig: &FunctionSig) {
        let registry = ModuleRegistry::new();
        let valid = self.used_modules.iter()
            .any(|m| registry.get(m, &sig.name).is_some_and(|f| f.overridable));
        if !valid {
            let used = if self.used_modules.is_empty() {
                "(none)".to_string()
            } else {
                self.used_modules.join(", ")
            };
            self.capability_errors.push(ParseError {
                message: format!(
                    "function `{}` is declared `override`, but no `use`'d module exposes an \
                     overridable `{}` (used modules: {}; registry v{})",
                    sig.name, sig.name, used, registry.version,
                ),
                span: sig.span,
            });
        }
    }

    /// Fix round 1 (Task 3 review): a PLAIN (no `override` marker) function
    /// whose name collides with a SEALED (`overridable: false`) export of a
    /// `use`'d module used to compile silently and just... never run --
    /// `op::CALL`'s registry-wins-without-a-valid-override branch always
    /// picks the registry impl, and a sealed name has no `override` escape
    /// (declaring one is ALREADY a compile error, via
    /// `check_override_validity`). That silent dead code is a compile error
    /// instead: there is no way to reach a program function shadowed by a
    /// sealed capability other than renaming it, so say so at compile time.
    ///
    /// Deliberately narrower than it might look: an OVERRIDABLE collision
    /// (the module exposes `sig.name` with `overridable: true`) is NOT an
    /// error here -- registry-wins is the correct, intended behaviour there,
    /// and `override` is the discoverable way to reclaim the name. Only a
    /// SEALED collision has no escape hatch at all, which is what makes it
    /// worth failing loudly over.
    ///
    /// Uses `ModuleRegistry::resolve_named` rather than scanning
    /// `used_modules` by hand, specifically so this asks the EXACT question
    /// `op::CALL` will ask at runtime (first `use`'d module that exposes
    /// `sig.name` at all, in `used_modules` order -- not "does any used
    /// module have it sealed"). With today's single `demo` module those
    /// questions have the same answer; they would not once a second module
    /// could also expose `sig.name`.
    fn check_sealed_collision(&mut self, sig: &FunctionSig) {
        let registry = ModuleRegistry::new();
        let Some((module, entry)) = registry.resolve_named(&self.used_modules, &sig.name) else {
            return;
        };
        if entry.overridable {
            return;
        }
        self.capability_errors.push(ParseError {
            message: format!(
                "function `{}` collides with sealed capability `{}` from use'd module \
                 `{}`; rename it (sealed capabilities cannot be overridden or shadowed)",
                sig.name, sig.name, module,
            ),
            span: sig.span,
        });
    }

    // ── Standard interfaces (Task 4 + Task 7): required + validated
    // `implements` ────────────────────────────────────────────────────────
    //
    // `implements X` used to be pure decoration -- parsed into
    // `ProgramDecl::implements`, round-tripped through `.cubebin`, never
    // checked against anything. Task 4 made it real: `X` must resolve to
    // either a VM-internal interface (`vm::interfaces::InterfaceRegistry`)
    // or an in-file inline `interface X { ... }` decl, and the program must
    // provide a same-name/same-arity function for every required member of
    // whichever one it resolved to -- but an EMPTY (or absent) `implements`
    // list was deliberately left untouched, so the migration to a
    // conforming repo could land in its own task rather than needing every
    // program fixed in the same commit that built the checker.
    //
    // Task 7 is that migration: every program in this repo (examples/, the
    // embedded `.cube` fixtures across tests/*.rs, and this file's own unit
    // tests) was brought to a real, validated `implements` first, and ONLY
    // THEN does this method start rejecting an empty one below -- reversing
    // that order would have made every one of those fixtures a compile
    // error the moment this landed, instead of a deliberate, reviewable
    // migration.

    /// For every name in `prog.implements`: resolve it, then confirm every
    /// required member is satisfied. Pushes onto `self.interface_errors`
    /// (checked unconditionally -- see that field's doc), exactly parallel
    /// to how `check_override_validity`/`check_sealed_collision` push onto
    /// `capability_errors`.
    ///
    /// **Resolution order: the VM interface registry FIRST, an in-file
    /// inline `interface` decl only as a FALLBACK for a name the registry
    /// doesn't recognize -- never the reverse, and never "whichever is
    /// found first wins."** This is the one real design decision in this
    /// method, so spelling out why: the entire point of Task 4 is that
    /// `ISolve`/`ISolver` become VM-internal (Rust source, not `.cube`
    /// source) so they're tamper-proof. If a program's OWN inline
    /// `interface ISolver { ... }` block were allowed to win when its name
    /// collides with a registry interface, a program could declare a
    /// weaker local `ISolver` (say, `solve` only, dropping `parse`/`verify`)
    /// and validate against that instead -- silently defeating the whole
    /// mechanism. So a name the registry knows ALWAYS means the registry's
    /// contract; an in-file decl is only ever consulted for a name the
    /// registry does NOT recognize (e.g. `SpinningWidget`'s own `IWidget` in
    /// tests/interfaces.rs), which is exactly the "existing programs that
    /// declare inline interfaces MUST keep working" case the brief calls
    /// out. This ordering is unobservable on every program in examples/
    /// today -- the six inline `ISolver` decls this project's examples used
    /// to carry were identical in shape to the registry's `ISolver`, and
    /// Task 7 deleted them as redundant now that `implements` is required --
    /// it only matters once a program tries to shadow a registry name with a
    /// weaker local one; `a_local_interface_block_cannot_redefine_a_registry_interface_to_something_weaker`
    /// (tests/interfaces.rs) pins this down directly.
    ///
    /// Task 7: an EMPTY `implements` is now itself a member of
    /// `interface_errors` below, not an early return -- every `program` must
    /// declare at least one interface. This is checked before resolution
    /// (there is nothing to resolve), but goes through the exact same
    /// accumulator as a resolution/member failure, so it is checked
    /// unconditionally (not just under `--strict`) and folds into the same
    /// combined error surface as everything else in this method.
    fn check_implements(&mut self, prog: &ProgramDecl) {
        if prog.implements.is_empty() {
            self.interface_errors.push(ParseError {
                message: format!(
                    "program `{}` has no `implements` clause -- every program must \
                     declare at least one interface (e.g. `implements ISolve`); \
                     see src/vm/interfaces.rs for the full VM interface registry",
                    prog.name,
                ),
                span: prog.span,
            });
            return;
        }

        let registry = InterfaceRegistry::new();
        for iface_name in &prog.implements {
            let required: Vec<(String, usize)> = if let Some(members) = registry.get(iface_name) {
                members.iter().map(|m| (m.name.clone(), m.arity)).collect()
            } else if let Some(decl) = self.inline_interfaces.get(iface_name) {
                // Fallback path: an in-file `interface` decl's own abstract,
                // non-optional members are the required set. `type Input;`-
                // style members carry no name/arity and aren't functions at
                // all, so they're skipped (`filter_map` drops the `None`s).
                decl.members.iter().filter_map(|m| match m {
                    InterfaceMember::Function(sig)
                        if sig.modifiers.contains(&Modifier::Abstract)
                            && !sig.modifiers.contains(&Modifier::Optional) =>
                    {
                        Some((sig.name.clone(), sig.params.len()))
                    }
                    _ => None,
                }).collect()
            } else {
                self.interface_errors.push(ParseError {
                    message: format!(
                        "program `{}` declares `implements {}`, but `{}` is neither a \
                         VM interface nor an in-file `interface` declaration",
                        prog.name, iface_name, iface_name,
                    ),
                    span: prog.span,
                });
                continue;
            };

            for (member_name, member_arity) in &required {
                let provided = prog.body.iter().any(|item| {
                    let sig = match item {
                        ProgramItem::Function(f) | ProgramItem::Constructor(f) => &f.sig,
                        _ => return false,
                    };
                    &sig.name == member_name && sig.params.len() == *member_arity
                });
                if !provided {
                    self.interface_errors.push(ParseError {
                        message: format!(
                            "program `{}` declares `implements {}`, but does not provide a \
                             matching `{}` function ({} parameter{}) required by that interface",
                            prog.name, iface_name, member_name, member_arity,
                            if *member_arity == 1 { "" } else { "s" },
                        ),
                        span: prog.span,
                    });
                }
            }
        }
    }

    // ── Strict-mode safety check (P0-1, deny-by-default whitelist) ──────
    //
    // `--strict` is supposed to mean "verified" -- but a statement this
    // function doesn't recognize used to fall through the trailing `_ => {}`
    // and pass silently, even when it compiled to zero real bytecode. Live
    // repro: `frobnicate d;` (not a real construct) compiled clean and ran.
    //
    // Fixed to deny-by-default: this match is now EXHAUSTIVE over `Stmt`
    // (no trailing wildcard), so a future AST variant forces an explicit
    // decision here instead of silently defaulting to "OK". A statement is
    // valid under strict iff it is a recognized construct that compiles to
    // >=1 real opcode, or is an explicitly-whitelisted no-op (commit /
    // rollback). Everything else is a hard error naming the construct
    // (+ source line when the statement's span carries one).
    //
    // Compound / control-flow statements (Let/If/For/While/Return/Atomic/
    // Emit/TryCatch) always emit their own real scaffolding bytecode
    // (CREATE/COMPARE/COND/LABEL/JMP/RETURN/STORE/...) regardless of what's
    // nested inside them -- so no additional check is needed at THIS level;
    // `compile_stmt` recurses into their bodies via `compile_stmt` again,
    // which re-invokes this function on every nested statement.
    fn strict_check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            // ── Known non-executing constructs (unchanged from before) ──
            // `match` was on this blocklist because every arm's body ran
            // unconditionally (DRIFT.md C3). `compile_match` now emits a
            // real COMPARE/COND chain, so the construct executes and is no
            // longer a strict violation. Arm bodies are strict-checked by
            // the ordinary recursion (`compile_match` -> `compile_stmt` ->
            // `strict_check_stmt`), same as if/while/try bodies.
            Stmt::Match(_) => {}
            Stmt::Opcode(OpcodeStmt::Extended { op: ext, .. }) => {
                if is_trace_only_ext_op(*ext) {
                    self.strict_violation(&format!("opcode {} (trace-only)", ext_name(*ext)), None);
                }
            }
            Stmt::Opcode(OpcodeStmt::Unify { .. }) => {
                self.strict_violation("opcode unify (trace-only)", None);
            }
            // Every other OpcodeStmt (Create/Assign/Add/Sub/Mul/Div/Sum/
            // Push/Pop/Query/Remember/Store/Recall/Bind/BindRole/Transfer/
            // Compare/Extended-non-trace-only) always emits a real opcode in
            // compile_opcode -- nothing further to check.
            Stmt::Opcode(_) => {}

            // ── Real constructs: always compile to real bytecode ────────
            // Task 6: `asm` always emits >=1 real opcode for every
            // instruction it contains -- an unrecognized mnemonic is a
            // compile ERROR (`asm_errors`, checked unconditionally, same as
            // `capability_errors`/`interface_errors`) rather than a silent
            // no-op, so by the time this runs on a program that compiled at
            // all, every instruction here is real bytecode. No separate
            // strict-mode check is needed on top of that.
            Stmt::Let(_) | Stmt::If(_) | Stmt::For(_) | Stmt::While(_)
            | Stmt::Return(_) | Stmt::Atomic(_) | Stmt::Emit(_) | Stmt::Asm(_) => {}
            // try/catch/finally bodies are all real (LABEL/JMP scaffolding,
            // and -- DRIFT.md §C8 fix -- `finally_body`'s statements are now
            // compiled onto the shared try/catch convergence point too);
            // nested statements in all three are independently re-checked
            // via recursion (`compile_stmt` calls itself on each), so no
            // separate check is needed at this level.
            Stmt::TryCatch(_) => {}

            // ── Explicitly-whitelisted no-ops ────────────────────────────
            // commit/rollback deliberately compile to a single SKIP -- they
            // are VM-level markers, not opcodes with their own semantics.
            Stmt::Commit | Stmt::Rollback => {}

            // ── Sub-classified: these wrap an inner Expr, and only compile
            // to real bytecode for specific shapes of it ───────────────
            Stmt::Assert(a) => {
                // compile_expr_discard only emits real bytecode for a
                // Call/MethodCall condition; any other shape (comparison,
                // bare ident, ...) -- i.e. essentially every realistic
                // assert today -- silently compiles to a single SKIP.
                if !matches!(a.condition, Expr::Call(..) | Expr::MethodCall(..)) {
                    self.strict_violation(
                        "assert (condition does not compile to an executable check under the \
                         current compiler -- only a call/method-call condition does)",
                        Some(a.span.line));
                }
            }
            // DRIFT.md §C4 fix (2026-09-14). `compile_assign` now handles a
            // bare identifier, `self.field`, and a single index into either
            // (`name[key] = v` -> SET_INDEX). Every OTHER target shape --
            // nested field, nested index, call result -- is a hard
            // `operand_errors` failure in EVERY mode rather than a
            // `--strict`-only warning, for the same reason as C11: a
            // statement that cannot be encoded must not silently vanish, and
            // a mode flag is not an acceptable gate for a silent drop. So
            // there is nothing left for strict mode to add here.
            Stmt::Assign(_) => {}
            Stmt::Expr(e) => {
                // compile_expr_discard only emits real bytecode (CALL+POP)
                // for a Call/MethodCall; any other expression used as a
                // bare statement (an unrecognized leading identifier, a
                // literal, a comparison, ...) silently compiles to a single
                // SKIP. This is the `frobnicate d;` hole: an unrecognized
                // construct parses as an Ident expression-statement and
                // never surfaces an error anywhere. (Stmt::Expr carries no
                // span in the current AST, so no line number here.)
                if !matches!(e, Expr::Call(..) | Expr::MethodCall(..)) {
                    self.strict_violation(
                        &format!("unrecognized or no-op statement (expression `{:?}` \
                         has no compiled effect)", e),
                        None);
                }
            }

            // ── Parsed but never compiled at all: compile_stmt's own
            // catch-all (`_ => emit SKIP`) silently no-ops these today.
            // None of them execute anything in the VM yet (import's real
            // loader, and any future asm/vsa hatch, are separately-scoped,
            // not-yet-built work -- see the cubelang-foundation design doc).
            Stmt::Throw(_) => {
                self.strict_violation(
                    "throw (not compiled -- falls through to a no-op under the current compiler)",
                    None);
            }
        }
    }

    // ── Statement compilation ───────────────────────────────────────────

    fn compile_stmt(&mut self, stmt: &Stmt) {
        // DRIFT.md §C11: track the nearest enclosing statement span so an
        // operand-encoding error deep inside (e.g. a `let`/`assign` RHS, a
        // call argument) can name a source line. Not every `Stmt` variant
        // carries a `Span` (see `stmt_span`); when one doesn't, the
        // previously-seen span is left in place rather than cleared, since
        // "the nearest known line" is more useful than nothing.
        if let Some(span) = stmt_span(stmt) {
            self.current_span = Some(span);
        }
        if self.strict {
            self.strict_check_stmt(stmt);
        }
        // 2026-10-24: immutable storage is written only by the constructor.
        self.check_writes(stmt);
        // 2026-10-24: a call into another program becomes a CALL of
        // `Program.function`.
        let routed = self.route_calls(stmt);
        let stmt = routed.as_ref().unwrap_or(stmt);
        // 2026-10-24: struct, map and array literals in operand positions are
        // built into temporaries first (this compiler has no other lowering
        // pass; see C11).
        let hoisted = self.hoist_literals(stmt);
        let stmt = hoisted.as_ref().unwrap_or(stmt);
        match stmt {
            Stmt::Opcode(op) => self.compile_opcode(op),
            Stmt::Let(l) => self.compile_let(l),
            Stmt::If(i) => self.compile_if(i),
            Stmt::For(f) => self.compile_for(f),
            Stmt::While(w) => self.compile_while(w),
            Stmt::Match(m) => self.compile_match(m),
            Stmt::Return(value) => {
                // Plan-1: real early return. Bare `return;` → 0 operands.
                // `return <expr>;` → 1 operand resolved at runtime.
                //
                // Task 3: `return foo(args);` needs the SAME special case
                // `compile_let` already gives `let x = foo(args);` --
                // `Expr::Call` has no operand encoding (CALL always
                // delivers its result on the STACK, never as something
                // `compile_expr_operand` can express), so without this it
                // fell through that function's opaque `_ => None` arm and
                // silently returned Int(0) WITHOUT EVER CALLING `foo`. Emit
                // the CALL, POP its stack result into a reserved register,
                // then RETURN that register -- required for `use`'d-module
                // resolution (Task 3) to be observable via `return greet();`
                // at all, but not specific to it: any `return call(...)`.
                match value {
                    Some(Expr::Call(callee, args)) => {
                        self.emit_call(callee, args);
                        self.emit_debug(op::POP, "pop __return_call");
                        self.emit_byte(op::POP);
                        self.emit_byte(1);
                        self.emit_named("__return_call");
                        self.emit_debug(op::RETURN, "return value");
                        self.emit_byte(op::RETURN);
                        self.emit_byte(1);
                        self.emit_named("__return_call");
                    }
                    Some(expr) => {
                        let spec = self.current_return.clone();
                        let what = format!("the return value of {}::{}", self.current_program, self.current_function);
                        self.check_literal(&spec, expr, &what);
                        self.emit_debug(op::RETURN, "return value");
                        self.emit_byte(op::RETURN);
                        self.emit_byte(1);
                        self.compile_expr_operand(expr);
                    }
                    None => {
                        self.emit_debug(op::RETURN, "return");
                        self.emit_byte(op::RETURN);
                        self.emit_byte(0);
                    }
                }
            }
            Stmt::Atomic(a) => {
                // Atomic block: snapshot ctx before, rollback on error
                self.emit_debug(op::LABEL, "atomic_start");
                let label = self.fresh_label("atomic");
                self.emit_label(&label);
                for s in &a.body {
                    self.compile_stmt(s);
                }
                self.emit_debug(op::LABEL, "atomic_end");
            }
            Stmt::Commit => {
                self.emit_debug(op::SKIP, "commit");
                self.emit_byte(op::SKIP); // commit is a VM-level operation
            }
            Stmt::Rollback => {
                self.emit_debug(op::SKIP, "rollback");
                self.emit_byte(op::SKIP);
            }
            Stmt::Assert(a) => {
                // Assert compiles to: evaluate condition, COND to error
                self.emit_debug(op::COMPARE, &format!("assert"));
                self.compile_expr_discard(&a.condition);
            }
            Stmt::Emit(e) => {
                // Emit event: compile as STORE with event name
                self.emit_debug(op::STORE, &format!("emit {}", e.event_name));
                self.emit_byte(op::STORE);
                self.emit_byte(2); // 2 operands
                self.emit_named("__event__");
                self.emit_global(&e.event_name);
            }
            Stmt::TryCatch(tc) => {
                // DRIFT.md §C8 fix: `finally_body` used to be parsed and
                // then never compiled at all, in any mode -- the one
                // construct on the non-executing list that even `--strict`
                // couldn't catch (per-statement checks can't see a body
                // `compile_stmt` never visits).
                //
                // The normal path is `try_body; JMP <join>` (skip the catch
                // body entirely); the caught path is `LABEL catch; catch_body`
                // falling through into the same `<join>` point. A `finally`
                // block must run on BOTH paths, which means it has to be
                // compiled AT that join point, not after it -- putting it
                // between the join label and a separate trailing label (an
                // earlier version of this fix did exactly that) would let
                // the normal path's JMP skip over the finally body too,
                // since the JMP lands on the join label and everything
                // after it -- catch body AND finally body alike -- runs
                // only via fall-through. So when there IS a finally block,
                // the join label IS the finally label: the JMP targets the
                // spot right before the finally body, not right after it.
                let catch_label = self.fresh_label("catch");
                let join_label = if tc.finally_body.is_some() {
                    self.fresh_label("finally")
                } else {
                    self.fresh_label("try_end")
                };
                self.emit_debug(op::LABEL, "try_start");
                for s in &tc.try_body {
                    self.compile_stmt(s);
                }
                self.emit_jmp(&join_label);
                self.emit_label(&catch_label);
                for s in &tc.catch_body {
                    self.compile_stmt(s);
                }
                self.emit_label(&join_label);
                if let Some(finally_body) = &tc.finally_body {
                    for s in finally_body {
                        self.compile_stmt(s);
                    }
                }
            }
            Stmt::Assign(a) => self.compile_assign(a),
            Stmt::Expr(e) => {
                self.compile_expr_discard(e);
            }
            Stmt::Asm(block) => self.compile_asm_block(block),
            _ => {
                // Other statements: emit SKIP as placeholder
                self.emit_byte(op::SKIP);
            }
        }
    }

    /// Task 6: compile an `asm { ... }` block. Each instruction maps its
    /// mnemonic to an op byte (`asm_mnemonic_to_op`) and each operand to a
    /// tagged byte pair via the SAME emitters every other surface construct
    /// uses (`emit_named`/`emit_role`/`emit_global`/`emit_imm`/
    /// `emit_float_imm`) -- producing exactly the layout the hand-assembled
    /// `engine.rs` UNBIND test builds by hand, but from real source text.
    /// An unrecognized mnemonic pushes onto `asm_errors` (checked
    /// unconditionally by `compile_ast`/`compile_ast_strict`) and is
    /// skipped rather than emitted as malformed bytecode.
    fn compile_asm_block(&mut self, block: &AsmBlock) {
        for instr in &block.instrs {
            let Some(opcode) = asm_mnemonic_to_op(&instr.mnemonic) else {
                self.asm_errors.push(ParseError {
                    message: format!(
                        "asm: unknown mnemonic `{}` -- not a name in the op table \
                         (check spelling and case; mnemonics are the exact `op::` \
                         constant names, e.g. BIND_ROLE, UNBIND, CREATE)",
                        instr.mnemonic,
                    ),
                    span: instr.span,
                });
                continue;
            };
            self.emit_debug(opcode, &format!(
                "asm {} ({} operand(s))", instr.mnemonic, instr.operands.len()));
            self.emit_byte(opcode);
            self.emit_count(instr.operands.len(), "an asm instruction's operands");
            for operand in &instr.operands {
                match operand {
                    AsmOperand::Named(name) => self.emit_named(name),
                    // Roles need an explicit `record_symbol` the same way
                    // `OpcodeStmt::Bind`'s own compile arm gives its role
                    // argument -- `emit_role` (unlike `emit_global`) doesn't
                    // record on its own, so without this the VM's name
                    // table would have no entry and BIND_ROLE/UNBIND would
                    // fall back to an opaque `role_xxyy` token instead of
                    // the real role string.
                    AsmOperand::Role(name) => {
                        self.record_symbol(name);
                        self.emit_role(name);
                    }
                    AsmOperand::Global(s) => self.emit_global(s),
                    AsmOperand::Imm(n) => self.emit_imm(*n),
                    AsmOperand::FloatImm(f) => self.emit_float_imm(*f),
                }
            }
        }
    }

    // ── Opcode compilation (direct 1:1 mapping) ─────────────────────────

    fn compile_opcode(&mut self, op_stmt: &OpcodeStmt) {
        match op_stmt {
            OpcodeStmt::Create { reg, ty } => {
                self.emit_debug(op::CREATE, &format!("create {} : {}", reg, ty));
                self.emit_byte(op::CREATE);
                self.emit_byte(2); // 2 operands
                self.emit_named(reg);
                // `map<str, number>` / `array<str>` create containers: the VM
                // knows them by the bare name, as `type_name` spells them.
                let base = ty.split('<').next().unwrap_or(ty).trim();
                if base == "map" || base == "array" {
                    self.emit_type(base);
                } else {
                    self.emit_type(ty);
                }
            }
            OpcodeStmt::Assign { reg, value } => {
                self.emit_debug(op::ASSIGN, &format!("assign {} = ...", reg));
                self.emit_byte(op::ASSIGN);
                self.emit_byte(2);
                self.emit_named(reg);
                self.compile_expr_operand(value);
            }
            OpcodeStmt::Add { reg, value } => {
                self.emit_debug(op::ADD, &format!("add {}, ...", reg));
                self.emit_byte(op::ADD);
                self.emit_byte(2);
                self.emit_named(reg);
                self.compile_expr_operand(value);
            }
            OpcodeStmt::Sub { reg, value } => {
                self.emit_debug(op::SUB, &format!("sub {}, ...", reg));
                self.emit_byte(op::SUB);
                self.emit_byte(2);
                self.emit_named(reg);
                self.compile_expr_operand(value);
            }
            OpcodeStmt::Mul { reg, value } => {
                self.emit_debug(op::MUL, &format!("mul {}, ...", reg));
                self.emit_byte(op::MUL);
                self.emit_byte(2);
                self.emit_named(reg);
                self.compile_expr_operand(value);
            }
            OpcodeStmt::Div { reg, value } => {
                self.emit_debug(op::DIV, &format!("div {}, ...", reg));
                self.emit_byte(op::DIV);
                self.emit_byte(2);
                self.emit_named(reg);
                self.compile_expr_operand(value);
            }
            OpcodeStmt::Sum { reg } => {
                self.emit_debug(op::SUM, &format!("sum {}", reg));
                self.emit_byte(op::SUM);
                self.emit_byte(1);
                self.emit_named(reg);
            }
            OpcodeStmt::Push { reg } => {
                self.emit_debug(op::PUSH, &format!("push {}", reg));
                self.emit_byte(op::PUSH);
                self.emit_byte(1);
                self.emit_named(reg);
            }
            OpcodeStmt::Pop { reg } => {
                self.emit_debug(op::POP, &format!("pop {}", reg));
                self.emit_byte(op::POP);
                self.emit_byte(1);
                self.emit_named(reg);
            }
            OpcodeStmt::Query { reg } => {
                self.emit_debug(op::QUERY, &format!("query {}", reg));
                self.emit_byte(op::QUERY);
                self.emit_byte(1);
                self.emit_named(reg);
            }
            OpcodeStmt::Remember { reg } => {
                self.emit_debug(op::REMEMBER, &format!("remember {}", reg));
                self.emit_byte(op::REMEMBER);
                self.emit_byte(1);
                self.emit_named(reg);
            }
            OpcodeStmt::Store { reg, key } => {
                self.emit_debug(op::STORE, &format!("store {}, ...", reg));
                self.emit_byte(op::STORE);
                self.emit_byte(2);
                self.emit_named(reg);
                self.compile_expr_operand(key);
            }
            OpcodeStmt::Recall { reg, key } => {
                self.emit_debug(op::RECALL, "recall ...");
                self.emit_byte(op::RECALL);
                match reg {
                    Some(r) => {
                        self.emit_byte(2);
                        self.emit_named(r);
                        self.compile_expr_operand(key);
                    }
                    None => {
                        self.emit_byte(1);
                        self.compile_expr_operand(key);
                    }
                }
            }
            OpcodeStmt::Bind { reg, role, val } => {
                self.emit_debug(op::BIND_ROLE, &format!("bind {} {} ...", reg, role));
                // Record role + filler names so the VM can reverse the hashes
                // for genuine VSA binding. The filler is a string literal or an
                // identifier; other expr forms have no stable symbol to record.
                self.record_symbol(role);
                match val {
                    Expr::StringLit(s) => self.record_symbol(s),
                    Expr::Ident(n) => self.record_symbol(n),
                    _ => {}
                }
                self.emit_byte(op::BIND_ROLE);
                self.emit_byte(3);
                self.emit_named(reg);
                self.emit_role(role);
                self.compile_expr_operand(val);
            }
            OpcodeStmt::Unify { a, b } => {
                self.emit_debug(op::UNIFY, &format!("unify {} {}", a, b));
                self.emit_byte(op::UNIFY);
                self.emit_byte(2);
                self.emit_named(a);
                self.emit_named(b);
            }
            OpcodeStmt::BindRole { reg, role } => {
                // bind_role reg, ROLE : 2-arg surface form. Filler defaults to
                // the register itself (self-binding the register into the role).
                self.emit_debug(op::BIND_ROLE, &format!("bind_role {} {}", reg, role));
                self.record_symbol(role);
                self.record_symbol(reg);
                self.emit_byte(op::BIND_ROLE);
                self.emit_byte(3);
                self.emit_named(reg);
                self.emit_role(role);
                self.emit_named(reg);
            }
            OpcodeStmt::Transfer { src, dst, amount } => {
                self.emit_debug(op::TRANSFER, &format!("transfer {} {} ...", src, dst));
                self.emit_byte(op::TRANSFER);
                self.emit_byte(3);
                self.emit_named(src);
                self.emit_named(dst);
                self.compile_expr_operand(amount);
            }
            OpcodeStmt::Compare { a, b } => {
                self.emit_debug(op::COMPARE, &format!("compare {} {}", a, b));
                self.emit_byte(op::COMPARE);
                self.emit_byte(2);
                self.emit_named(a);
                self.emit_named(b);
            }
            OpcodeStmt::Extended { op: ext, args } => {
                let code = ext_bytecode(*ext);
                self.emit_debug(code, &format!("{:?} ({} args)", ext, args.len()));
                self.emit_byte(code);
                self.emit_count(args.len(), "an extended opcode's arguments");
                for arg in args {
                    match arg {
                        ExtArg::Reg(name) => self.emit_named(name),
                        ExtArg::Val(expr) => self.compile_expr_operand(expr),
                    }
                }
            }
        }
    }

    // ── Control flow compilation ────────────────────────────────────────

    fn compile_if(&mut self, if_stmt: &IfStmt) {
        let else_label = self.fresh_label("else");
        let end_label = self.fresh_label("endif");

        // Evaluate condition → sets the VM comparison flag.
        self.compile_condition(&if_stmt.condition);
        // COND: jump to else_label if the flag is FALSE (skip the then-body).
        self.emit_cond_jump(&else_label);

        // Then body
        for s in &if_stmt.then_body {
            self.compile_stmt(s);
        }
        if if_stmt.else_body.is_some() {
            self.emit_jmp(&end_label);
        }

        // Else body
        self.emit_label(&else_label);
        if let Some(else_body) = &if_stmt.else_body {
            for s in else_body {
                self.compile_stmt(s);
            }
        }
        self.emit_label(&end_label);
    }

    fn compile_for(&mut self, for_stmt: &ForStmt) {
        // Plan-4. Lower `for (let x of arr) { body }` to an index-based while:
        //
        //   ASSIGN __i = 0
        //   LEN __n, arr_reg
        //   LABEL loop_top
        //   COMPARE __i, __n, kind=2(lt) ; COND -> loop_end
        //   INDEX <binding>, arr_reg, __i
        //   <body>
        //   ADD __i, 1
        //   JMP loop_top
        //   LABEL loop_end
        //
        // The iteration source must be available as a register: if it's already
        // an Ident we name it directly; otherwise we materialise it into a
        // fresh temp (handles `for (let x of [1,2,3])` and similar) via a
        // recursive compile_let.
        let n_id = { self.label_counter += 1; self.label_counter };
        let i_var   = format!("__for_i_{}",   n_id);
        let n_var   = format!("__for_n_{}",   n_id);
        let arr_var = format!("__for_arr_{}", n_id);
        let loop_top = self.fresh_label("for_top");
        let end_label = self.fresh_label("for_end");

        let arr_name: String = match &for_stmt.iter {
            Expr::Ident(n) => n.clone(),
            other => {
                // Materialise into __for_arr_<n> via the same lowering used
                // for any other `let`. Picks up ArrayLit -> MAKE_ARRAY.
                let tmp_let = LetStmt {
                    name: arr_var.clone(),
                    ty: None,
                    value: other.clone(),
                    span: for_stmt.span,
                };
                self.compile_let(&tmp_let);
                arr_var.clone()
            }
        };

        // __i = 0
        self.emit_byte(op::CREATE);  self.emit_byte(2);
        self.emit_named(&i_var);     self.emit_type("auto");
        self.emit_byte(op::ASSIGN);  self.emit_byte(2);
        self.emit_named(&i_var);     self.emit_imm(0);

        // LEN __n, arr_name
        self.emit_debug(op::LEN, &format!("for: len {}", arr_name));
        self.emit_byte(op::LEN);     self.emit_byte(2);
        self.emit_named(&n_var);     self.emit_named(&arr_name);

        // loop_top:
        self.emit_label(&loop_top);

        // COMPARE __i, __n, kind=2(lt) ; COND -> end
        self.emit_debug(op::COMPARE, "for: i < n");
        self.emit_byte(op::COMPARE); self.emit_byte(3);
        self.emit_named(&i_var);     self.emit_named(&n_var);
        self.emit_imm(2);
        self.emit_cond_jump(&end_label);

        // INDEX <binding>, arr_name, __i
        self.emit_byte(op::CREATE);  self.emit_byte(2);
        self.emit_named(&for_stmt.binding); self.emit_type("auto");
        self.emit_debug(op::INDEX, &format!("for: {} = {}[i]", for_stmt.binding, arr_name));
        self.emit_byte(op::INDEX);   self.emit_byte(3);
        self.emit_named(&for_stmt.binding);
        self.emit_named(&arr_name);
        self.emit_named(&i_var);

        // body
        for s in &for_stmt.body {
            self.compile_stmt(s);
        }

        // ADD __i, 1
        self.emit_byte(op::ADD);     self.emit_byte(2);
        self.emit_named(&i_var);     self.emit_imm(1);

        // JMP loop_top ; LABEL loop_end
        self.emit_jmp(&loop_top);
        self.emit_label(&end_label);
    }

    fn compile_while(&mut self, while_stmt: &WhileStmt) {
        let loop_label = self.fresh_label("while");
        let end_label = self.fresh_label("endwhile");

        // loop_top:
        self.emit_label(&loop_label);
        // Evaluate condition → flag; COND jumps to end_label if false.
        self.compile_condition(&while_stmt.condition);
        self.emit_cond_jump(&end_label);

        // body
        for s in &while_stmt.body {
            self.compile_stmt(s);
        }

        // jump back to re-test the condition
        self.emit_jmp(&loop_label);
        self.emit_label(&end_label);
    }

    /// Real arm selection (DRIFT.md §C3 / §A7 fix, 2026-09-14).
    ///
    /// This used to compile every arm's body unconditionally, in source
    /// order, with no comparison and no jumps at all -- the `end_label` was
    /// emitted but nothing ever targeted it. It looked exactly like working
    /// branching logic and produced a plausible, wrong answer instead of an
    /// error, which DRIFT.md called the single most dangerous entry on the
    /// non-executing list.
    ///
    /// `match (subject) { p1 => b1; p2 => b2; _ => bN; }` now lowers to the
    /// same COMPARE/COND primitive `compile_if` already uses -- an if/else-if
    /// chain, which is all a match over scalar patterns is:
    ///
    /// ```text
    ///     COMPARE subject, p1, kind=0(eq)
    ///     COND  -> _arm_2          ; COND jumps when the flag is FALSE
    ///     <b1>
    ///     JMP   -> _endmatch
    ///   LABEL _arm_2
    ///     COMPARE subject, p2, kind=0
    ///     COND  -> _arm_3
    ///     <b2>
    ///     JMP   -> _endmatch
    ///   LABEL _arm_3
    ///     <bN>                     ; wildcard: no test, always taken
    ///   LABEL _endmatch
    /// ```
    ///
    /// Two things this depends on, both of which had to be true first:
    /// - `op::COMPARE` compares strings AS strings (fixed the same day in
    ///   `src/vm/engine.rs`). Before that every `Value::Str` resolved to 0,
    ///   so every string arm would have matched and the first arm would
    ///   always have won -- swapping one silent wrong answer for another.
    /// - The subject and every pattern must be operand-encodable (C11).
    ///   A subject like `match (op[0])` is now a compile error, not silent
    ///   garbage; write `index o, op, 0; match (o) { ... }`.
    ///
    /// `_` is the wildcard/default arm. It takes no test, and arms written
    /// after it are unreachable, so emission stops there rather than
    /// emitting dead bytecode.
    fn compile_match(&mut self, match_stmt: &MatchStmt) {
        let end_label = self.fresh_label("endmatch");

        for arm in &match_stmt.arms {
            if is_wildcard_pattern(&arm.pattern) {
                // Default arm: reached only by falling through every
                // preceding arm's COND, so it needs no test of its own.
                for s in &arm.body {
                    self.compile_stmt(s);
                }
                // Anything after `_` can never run. Don't emit it.
                break;
            }

            let next_label = self.fresh_label("arm");

            // COMPARE subject, pattern, kind 0 (eq) -- same layout as
            // `compile_condition`'s comparison path.
            self.emit_debug(op::COMPARE, "match arm ==");
            self.emit_byte(op::COMPARE);
            self.emit_byte(3);
            self.compile_expr_operand(&match_stmt.expr);
            self.compile_expr_operand(&arm.pattern);
            self.emit_imm(0);
            // COND jumps when the flag is FALSE, i.e. on a non-match, so it
            // targets the NEXT arm's test.
            self.emit_cond_jump(&next_label);

            for s in &arm.body {
                self.compile_stmt(s);
            }
            // Leave the match once an arm has run: without this the body
            // would fall straight into the next arm's COMPARE and the
            // construct would silently behave like the run-every-arm bug
            // this replaces. Emitted unconditionally -- on the final arm it
            // targets the label immediately following, which is harmless.
            self.emit_jmp(&end_label);
            self.emit_label(&next_label);
        }

        self.emit_label(&end_label);
    }

    /// Assignment statements. Two DRIFT.md fixes landed here on 2026-09-14;
    /// before them this was a bare if/else-if over two target shapes that
    /// emitted nothing at all for anything else, and ignored the operator.
    ///
    /// **§C4 — index-assignment targets emitted ZERO bytecode.** `arr[0] = x`,
    /// `regs[reg] = v`, `self.patterns[k] = h` all fell off the end of the
    /// chain silently. It is the mechanism behind `examples/gsm8k.cube`
    /// returning a meaningless 0, and the most common single violation across
    /// the complex examples. They now emit `SET_INDEX [container, key, value]`.
    ///
    /// **Compound assignment was compiled as plain assignment.** `a.op` was
    /// parsed into the AST and then never read, so `total += 5` emitted
    /// `ASSIGN total, 5` -- `total = 10; total += 5;` returned **5**, not 15,
    /// with no error. `+=`/`-=`/`*=`/`/=` now emit ADD/SUB/MUL/DIV, which
    /// already do a read-modify-write on the register.
    fn compile_assign(&mut self, a: &AssignStmt) {
        // The register (or self-field) name the operator applies to, for the
        // two simple target shapes. `self.field` and a bare identifier are
        // the same thing at the bytecode level: a named register.
        let simple_target: Option<String> = match &a.target {
            Expr::SelfAccess(field) => Some(field.clone()),
            Expr::Ident(name) => Some(name.clone()),
            _ => None,
        };

        if let Some(name) = simple_target {
            let opcode = match a.op {
                AssignOp::Eq => op::ASSIGN,
                AssignOp::PlusEq => op::ADD,
                AssignOp::MinusEq => op::SUB,
                AssignOp::StarEq => op::MUL,
                AssignOp::SlashEq => op::DIV,
            };
            self.emit_debug(opcode, &format!("{} {:?} ...", name, a.op));
            self.emit_byte(opcode);
            self.emit_byte(2);
            self.emit_named(&name);
            self.compile_expr_operand(&a.value);
            return;
        }

        // Index target: `container[key] = value`.
        if let Expr::Index(base, key) = &a.target {
            // The container has to name a register. `a[i][j] = v` (a nested
            // index) and `f()[0] = v` do not, and there is no temporary to
            // hold the intermediate container -- see C11 on why this compiler
            // has no lowering pass. Fall through to the error below rather
            // than emitting something that looks right.
            let container = match base.as_ref() {
                Expr::Ident(name) => Some(name.clone()),
                Expr::SelfAccess(field) => Some(field.clone()),
                _ => None,
            };
            if let Some(container) = container {
                if !matches!(a.op, AssignOp::Eq) {
                    // `m[k] += 1` would need a read-modify-write through a
                    // temporary the compiler cannot allocate. Refuse loudly.
                    self.operand_errors.push(ParseError {
                        message: format!(
                            "compound assignment `{:?}` to an index target (`{}[...]`) is not \
                             supported -- read the element into a register, apply the operator \
                             there, then assign it back",
                            a.op, container),
                        span: a.span,
                    });
                    return;
                }
                self.emit_debug(op::SET_INDEX, &format!("{}[...] = ...", container));
                self.emit_byte(op::SET_INDEX);
                self.emit_byte(3);
                self.emit_named(&container);
                self.compile_expr_operand(key);
                self.compile_expr_operand(&a.value);
                return;
            }
        }

        // Field target (2026-10-24): `p.x = v` / `self.p.x = v` writes the
        // field of the struct (a map) held in `p`. A nested `a.b.c = v` would
        // need the inner value read out, changed and written back: refused
        // below, as before.
        if let Expr::Field(base, field) = &a.target {
            let container = match base.as_ref() {
                Expr::Ident(name) | Expr::SelfAccess(name) => Some(name.clone()),
                _ => None,
            };
            if let Some(container) = container {
                if !matches!(a.op, AssignOp::Eq) {
                    self.operand_errors.push(ParseError {
                        message: format!(
                            "compound assignment `{:?}` to a field (`{}.{}`) is not supported -- read the \
                             field into a register, apply the operator there, then assign it back",
                            a.op, container, field),
                        span: a.span,
                    });
                    return;
                }
                self.emit_debug(op::SET_INDEX, &format!("{}.{} = ...", container, field));
                self.emit_byte(op::SET_INDEX);
                self.emit_byte(3);
                self.emit_named(&container);
                self.emit_global(field);
                self.compile_expr_operand(&a.value);
                return;
            }
        }

        // Anything else: a nested field (`a.b.c = v`), a nested index, a call
        // result. These used to compile to nothing at all. They are now a
        // hard error in every mode, for the same reason as C11 -- a statement
        // that cannot be encoded must not silently disappear.
        self.operand_errors.push(ParseError {
            message: format!(
                "assignment target `{:?}` cannot be encoded -- only a bare identifier, \
                 `self.field`, and a single index into either (`name[key]`) are supported",
                a.target),
            span: a.span,
        });
    }

    fn compile_let(&mut self, let_stmt: &LetStmt) {
        // 2026-10-24: `let p = Point { x: 1 };` and `let m = { k: v };` build
        // the value straight into the variable.
        match &let_stmt.value {
            Expr::StructLit(name, fields) => {
                if let Some(ty) = &let_stmt.ty {
                    let declared = crate::types::TypeSpec::from_expr(ty, &self.types);
                    let lit = crate::types::TypeSpec::Named(name.clone());
                    if !declared.accepts_literal(&lit, &self.types) {
                        self.literal_error(format!(
                            "`let {}: {}` is given a {} literal", let_stmt.name, declared.spell(), name));
                    }
                }
                self.build_struct(&let_stmt.name, name, fields);
                return;
            }
            Expr::MapLit(pairs) => {
                self.build_map(&let_stmt.name, pairs);
                return;
            }
            _ => {}
        }
        if let Some(ty) = &let_stmt.ty {
            let spec = crate::types::TypeSpec::from_expr(ty, &self.types);
            self.check_literal(&spec, &let_stmt.value, &format!("`let {}`", let_stmt.name));
        }
        // let x = expr → CREATE x + ASSIGN x = expr
        self.emit_debug(op::CREATE, &format!("let {}", let_stmt.name));
        self.emit_byte(op::CREATE);
        self.emit_byte(2);
        self.emit_named(&let_stmt.name);
        if let Some(ty) = &let_stmt.ty {
            self.emit_type(&type_name(ty));
        } else {
            self.emit_type("auto");
        }

        // Plan-2: `let x = foo(args)` lowers to CALL + POP x so the callee's
        // return value (stack-delivered) lands in x. Other RHS forms keep the
        // existing ASSIGN path.
        if let Expr::Call(callee, args) = &let_stmt.value {
            self.emit_call(callee, args);
            self.emit_debug(op::POP, &format!("pop {}", let_stmt.name));
            self.emit_byte(op::POP);
            self.emit_byte(1);
            self.emit_named(&let_stmt.name);
            return;
        }

        // Plan-3 array RHS lowerings.
        match &let_stmt.value {
            // let xs = [a, b, c] → MAKE_ARRAY xs, n, a, b, c
            Expr::ArrayLit(elems) => {
                self.emit_debug(op::MAKE_ARRAY,
                    &format!("make_array {} (n={})", let_stmt.name, elems.len()));
                self.emit_byte(op::MAKE_ARRAY);
                self.emit_count(elems.len() + 2, "a list literal");
                self.emit_named(&let_stmt.name);
                self.emit_imm(elems.len() as i64);
                for e in elems { self.compile_expr_operand(e); }
                return;
            }
            // let n = arr.len() → LEN n, arr (special-case the built-in;
            // any other method falls through to the CALL path above).
            Expr::MethodCall(obj, method, args) if method == "len" && args.is_empty() => {
                if let Expr::Ident(arr) = obj.as_ref() {
                    self.emit_debug(op::LEN, &format!("len {} = {}.len()", let_stmt.name, arr));
                    self.emit_byte(op::LEN);
                    self.emit_byte(2);
                    self.emit_named(&let_stmt.name);
                    self.emit_named(arr);
                    return;
                }
            }
            // let x = arr[i] → INDEX x, arr, i
            //
            // DRIFT.md §C4 (2026-09-14): `self.field[key]` reads here too.
            // This arm used to accept only `Expr::Ident` as the container, so
            // `let got = self.patterns["k"];` fell through to the operand
            // encoder and failed -- asymmetric with `compile_assign`, which
            // accepts `self.field[key]` as a WRITE target. At the bytecode
            // level a storage field and a local are both just named
            // registers, so the same operand shape works for both.
            Expr::Index(arr_expr, idx_expr) => {
                let container = match arr_expr.as_ref() {
                    Expr::Ident(name) => Some(name.clone()),
                    Expr::SelfAccess(field) => Some(field.clone()),
                    _ => None,
                };
                if let Some(arr) = container {
                    self.emit_debug(op::INDEX,
                        &format!("index {} = {}[...]", let_stmt.name, arr));
                    self.emit_byte(op::INDEX);
                    self.emit_byte(3);
                    self.emit_named(&let_stmt.name);
                    self.emit_named(&arr);
                    self.compile_expr_operand(idx_expr);
                    return;
                }
            }
            _ => {}
        }

        self.emit_byte(op::ASSIGN);
        self.emit_byte(2);
        self.emit_named(&let_stmt.name);
        self.compile_expr_operand(&let_stmt.value);
    }

    /// Plan-2: emit `CALL <fn_name>, arg0, ..., argN` with all args resolved
    /// at call time. Method calls fall back to `CALL <method>, <receiver>` —
    /// the receiver becomes arg0; full method dispatch is a later refinement.
    fn emit_call(&mut self, callee: &Expr, args: &[Expr]) {
        // 2026-10-24: a literal argument to one of the program's own functions
        // is checked against the parameter's declared type here; anything else
        // is checked by the VM as the call is made.
        if let Expr::Ident(name) = callee {
            if let Some(specs) = self.signatures.get(name).cloned() {
                for (i, (spec, arg)) in specs.iter().zip(args).enumerate() {
                    self.check_literal(spec, arg, &format!("argument {} of {}()", i + 1, name));
                }
            }
        }
        match callee {
            Expr::Ident(name) => {
                self.emit_debug(op::CALL, &format!("call {}({})", name, args.len()));
                self.emit_byte(op::CALL);
                self.emit_count(args.len() + 1, "a call's arguments");
                self.emit_global(name);
                for a in args { self.compile_expr_operand(a); }
            }
            _ => {
                self.emit_debug(op::CALL, "call <expr>");
                self.emit_byte(op::CALL);
                self.emit_count(args.len() + 1, "a call's arguments");
                self.emit_byte(OP_NONE);
                self.emit_byte(0x00);
                self.emit_byte(0x00);
                for a in args { self.compile_expr_operand(a); }
            }
        }
    }

    // ── Expression compilation (to operand bytes) ───────────────────────

    fn compile_expr_operand(&mut self, expr: &Expr) {
        match expr {
            Expr::IntLit(n) => self.emit_imm(*n),
            Expr::FloatLit(f) => self.emit_float_imm(*f),
            Expr::StringLit(s) => self.emit_global(s),
            Expr::BoolLit(b) => self.emit_imm(if *b { 1 } else { 0 }),
            Expr::Null => self.emit_byte(OP_NULL),
            Expr::Ident(name) => self.emit_named(name),
            Expr::SelfAccess(field) => self.emit_named(field),
            // 2026-10-24: a real field read. Until now `p.x` compiled to a
            // register literally named "p.x", which nothing could ever write
            // (a field assignment was a compile error), so every field read
            // silently produced 0; and `f().x` read a register named "x".
            Expr::Field(..) => match field_path(expr) {
                Some((base, path)) if path.iter().all(|k| k.len() <= 255) && path.len() <= 255 => {
                    self.emit_field(&base, &path);
                }
                _ => {
                    self.operand_errors.push(ParseError {
                        message: format!(
                            "a field read needs a variable at its base (`p.x`, `self.p.x`, `a.b.c`){}; \
                             put the value in a variable first",
                            if self.current_function.is_empty() { String::new() }
                            else { format!(" in {}::{}", self.current_program, self.current_function) }),
                        span: self.current_span.unwrap_or(Span { line: 0, col: 0, offset: 0 }),
                    });
                    self.emit_byte(OP_NONE);
                    self.emit_byte(0x00);
                    self.emit_byte(0x00);
                }
            },
            _ => {
                // DRIFT.md §C11: this used to silently emit an opaque
                // OP_NONE placeholder here -- e.g. `double_it(x - y)`
                // compiled clean (even under `--strict`, which only checks
                // statement SHAPES and never descends into an individual
                // operand) and ran as `double_it(<garbage>)`, returning 0
                // instead of 4. That is now a hard compile error in EVERY
                // mode, not just `--strict`: this is the one silent-garbage
                // path strict mode could never catch, so a mode flag is not
                // an acceptable gate for it. Reached from call arguments
                // (`emit_call`), array-literal elements, index expressions,
                // and any `let`/`assign` RHS that isn't one of the
                // specially-handled shapes above.
                let form = expr_form_name(expr);
                let message = format!(
                    "expression form {} cannot be encoded as an operand{}; compute it into a \
                     register first with add/sub/mul/div, then pass the register",
                    form,
                    if self.current_program.is_empty() && self.current_function.is_empty() {
                        String::new()
                    } else {
                        format!(" in {}::{}", self.current_program, self.current_function)
                    },
                );
                self.operand_errors.push(ParseError {
                    message,
                    span: self.current_span.unwrap_or(Span { line: 0, col: 0, offset: 0 }),
                });
                // Still emit a well-formed (if meaningless) operand so the
                // surrounding bytecode's operand count stays consistent --
                // harmless, since `compile_ast`/`compile_ast_strict` never
                // hand back bytecode when `operand_errors` is non-empty.
                self.emit_byte(OP_NONE);
                self.emit_byte(0x00);
                self.emit_byte(0x00);
            }
        }
    }

    /// Compile an expression for its side effects (discard result).
    fn compile_expr_discard(&mut self, expr: &Expr) {
        match expr {
            Expr::Call(callee, args) => {
                // Plan-2: emit CALL with args, then POP into __discard so the
                // callee's return value (always pushed by CALL) doesn't leak
                // onto the caller's value stack across the next statement.
                self.emit_call(callee, args);
                self.emit_byte(op::POP);
                self.emit_byte(1);
                self.emit_named("__call_discard");
            }
            Expr::MethodCall(_obj, method, args) => {
                // v1: method becomes the fn name; receiver becomes arg0.
                // (Full method dispatch is a later refinement.)
                self.emit_debug(op::CALL, &format!(".{}()", method));
                self.emit_byte(op::CALL);
                self.emit_count(args.len() + 1, "a call's arguments");
                self.emit_global(method);
                for a in args { self.compile_expr_operand(a); }
                self.emit_byte(op::POP);
                self.emit_byte(1);
                self.emit_named("__call_discard");
            }
            _ => {
                // Side-effect-free expression — emit SKIP
                self.emit_byte(op::SKIP);
            }
        }
    }

    // ── Byte emission helpers ───────────────────────────────────────────

    fn emit_byte(&mut self, b: u8) {
        self.bytecode.push(b);
    }

    /// An instruction's operand count, one byte (P6-8, 2026-11-13). A count over 255 used to wrap or saturate, so
    /// the operands written no longer matched the count read: a list literal of 254+ items ran as nothing, one of
    /// 255 numbers misread its last operands. Now it is a compile error naming the limit (`TooLong`); a list
    /// literal holds at most `MAX_OPERANDS - 2` items (its register and its length take two operands).
    fn emit_count(&mut self, n: usize, what: &str) {
        if n > MAX_OPERANDS {
            let items = if what == "a list literal" { format!("{} items; at most {}", n - 2, MAX_OPERANDS - 2) }
                        else { format!("{} operands; at most {}", n, MAX_OPERANDS) };
            self.operand_errors.push(ParseError {
                message: format!("TooLong: {} has {} fit one instruction{}", what, items,
                    if self.current_function.is_empty() { String::new() }
                    else { format!(" (in {}::{})", self.current_program, self.current_function) }),
                span: self.current_span.unwrap_or(Span { line: 0, col: 0, offset: 0 }),
            });
            self.emit_byte(MAX_OPERANDS as u8);
            return;
        }
        self.emit_byte(n as u8);
    }

    fn emit_named(&mut self, name: &str) {
        self.emit_byte(OP_NAMED8);
        self.emit_key(name);
    }

    /// A name's 8-byte key (P6-9), checked against every other spelling this compilation keyed: two spellings with
    /// one key would read as one, so that is a compile error naming both (`KeyCollision`).
    fn emit_key(&mut self, name: &str) {
        let key = name_hash(name);
        match self.keyed.get(&key) {
            Some(other) if other != name => {
                let msg = format!("KeyCollision: `{}` and `{}` share the key {}; rename one", other, name, key.hex());
                self.operand_errors.push(ParseError {
                    message: msg,
                    span: self.current_span.unwrap_or(Span { line: 0, col: 0, offset: 0 }),
                });
            }
            Some(_) => {}
            None => { self.keyed.insert(key, name.to_string()); }
        }
        for b in key.as_slice().to_vec() {
            self.emit_byte(b);
        }
    }

    /// `base.k1.k2...` as one operand (the caller checked the lengths fit a byte).
    fn emit_field(&mut self, base: &str, path: &[String]) {
        self.emit_byte(OP_FIELD8);
        self.emit_key(base);
        self.emit_byte(path.len() as u8);
        for key in path {
            self.emit_byte(key.len() as u8);
            for b in key.as_bytes() {
                self.emit_byte(*b);
            }
        }
    }

    fn fresh_temp(&mut self, what: &str) -> String {
        self.label_counter += 1;
        format!("__{}_{}", what, self.label_counter)
    }

    fn literal_error(&mut self, message: String) {
        let at = if self.current_function.is_empty() { String::new() }
                 else { format!(" (in {}::{})", self.current_program, self.current_function) };
        self.operand_errors.push(ParseError {
            message: format!("{}{}", message, at),
            span: self.current_span.unwrap_or(Span { line: 0, col: 0, offset: 0 }),
        });
    }

    /// Build a struct literal into the register `dst` (2026-10-24). A struct
    /// value is a map holding every declared field: a field the literal leaves
    /// out takes its declared default; one with no default, a field the struct
    /// does not declare, a field given twice, or a literal of the wrong type is
    /// a compile error.
    fn build_struct(&mut self, dst: &str, name: &str, given: &[(String, Expr)]) {
        let Some(decl) = self.struct_decls.get(name).cloned() else {
            self.literal_error(format!("`{} {{ ... }}`: no struct named {} is declared", name, name));
            return;
        };
        let mut seen = std::collections::HashSet::new();
        for (field, _) in given {
            if !seen.insert(field.as_str()) {
                self.literal_error(format!("`{} {{ ... }}` gives the field `{}` twice", name, field));
            }
            if !decl.fields.iter().any(|f| &f.name == field) {
                self.literal_error(format!(
                    "struct {} has no field `{}` (its fields: {})", name, field,
                    decl.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")));
            }
        }
        let mut values: Vec<(String, Expr)> = Vec::with_capacity(decl.fields.len());
        for f in &decl.fields {
            let value = match given.iter().find(|(n, _)| n == &f.name) {
                Some((_, v)) => v.clone(),
                None => match &f.default {
                    Some(d) => d.clone(),
                    None => {
                        self.literal_error(format!(
                            "`{} {{ ... }}` leaves out `{}`, which has no default", name, f.name));
                        continue;
                    }
                },
            };
            let spec = crate::types::TypeSpec::from_expr(&f.ty, &self.types);
            if let Some(lit) = crate::types::TypeSpec::of_literal(&value) {
                if !spec.accepts_literal(&lit, &self.types) {
                    self.literal_error(format!(
                        "`{}.{}` is declared {} but the literal is {}", name, f.name, spec.spell(), lit.spell()));
                }
            }
            values.push((f.name.clone(), value));
        }
        self.emit_container(dst, "map");
        for (field, value) in values {
            let value = self.materialize(&value);
            self.emit_byte(op::SET_INDEX);
            self.emit_byte(3);
            self.emit_named(dst);
            self.emit_global(&field);
            self.compile_expr_operand(&value);
        }
    }

    /// Build a map literal `{ k: v, ... }` into `dst`.
    fn build_map(&mut self, dst: &str, pairs: &[(Expr, Expr)]) {
        self.emit_container(dst, "map");
        for (k, v) in pairs {
            // a bare word as a key is the key's text, as in `{ name: "x" }`
            let key = match k {
                Expr::Ident(word) => Expr::StringLit(word.clone()),
                other => self.materialize(other),
            };
            let value = self.materialize(v);
            self.emit_byte(op::SET_INDEX);
            self.emit_byte(3);
            self.emit_named(dst);
            self.compile_expr_operand(&key);
            self.compile_expr_operand(&value);
        }
    }

    fn emit_container(&mut self, dst: &str, ty: &str) {
        self.emit_debug(op::CREATE, &format!("create {} : {}", dst, ty));
        self.emit_byte(op::CREATE);
        self.emit_byte(2);
        self.emit_named(dst);
        self.emit_type(ty);
    }

    /// Turn the literals inside an operand expression into registers
    /// (2026-10-24): a struct, map or array literal is built into a fresh
    /// temporary first and the expression refers to the temporary. Calls keep
    /// their shape, with their arguments materialized. Anything else is
    /// returned unchanged.
    fn materialize(&mut self, e: &Expr) -> Expr {
        match e {
            Expr::StructLit(name, fields) => {
                let tmp = self.fresh_temp("struct");
                self.build_struct(&tmp, name, fields);
                Expr::Ident(tmp)
            }
            Expr::MapLit(pairs) => {
                let tmp = self.fresh_temp("map");
                self.build_map(&tmp, pairs);
                Expr::Ident(tmp)
            }
            Expr::ArrayLit(elems) => {
                let tmp = self.fresh_temp("array");
                let elems: Vec<Expr> = elems.iter().map(|x| self.materialize(x)).collect();
                self.emit_byte(op::CREATE);
                self.emit_byte(2);
                self.emit_named(&tmp);
                self.emit_type("auto");
                self.emit_byte(op::MAKE_ARRAY);
                self.emit_count(elems.len() + 2, "a list literal");
                self.emit_named(&tmp);
                self.emit_imm(elems.len() as i64);
                for x in &elems {
                    self.compile_expr_operand(x);
                }
                Expr::Ident(tmp)
            }
            Expr::Call(callee, args) => {
                Expr::Call(callee.clone(), args.iter().map(|a| self.materialize(a)).collect())
            }
            Expr::MethodCall(obj, method, args) => {
                Expr::MethodCall(obj.clone(), method.clone(), args.iter().map(|a| self.materialize(a)).collect())
            }
            other => other.clone(),
        }
    }

    /// Immutable storage (2026-10-24): a field declared `immutable`, one of a
    /// container's `world` declared so, and a container's `config` are set by
    /// the constructor (and a container's `on_start`) and by nothing after.
    /// A local of the same name is the same register, so it counts too.
    fn check_writes(&mut self, stmt: &Stmt) {
        if self.immutable.is_empty()
            || self.current_function == "constructor"
            || (self.in_container && self.current_function == "on_start") {
            return;
        }
        let mut targets: Vec<(String, Option<String>)> = Vec::new();
        match stmt {
            Stmt::Let(l) => targets.push((l.name.clone(), None)),
            Stmt::Assign(a) => {
                let target = match &a.target {
                    Expr::Index(base, _) => base.as_ref(),
                    other => other,
                };
                if let Some((base, path)) = field_path(target) {
                    targets.push((base, path.first().cloned()));
                }
            }
            Stmt::For(f) => targets.push((f.binding.clone(), None)),
            Stmt::Opcode(o) => match o {
                OpcodeStmt::Create { reg, .. } | OpcodeStmt::Assign { reg, .. } | OpcodeStmt::Add { reg, .. }
                | OpcodeStmt::Sub { reg, .. } | OpcodeStmt::Mul { reg, .. } | OpcodeStmt::Div { reg, .. }
                | OpcodeStmt::Pop { reg } | OpcodeStmt::Bind { reg, .. } | OpcodeStmt::BindRole { reg, .. }
                | OpcodeStmt::Recall { reg: Some(reg), .. } => targets.push((reg.clone(), None)),
                OpcodeStmt::Transfer { src, dst, .. } => {
                    targets.push((src.clone(), None));
                    targets.push((dst.clone(), None));
                }
                _ => {}
            },
            // the escape hatch has no per-opcode schema: any register it names
            Stmt::Asm(b) => {
                for i in &b.instrs {
                    for o in &i.operands {
                        if let AsmOperand::Named(n) = o {
                            targets.push((n.clone(), None));
                        }
                    }
                }
            }
            _ => {}
        }
        for (field, sub) in targets {
            let hit = self.immutable.iter()
                .any(|(f, s)| f == &field && (s.is_none() || sub.is_none() || s == &sub));
            if hit {
                let what = match &sub {
                    Some(s) if self.immutable.iter().any(|(f, x)| f == &field && x.as_ref() == Some(s)) => format!("{}.{}", field, s),
                    _ => field.clone(),
                };
                let who = if self.in_container { "the constructor and on_start" } else { "the constructor" };
                self.literal_error(format!("`{}` is immutable: only {} may set it", what, who));
            }
        }
    }

    /// Calls into another program (2026-10-24): `Other.f(x)`, and in a
    /// container `self.programs.b.f(x)` or `b.f(x)` (a binding wins over a
    /// local of the same name), become a CALL of `Other.f`, which the VM runs
    /// as that program and admits with the calling program as the caller.
    /// Checked here: the program has `f`, it takes that many arguments, and
    /// `f` is neither `@system` nor `@cron`.
    fn route_calls(&mut self, stmt: &Stmt) -> Option<Stmt> {
        match stmt {
            Stmt::Let(l) => self.route_call(&l.value).map(|value| Stmt::Let(LetStmt { value, ..l.clone() })),
            Stmt::Return(Some(e)) => self.route_call(e).map(|e| Stmt::Return(Some(e))),
            Stmt::Expr(e) => self.route_call(e).map(Stmt::Expr),
            _ => None,
        }
    }

    fn route_call(&mut self, e: &Expr) -> Option<Expr> {
        let Expr::MethodCall(obj, method, args) = e else { return None };
        let program = match obj.as_ref() {
            Expr::Field(base, b) if matches!(base.as_ref(), Expr::SelfAccess(p) if p == "programs") => {
                match self.bindings.get(b) {
                    Some(p) => p.clone(),
                    None => {
                        self.literal_error(format!("`self.programs.{}`: {} binds no program `{}`", b, self.current_program, b));
                        return None;
                    }
                }
            }
            Expr::Ident(b) if self.bindings.contains_key(b) => self.bindings[b].clone(),
            Expr::Ident(p) if self.units.contains_key(p) => p.clone(),
            _ => return None,
        };
        if program == self.current_program {
            return Some(Expr::Call(Box::new(Expr::Ident(method.clone())), args.clone()));
        }
        let sig = self.units.get(&program).and_then(|fs| fs.iter().find(|s| &s.name == method)).cloned();
        let Some(sig) = sig else {
            self.literal_error(format!("{} has no function {} (called as {}.{})", program, method, program, method));
            return None;
        };
        if sig.params.len() != args.len() {
            self.literal_error(format!("{}.{} takes {} argument(s), but is given {}",
                program, method, sig.params.len(), args.len()));
        }
        let (contract, _) = contract_of(&sig);
        if contract.access == Access::System {
            self.literal_error(format!("{}.{} is @system: only the VM runtime may call it", program, method));
        } else if contract.cron.is_some() {
            self.literal_error(format!("{}.{} is @cron: the VM's scheduler runs it, nothing calls it", program, method));
        }
        for (i, (p, a)) in sig.params.iter().zip(args.iter()).enumerate() {
            let spec = crate::types::TypeSpec::from_expr(&p.ty, &self.types);
            self.check_literal(&spec, a, &format!("argument {} of {}.{}()", i + 1, program, method));
        }
        Some(Expr::Call(Box::new(Expr::Ident(format!("{}.{}", program, method))), args.clone()))
    }

    /// The statement with the literals in its operand positions materialized
    /// (`return Point { x: 1 };`, `f({ k: v })`, `x = [a, b];`), or `None`
    /// when it holds none. A `let` builds a top-level struct or map literal
    /// straight into its own variable (`compile_let`), so only the literals
    /// nested inside it are hoisted here.
    fn hoist_literals(&mut self, stmt: &Stmt) -> Option<Stmt> {
        fn has_literal(e: &Expr) -> bool {
            match e {
                // an array literal is not an operand either: in an operand
                // position it is built into a temporary like the others
                Expr::StructLit(..) | Expr::MapLit(..) | Expr::ArrayLit(..) => true,
                Expr::Call(_, args) | Expr::MethodCall(_, _, args) => args.iter().any(has_literal),
                _ => false,
            }
        }
        match stmt {
            Stmt::Let(l) => {
                let nested = match &l.value {
                    Expr::StructLit(_, fs) => fs.iter().any(|(_, v)| has_literal(v)),
                    Expr::MapLit(ps) => ps.iter().any(|(k, v)| has_literal(k) || has_literal(v)),
                    Expr::ArrayLit(xs) => xs.iter().any(has_literal),
                    other => has_literal(other),
                };
                if !nested {
                    return None;
                }
                let value = match &l.value {
                    // the outer literal stays for compile_let; its parts are built there
                    Expr::StructLit(..) | Expr::MapLit(..) => return None,
                    Expr::ArrayLit(xs) => Expr::ArrayLit(xs.iter().map(|x| self.materialize(x)).collect()),
                    other => self.materialize(other),
                };
                Some(Stmt::Let(LetStmt { value, ..l.clone() }))
            }
            Stmt::Assign(a) if has_literal(&a.value) => {
                let value = self.materialize(&a.value);
                Some(Stmt::Assign(AssignStmt { value, ..a.clone() }))
            }
            Stmt::Return(Some(e)) if has_literal(e) => Some(Stmt::Return(Some(self.materialize(e)))),
            Stmt::Expr(e) if has_literal(e) => Some(Stmt::Expr(self.materialize(e))),
            _ => None,
        }
    }

    fn emit_imm(&mut self, val: i64) {
        self.emit_byte(OP_IMM);
        for b in val.to_le_bytes() {
            self.emit_byte(b);
        }
    }

    fn emit_float_imm(&mut self, val: f64) {
        self.emit_byte(OP_FLOAT);
        for b in val.to_le_bytes() {
            self.emit_byte(b);
        }
    }

    fn emit_type(&mut self, ty: &str) {
        self.emit_byte(OP_TYPE8);
        self.emit_key(ty);
    }

    fn emit_role(&mut self, role: &str) {
        self.emit_byte(OP_ROLE8);
        self.emit_key(role);
    }

    fn emit_global(&mut self, name: &str) {
        // Globals (string literals, mostly) compile to a 2-byte hash. Record the
        // spelling so `VM::load` puts it in the name table and
        // `resolve_symbol_name` can reverse it. Without this an ASK's question
        // text -- the very thing the trunk is supposed to render into language
        // for the user -- comes back as `g_e009`.
        self.record_symbol(name);
        self.emit_byte(OP_GLOBAL8);
        self.emit_key(name);
    }

    fn emit_label(&mut self, name: &str) {
        let offset = self.bytecode.len();
        self.labels.push((name.to_string(), offset));
        self.emit_debug(op::LABEL, name);
        self.emit_byte(op::LABEL);
        self.emit_byte(1);
        self.emit_global(name);
    }

    fn emit_jmp(&mut self, label: &str) {
        self.emit_byte(op::JMP);
        self.emit_byte(1);
        self.emit_global(label);
    }

    /// Emit a conditional jump: COND <label>. At runtime the VM jumps to
    /// `label` if the comparison flag is FALSE (i.e. skip-if-false).
    fn emit_cond_jump(&mut self, label: &str) {
        self.emit_debug(op::COND, &format!("cond -> {}", label));
        self.emit_byte(op::COND);
        self.emit_byte(1);
        self.emit_global(label);
    }

    /// Lower a boolean condition expression so it leaves a result in the VM's
    /// comparison flag. Comparisons (`a > b`, `a == b`, ...) lower to a COMPARE
    /// carrying the two operands plus a comparison-kind immediate. Any other
    /// expression is compared against truthiness (`expr != 0`).
    ///
    /// COMPARE operand layout: [lhs, rhs, kind_imm] where kind ∈
    /// 0=eq 1=ne 2=lt 3=le 4=gt 5=ge 6=truthy.
    fn compile_condition(&mut self, cond: &Expr) {
        if let Expr::BinOp(lhs, bop, rhs) = cond {
            let kind: Option<i64> = match bop {
                BinOp::Eq => Some(0),
                BinOp::NotEq => Some(1),
                BinOp::Lt => Some(2),
                BinOp::LtEq => Some(3),
                BinOp::Gt => Some(4),
                BinOp::GtEq => Some(5),
                _ => None, // And/Or/arithmetic: fall through to truthy below
            };
            if let Some(k) = kind {
                self.emit_debug(op::COMPARE, &format!("cond {:?}", bop));
                self.emit_byte(op::COMPARE);
                self.emit_byte(3);
                self.compile_expr_operand(lhs);
                self.compile_expr_operand(rhs);
                self.emit_imm(k);
                return;
            }
        }
        // Fallback: truthiness test of a single expression (kind = 6).
        // P6 (2026-11-13): the kind goes in the THIRD operand, where COMPARE reads it. It was emitted second, so
        // COMPARE took the 2-operand statement form (equality) and `if (t)` ran as `if (t == 6)`.
        self.emit_debug(op::COMPARE, "cond truthy");
        self.emit_byte(op::COMPARE);
        self.emit_byte(3);
        self.compile_expr_operand(cond);
        self.emit_imm(0);
        self.emit_imm(6);
    }

    fn emit_debug(&mut self, opcode: u8, description: &str) {
        self.debug_lines.push(DebugLine {
            offset: self.bytecode.len(),
            opcode,
            description: description.to_string(),
        });
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        self.label_counter += 1;
        format!("_{}_{}", prefix, self.label_counter)
    }

    /// Record a role/filler symbol name (deduplicated) so the VM can reverse
    /// the 2-byte hash the bytecode carries back to the original string for
    /// genuine VSA codebook lookups.
    fn record_symbol(&mut self, name: &str) {
        if !self.symbol_names.iter().any(|s| s == name) {
            self.symbol_names.push(name.to_string());
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Is this match arm the wildcard/default arm?
///
/// The parser reads an arm's pattern with the ordinary expression parser
/// (`parse_match_stmt`), so `_` arrives as `Expr::Ident("_")` rather than a
/// dedicated pattern node. There is no `Pattern` type in the AST today;
/// when one is added this is the single place that has to learn about it.
fn is_wildcard_pattern(pattern: &Expr) -> bool {
    matches!(pattern, Expr::Ident(name) if name == "_")
}

fn type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Array(_) => "array".into(),
        TypeExpr::Map(_, _) => "map".into(),
        TypeExpr::Void => "void".into(),
        _ => "any".into(),
    }
}

/// Best-effort source span for a statement, used to locate an
/// `operand_errors` finding (DRIFT.md §C11) at the nearest enclosing
/// statement. Not every `Stmt` variant carries a `Span` in the current AST
/// (`Return`, `Throw`, `Opcode`, `Expr`, `Commit`, `Rollback`) -- those
/// return `None` and `compile_stmt` simply keeps whatever span it last saw.
fn stmt_span(stmt: &Stmt) -> Option<Span> {
    match stmt {
        Stmt::Let(l) => Some(l.span),
        Stmt::Assign(a) => Some(a.span),
        Stmt::If(i) => Some(i.span),
        Stmt::For(f) => Some(f.span),
        Stmt::While(w) => Some(w.span),
        Stmt::Match(m) => Some(m.span),
        Stmt::Emit(e) => Some(e.span),
        Stmt::TryCatch(tc) => Some(tc.span),
        Stmt::Assert(a) => Some(a.span),
        Stmt::Atomic(a) => Some(a.span),
        Stmt::Asm(b) => Some(b.span),
        Stmt::Return(_) | Stmt::Throw(_) | Stmt::Opcode(_) | Stmt::Expr(_)
        | Stmt::Commit | Stmt::Rollback => None,
    }
}

/// The value a storage field of a map or array type starts as when it
/// declares none: an empty one (as `create m : map<..>` makes).
fn empty_of(spec: &crate::types::TypeSpec, types: &crate::types::TypeTable, depth: u32) -> Option<serde_json::Value> {
    use crate::types::TypeSpec;
    match spec {
        TypeSpec::Map(..) => Some(serde_json::Value::Object(serde_json::Map::new())),
        TypeSpec::Array(_) => Some(serde_json::Value::Array(Vec::new())),
        TypeSpec::Named(n) if depth < 16 => types.aliases.get(n).and_then(|t| empty_of(t, types, depth + 1)),
        _ => None,
    }
}

/// `p.x` / `self.p.x` / `a.b.c` as (base register, field path); `None` when
/// the base is not a variable (`f().x`, `xs[0].y`).
fn field_path(e: &Expr) -> Option<(String, Vec<String>)> {
    match e {
        Expr::Field(base, f) => {
            let (b, mut path) = field_path(base)?;
            path.push(f.clone());
            Some((b, path))
        }
        Expr::Ident(n) | Expr::SelfAccess(n) => Some((n.clone(), Vec::new())),
        _ => None,
    }
}

/// Human-readable name for an expression form that `compile_expr_operand`
/// cannot encode (DRIFT.md §C11) -- named in the resulting compile error so
/// it's clear exactly what needs to be computed into a register first.
fn expr_form_name(expr: &Expr) -> &'static str {
    match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::StringLit(_) | Expr::BoolLit(_)
        | Expr::Null | Expr::Ident(_) | Expr::SelfAccess(_) | Expr::Field(..) => {
            // Unreachable in practice -- these all have their own arms
            // above and never reach this function -- but named rather than
            // `unreachable!()` so a future refactor that DOES route one of
            // these here fails loudly with a wrong name instead of a panic.
            "expression"
        }
        Expr::Index(..) => "Index",
        Expr::Call(..) => "Call",
        Expr::MethodCall(..) => "MethodCall",
        Expr::BinOp(..) => "BinOp",
        Expr::UnaryOp(..) => "UnaryOp",
        Expr::StructLit(..) => "StructLit",
        Expr::ArrayLit(..) => "ArrayLit",
        Expr::MapLit(..) => "MapLit",
        Expr::Lambda(..) => "Lambda",
        Expr::Await(..) => "Await",
        Expr::Deploy(..) => "Deploy",
        Expr::Proxy(..) => "Proxy",
    }
}

/// True when this extended opcode does NOT execute value-level semantics in
/// the current VM (it only emits a structural trace line). The strict-mode
/// blocklist (P0-1) uses this; it is also the authoritative list for the
/// `validate` subcommand (P0-3). The list mirrors `trace_structural` in
/// `src/vm/engine.rs` — keep in sync as P2 opcodes start executing.
///
/// `ExtOp::Ask` left this list on 2026-08-30: ASK executes — it returns
/// `ExecResult::Suspend` and `VM::resume` continues past it (see
/// `tests/ask_suspend.rs`, `examples/ask_min.cube`), so calling it
/// "non-executing" was stale and made every asking program fail
/// verify-before-execute over `run-proto` and `run --strict`.
pub fn is_trace_only_ext_op(o: ExtOp) -> bool {
    matches!(o,
        ExtOp::Infer | ExtOp::MapRoles | ExtOp::Filter | ExtOp::Score
        | ExtOp::DetectPattern | ExtOp::Decode | ExtOp::Reduce | ExtOp::Merge
        | ExtOp::Split | ExtOp::Debate | ExtOp::Predict | ExtOp::Discover
        | ExtOp::Diff | ExtOp::Seq | ExtOp::Specialize | ExtOp::Reward
        | ExtOp::Match | ExtOp::TemporalBind | ExtOp::Analogy | ExtOp::Gen
        | ExtOp::Inst | ExtOp::Broadcast | ExtOp::Explore | ExtOp::Forge
        | ExtOp::Sync
    )
}

/// Canonical lowercase mnemonic for an extended opcode (matches the surface
/// keyword in source).
pub fn ext_name(o: ExtOp) -> &'static str {
    match o {
        ExtOp::Infer => "infer",            ExtOp::MapRoles => "map_roles",
        ExtOp::Filter => "filter",          ExtOp::Score => "score",
        ExtOp::DetectPattern => "detect_pattern",
        ExtOp::Decode => "decode",          ExtOp::Reduce => "reduce",
        ExtOp::Merge => "merge",            ExtOp::Split => "split",
        ExtOp::Debate => "debate",          ExtOp::Predict => "predict",
        ExtOp::Discover => "discover",      ExtOp::Diff => "diff",
        ExtOp::Seq => "seq",                ExtOp::Specialize => "specialize",
        ExtOp::Reward => "reward",          ExtOp::Match => "match",
        ExtOp::Push => "push",              ExtOp::Sum => "sum",
        ExtOp::Compare => "compare",        ExtOp::TemporalBind => "temporal_bind",
        ExtOp::Analogy => "analogy",        ExtOp::Gen => "gen",
        ExtOp::Inst => "inst",              ExtOp::Broadcast => "broadcast",
        ExtOp::Explore => "explore",        ExtOp::Forge => "forge",
        ExtOp::Ask => "ask",                ExtOp::Sync => "sync",
        ExtOp::Forget => "forget",
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Compile CubeLang source to bytecode programs.
///
/// Task 3: an invalid `override` (a name that isn't genuinely overridable in
/// a `use`'d module), or a plain function name colliding with a `use`'d
/// module's SEALED capability, is a compile error here too, not just under
/// `--strict` -- `check_override_validity`/`check_sealed_collision` always
/// run. Reuses `ParseError`'s shape (message + span) for these rather than
/// widening this function's return type; see `Compiler::capability_errors`'s
/// doc for why.
///
/// Task 4: an `implements X` naming neither a VM interface nor an in-file
/// inline `interface` decl, or missing a required member of whichever it
/// resolves to, is also a compile error here (`check_implements`, always
/// checked -- see `Compiler::interface_errors`'s doc). Checked AFTER
/// `capability_errors` -- an arbitrary but harmless ordering choice, since a
/// single program triggering both in this task's tests is not a case either
/// accumulator needs to interleave with the other.
///
/// Task 6: an `asm { ... }` instruction naming a mnemonic the op table
/// doesn't recognize is a compile error here too (`asm_mnemonic_to_op`,
/// always checked -- see `Compiler::asm_errors`'s doc), folded in after
/// `interface_errors`.
/// Every spelling one compilation of `source` keys -- names, string literals, the compiler's own labels -- sorted
/// (P6-9's cross-check: `cubelang keys`). A parse error is the Err; compile errors do not stop the list.
pub fn keyed_spellings(source: &str) -> Result<Vec<String>, String> {
    let ast = crate::parser::parse(source).map_err(|e| format!("parse error: {}", e))?;
    let mut c = Compiler::new();
    c.compile(&ast);
    let mut v: Vec<String> = c.keyed.values().cloned().collect();
    v.sort();
    Ok(v)
}

pub fn compile(source: &str) -> Result<Vec<CompiledProgram>, crate::parser::ParseError> {
    let ast = crate::parser::parse(source)?;
    compile_ast(&ast)
}

/// Compile an already-parsed `SourceFile` — the AST-input sibling of
/// `compile(source: &str)` above, factored out so Task 5's import loader
/// (`crate::loader`) has somewhere to hand its AST-merged `SourceFile`
/// without round-tripping back through source text (which would either
/// require an unparser that doesn't exist, or re-lexing concatenated text
/// and losing each declaration's OWN file's line numbers — exactly what
/// "AST-level merge, not textual concatenation" is for). `compile(&str)`
/// now just parses and delegates here; behavior is unchanged, this only
/// factors out the part that doesn't actually need source text.
pub fn compile_ast(source: &SourceFile) -> Result<Vec<CompiledProgram>, crate::parser::ParseError> {
    let mut compiler = Compiler::new();
    let programs = compiler.compile(source);
    if let Some(e) = compiler.capability_errors.into_iter().next() {
        return Err(e);
    }
    if let Some(e) = compiler.interface_errors.into_iter().next() {
        return Err(e);
    }
    // Task 6: an unknown `asm` mnemonic, folded in after capability/
    // interface errors -- same "always checked, not just under --strict"
    // treatment as those two.
    if let Some(e) = compiler.asm_errors.into_iter().next() {
        return Err(e);
    }
    // DRIFT.md §C11: a non-encodable expression operand (BinOp, UnaryOp,
    // StructLit, MapLit, Lambda, Await, Deploy, Proxy, a nested Call/
    // MethodCall, Index, ...) used as a call argument, array-literal
    // element, index expression, or `let`/`assign` RHS -- checked
    // unconditionally, same as the three error categories above, since this
    // is the one silent-garbage path `--strict` alone could never catch.
    if let Some(e) = compiler.operand_errors.into_iter().next() {
        return Err(e);
    }
    Ok(programs)
}

/// Compile in strict mode (P0-1): rejects any construct that compiles but does
/// not execute under the safe-subset VM — extended trace-only opcodes,
/// `match`, `unify`, and the other shapes `strict_check_stmt` names. Returns
/// one combined error string listing every violation (named construct +
/// source line when available).
///
/// Fix 4 (docs/DRIFT.md "Bugs found" #5): this comment used to list
/// "intra-program `call`" among the rejected constructs -- stale since
/// Plan-2 made CALL a genuinely executing opcode (see `src/validate.rs:16-17`
/// and this file's own `strict_accepts_call_now_that_it_executes` test).
/// CALL is not rejected here; corrected.
///
/// NOTE: `for` is NOT in this list, despite an earlier version of this
/// comment claiming it was -- `strict_check_stmt`'s `Stmt::For(_)` arm is a
/// deliberate no-op (Task 6: `for` always lowers to real COMPARE/COND/JMP/
/// LABEL loop scaffolding, so there's nothing non-executing about it to
/// reject).
///
/// Task 3: `use`/`override`/registry capability errors are folded into the
/// same combined string (before the strict-mode findings), since they are
/// always checked, not a strict-only concern.
///
/// Task 4: `implements` interface errors are folded in the same way, right
/// after the capability errors and before the strict-mode findings.
pub fn compile_strict(source: &str) -> Result<Vec<CompiledProgram>, String> {
    let ast = crate::parser::parse(source).map_err(|e| format!("parse error: {}", e))?;
    compile_ast_strict(&ast)
}

/// Strict-mode sibling of `compile_ast` above, for the same reason: the
/// import loader produces a `SourceFile`, not source text, and strict
/// validation (P0-1) needs to run over the FULL merged compilation unit —
/// imported code is real code, checked exactly like the entry file's own
/// (see the Task 5 brief: "imported code is verified like any code").
/// `compile_strict(&str)` now just parses and delegates here.
pub fn compile_ast_strict(source: &SourceFile) -> Result<Vec<CompiledProgram>, String> {
    let mut compiler = Compiler::new_strict();
    let programs = compiler.compile(source);
    let mut errors: Vec<String> = compiler.capability_errors.iter().map(|e| e.to_string()).collect();
    errors.extend(compiler.interface_errors.iter().map(|e| e.to_string()));
    // Task 6: same treatment as capability/interface errors -- folded in
    // before the strict-mode findings, always checked.
    errors.extend(compiler.asm_errors.iter().map(|e| e.to_string()));
    // DRIFT.md §C11: same unconditional treatment -- a non-encodable
    // operand is a bug regardless of strict mode, so it's folded in here
    // too, before the strict-only findings.
    errors.extend(compiler.operand_errors.iter().map(|e| e.to_string()));
    errors.extend(compiler.strict_errors);
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(programs)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Task 4 dropped a decorative `implements ISolver` from most fixtures
    // below (bytecode/format/strict-mode concerns unrelated to interfaces,
    // none of them ever defined parse/verify) since `implements` was still
    // optional. Task 7 made `implements` REQUIRED, so every fixture that
    // expects a successful `compile`/`compile_strict` now carries a real
    // `implements ISolve` + a same-name/arity-1 `solve` -- usually the
    // fixture's OWN probe function renamed from `run` to `solve(input: str)`
    // with the parameter left unused, exactly like qc_decision.cube's real
    // `solve` ignores its declared `input` in favor of injected registers.
    // Fixtures that already expect a compile ERROR (missing verify, an
    // unknown statement under --strict, a sealed-capability collision, ...)
    // are deliberately left with no `implements` at all: `check_implements`
    // now ALSO rejects them (for that additional reason), and since
    // `compile_strict`/`compile_ast_strict` combine every error category
    // into one string, the substring each test actually asserts on is still
    // present regardless. `test_compile_gsm8k` (a real `include_str!` of
    // examples/gsm8k.cube) and `test_cubebin_roundtrip` (asserts on
    // `.implements` content directly) predate this and already carried a
    // real `implements ISolver` + full parse/solve/verify triad.

    #[test]
    fn test_compile_empty_program() {
        let programs = compile(r#"
            program Empty implements ISolve {
                public function solve(input: str): void {
                }
            }
        "#).unwrap();
        assert_eq!(programs.len(), 1);
        assert_eq!(programs[0].name, "Empty");
        assert_eq!(programs[0].functions.len(), 1);
        assert_eq!(programs[0].functions[0].name, "solve");
        assert!(programs[0].functions[0].bytecode.is_empty());
    }

    #[test]
    fn test_compile_opcodes() {
        let programs = compile(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 42;
                    add x, 10;
                    sub x, 5;
                    mul x, 2;
                    div x, 3;
                    sum x;
                    push x;
                    pop y;
                    query x;
                    remember x;
                }
            }
        "#).unwrap();

        let func = &programs[0].functions[0];
        assert!(!func.bytecode.is_empty());

        // First instruction should be CREATE (0x00)
        assert_eq!(func.bytecode[0], op::CREATE);
        // Debug lines should track all instructions
        assert!(func.debug_lines.len() >= 11);
    }

    #[test]
    fn test_compile_bytecode_format() {
        let programs = compile(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 16;
                    sub x, 3;
                    sum x;
                    query x;
                    remember x;
                }
            }
        "#).unwrap();

        let bc = &programs[0].functions[0].bytecode;
        let hex: String = bc.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
        println!("Bytecode: {}", hex);
        println!("Length: {} bytes", bc.len());

        // Verify structure: CREATE(0x00) + 2 ops + ASSIGN(0x02) + 2 ops + SUB(0x04) + 2 ops + SUM(0x34) + 1 op + QUERY(0x0C) + 1 op + REMEMBER(0x21) + 1 op
        assert_eq!(bc[0], op::CREATE, "first opcode should be CREATE");

        // Find SUM
        let sum_pos = bc.iter().position(|&b| b == op::SUM).unwrap();
        assert!(sum_pos > 0, "SUM should appear in bytecode");

        // Find REMEMBER at the end
        let rem_pos = bc.iter().rposition(|&b| b == op::REMEMBER).unwrap();
        assert!(rem_pos > sum_pos, "REMEMBER should come after SUM");
    }

    #[test]
    fn test_compile_with_control_flow() {
        let programs = compile(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    if (x > 0) {
                        add x, 1;
                    } else {
                        sub x, 1;
                    }
                }
            }
        "#).unwrap();

        let func = &programs[0].functions[0];
        assert!(!func.bytecode.is_empty());
        // Should contain JMP and LABEL opcodes for control flow
        assert!(func.bytecode.contains(&op::JMP));
        assert!(func.bytecode.contains(&op::LABEL));
    }

    #[test]
    fn test_compile_atomic_block() {
        let programs = compile(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    atomic {
                        assign x = 12;
                        sub x, 4;
                        commit;
                    }
                }
            }
        "#).unwrap();

        let func = &programs[0].functions[0];
        assert!(!func.bytecode.is_empty());
        // Should contain LABEL for atomic boundaries
        assert!(func.bytecode.contains(&op::LABEL));
        assert!(func.bytecode.contains(&op::ASSIGN));
        assert!(func.bytecode.contains(&op::SUB));
    }

    #[test]
    fn test_compile_let_stmt() {
        let programs = compile(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    let x: f64 = 3.14;
                }
            }
        "#).unwrap();

        let func = &programs[0].functions[0];
        // let compiles to CREATE + ASSIGN
        assert!(func.bytecode.contains(&op::CREATE));
        assert!(func.bytecode.contains(&op::ASSIGN));
    }

    #[test]
    fn test_compile_gsm8k_now_fails_even_loose_on_the_c11_operand_bug() {
        // FIXED 2026-09-14 (DRIFT.md §C11): this test used to assert
        // gsm8k.cube compiled clean under loose `compile()` and inspect its
        // bytecode. It no longer can: `learn()`'s
        // `let diff = expected.result - actual.result;` is a `BinOp` used
        // directly as a `let` RHS, which `compile_expr_operand` cannot
        // encode -- previously a silent OP_NONE placeholder, now a hard
        // compile error in EVERY mode (not just `--strict`), per the C11
        // fix. This is the correct, honest outcome: a program that could
        // only ever silently return a meaningless answer (docs/DRIFT.md
        // "Bugs found" #1) should not compile at all. See
        // `tests/examples_pinned.rs` for the CLI-level pin of this same
        // outcome.
        let source = include_str!("../examples/gsm8k.cube");
        let err = compile(source).expect_err(
            "gsm8k.cube uses un-encodable expression operands -- must be a compile error now");
        // WHICH violation fires first is traversal order, not a contract: the
        // constructor's `MapLit` is reached before `learn()`'s `BinOp`. Assert the
        // shape of the diagnostic, not the identity of whichever one is hit first,
        // or this test breaks every time an unrelated statement moves.
        let msg = err.to_string();
        assert!(msg.contains("cannot be encoded as an operand"),
            "error should be the C11 operand diagnostic: {}", msg);
        // (Struct and map literals are built since 2026-10-24, so they no longer appear here.)
        assert!(["BinOp", "UnaryOp", "Lambda", "MethodCall", "Call", "Index"].iter().any(|f| msg.contains(f)),
            "error should name the offending expression form: {}", msg);
    }

    #[test]
    fn test_compile_debug_lines() {
        let programs = compile(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 16;
                    add x, 3;
                    sum x;
                }
            }
        "#).unwrap();

        let func = &programs[0].functions[0];
        // Each opcode stmt should have a debug line
        assert_eq!(func.debug_lines.len(), 4);
        assert_eq!(func.debug_lines[0].opcode, op::CREATE);
        assert!(func.debug_lines[0].description.contains("create x"));
    }

    #[test]
    fn test_cubebin_roundtrip() {
        // `solve` used to end `return Output { result: 9 };` -- a StructLit operand,
        // which the C11 fix now rejects outright (DRIFT.md §C11, 2026-09-14). The
        // roundtrip under test is serialize/deserialize, not struct literals, so the
        // fixture returns the register it just computed instead.
        // (Note for the next person: CubeLang comments with `#`, not `//` -- a `//`
        // inside this raw string lexes as Slash and fails with "unexpected token in
        // expression".)
        let programs = compile(r#"
            program GSM8K implements ISolver {
                storage {
                    patterns: mutable map<str, function>;
                    count: mutable u64 = 0;
                }

                public function constructor() {
                    self.count = 0;
                }

                @external
                public function solve(input: Input): Output {
                    create x : number;
                    assign x = 16;
                    sub x, 3;
                    sub x, 4;
                    sum x;
                    query x;
                    remember x;
                    return x;
                }

                @external
                public function parse(raw: str): Input {
                    return raw;
                }

                @external
                public pure function verify(input: Input, output: Output): bool {
                    return true;
                }
            }
        "#).unwrap();

        let prog = &programs[0];

        // Serialize
        let bin = prog.to_cubebin();
        assert_eq!(&bin[0..4], b"CUBE");
        assert_eq!(bin[4], CUBEBIN_VERSION); // version (v2: + symbol table)

        // Deserialize
        let loaded = CompiledProgram::from_cubebin(&bin).unwrap();
        assert_eq!(loaded.name, "GSM8K");
        assert_eq!(loaded.implements, vec!["ISolver"]);
        assert_eq!(loaded.storage_fields, vec!["patterns", "count"]);
        // Task 4: `implements ISolver` is now VALIDATED, not decorative -- this
        // program must provide parse/verify too, so 4 functions, not the
        // original 2 (constructor + solve only).
        assert_eq!(loaded.functions.len(), 4);
        assert_eq!(loaded.functions[0].name, "constructor");
        assert_eq!(loaded.functions[1].name, "solve");

        // Bytecode should match exactly
        assert_eq!(loaded.functions[1].bytecode, prog.functions[1].bytecode);
    }

    #[test]
    fn test_cubebin_file_roundtrip() {
        let programs = compile(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 42;
                    sum x;
                }
            }
        "#).unwrap();

        let prog = &programs[0];
        let path = std::env::temp_dir().join("test_cubelang.cubebin");

        // Save
        prog.save(&path).unwrap();

        // Load
        let loaded = CompiledProgram::load(&path).unwrap();
        assert_eq!(loaded.name, "Test");
        assert_eq!(loaded.functions[0].bytecode, prog.functions[0].bytecode);

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    // ── P0-1: --strict compile mode ──────────────────────────────────

    #[test]
    fn two_spellings_on_one_key_are_a_compile_error() {
        // P6-9: no 8-byte collision is known, so one is planted: `y`'s key already belongs to `x`.
        let mut c = Compiler::new();
        c.keyed.insert(crate::vm::key::Key::wide("y"), "x".to_string());
        let ast = crate::parser::parse(
            "program P implements ISolve { public function solve(mention: str): number { let y = 1; return y; } }",
        ).unwrap();
        c.compile(&ast);
        assert!(c.operand_errors.iter().any(|e| e.message.starts_with("KeyCollision") && e.message.contains("`x`")
            && e.message.contains("`y`")), "{:?}", c.operand_errors);
    }

    #[test]
    fn strict_qc_decision_compiles() {
        let source = include_str!("../examples/qc_decision.cube");
        let result = compile_strict(source);
        assert!(result.is_ok(),
            "qc_decision.cube (safe-subset reference) must compile under --strict: {:?}",
            result.err());
    }

    #[test]
    fn strict_accepts_for_now_that_it_executes() {
        // Plan-4: `for` lowers to while+index; it executes for real and is
        // no longer a strict-mode violation.
        let source = r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    let xs = [1, 2, 3];
                    create total : number;
                    assign total = 0;
                    for (let x of xs) {
                        add total, x;
                    }
                    sum total;
                }
            }
        "#;
        compile_strict(source).expect("for must be accepted under --strict now");
    }

    #[test]
    fn strict_accepts_match_now_that_arm_selection_is_real() {
        // FLIPPED 2026-09-14 (DRIFT.md C3/A7). This was
        // `strict_rejects_match`, and it was right to: `compile_match` used
        // to run EVERY arm's body unconditionally, so `match` belonged on
        // the `--strict` blocklist for exactly the stated reason ("no arm
        // selection at runtime"). `compile_match` now emits a real
        // COMPARE/COND chain, so the construct executes and strict mode has
        // nothing to object to. See `tests/match_arms.rs` for the behavioural
        // pins, including that strict still descends INTO arm bodies.
        //
        // The fixture also gained the `implements ISolve` clause every
        // program has required since the interface registry landed -- without
        // it this test would "pass" on an unrelated error.
        let source = r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 1;
                    match (x) {
                        1 => { sum x; }
                        _ => { query x; }
                    }
                }
            }
        "#;
        compile_strict(source)
            .expect("match executes now, so --strict must accept it");
    }

    #[test]
    fn strict_rejects_trace_only_predict() {
        let source = r#"
            program Test {
                public function run(): void {
                    predict x;
                }
            }
        "#;
        let err = compile_strict(source).expect_err("predict must be rejected in --strict");
        assert!(err.to_lowercase().contains("predict"),
            "error must name 'predict': got: {}", err);
    }

    #[test]
    fn strict_rejects_trace_only_infer() {
        let source = r#"
            program Test {
                public function run(): void {
                    infer x;
                }
            }
        "#;
        let err = compile_strict(source).expect_err("infer must be rejected in --strict");
        assert!(err.to_lowercase().contains("infer"));
    }

    #[test]
    fn strict_accepts_ask_now_that_it_suspends_and_resumes() {
        // 2026-08-30: `ask` is the third execution outcome (Suspend/resume),
        // not a trace-only stub -- examples/ask_min.cube must pass
        // verify-before-execute, or ASK can never reach the bridge.
        let source = r#"
            program AskMin implements ISolve {
                public function solve(input: str): number {
                    create luna : number;
                    assign luna = 1959;
                    create apollo : number;
                    assign apollo = 1969;
                    ask "which moon landing did you mean", luna, apollo;
                    create chosen : number;
                    pop chosen;
                    sum chosen;
                    return chosen;
                }
            }
        "#;
        let programs = compile_strict(source).expect("ask must compile under --strict");
        assert_eq!(programs.len(), 1);
        assert!(programs[0].functions[0].bytecode.contains(&op::ASK));
        assert!(!is_trace_only_ext_op(ExtOp::Ask));
    }

    #[test]
    fn strict_accepts_call_now_that_it_executes() {
        // Plan-2: CALL is real. A program whose only "stub" risk is a call
        // should now compile under --strict.
        let source = r#"
            program Test implements ISolve {
                public function helper(): number {
                    return 1;
                }
                public function solve(input: str): void {
                    let r = helper();
                    sum r;
                }
            }
        "#;
        compile_strict(source).expect("CALL must be accepted under --strict now");
    }

    #[test]
    fn strict_reports_all_violations_not_just_first() {
        // Two distinct violations on different lines — both should be reported.
        let source = r#"
            program Test {
                public function run(): void {
                    predict x;
                    infer y;
                }
            }
        "#;
        let err = compile_strict(source).expect_err("both violations should be rejected");
        let low = err.to_lowercase();
        assert!(low.contains("predict"), "error must mention predict: {}", err);
        assert!(low.contains("infer"),   "error must mention infer: {}", err);
    }

    // ── Deny-by-default whitelist (Task 0, 2026-08-05) ──────────────────
    // `--strict` was blacklist-shaped: strict_check_stmt only inspected
    // Match/ExtOp/Unify, so any OTHER unrecognized statement fell through to
    // `Stmt::Expr` -> `compile_expr_discard`'s catch-all (a silent SKIP) and
    // strict never noticed. Live-reproduced: `frobnicate d;` compiled clean
    // under --strict. These tests pin the fix: strict now denies by default.

    #[test]
    fn strict_rejects_unknown_statement() {
        // `frobnicate` is not a real construct. It parses as a bare
        // expression-statement (an unrecognized leading identifier), which
        // compiled to a no-op SKIP with no error before this fix.
        let source = r#"
            program T {
                public function solve(): number {
                    frobnicate d;
                    return 0;
                }
            }
        "#;
        let err = compile_strict(source).expect_err(
            "an unrecognized statement must be rejected under --strict");
        assert!(err.to_lowercase().contains("unrecognized")
            || err.to_lowercase().contains("no compiled effect"),
            "error should explain the statement has no compiled effect: {}", err);
    }

    #[test]
    fn an_index_assign_target_compiles_and_a_nested_one_is_a_hard_error() {
        // FLIPPED 2026-09-14 (DRIFT.md §C4). This was
        // `strict_rejects_unsupported_assign_target`, and it was right to:
        // `Stmt::Assign` only handled `self.field` and a bare identifier, so
        // `arr[0] = 5` silently compiled to ZERO bytecode -- the sibling
        // silent-drop named in the audit, and the mechanism behind
        // `examples/gsm8k.cube` returning a meaningless 0.
        //
        // `compile_assign` now emits SET_INDEX for a single index into a
        // register or a storage field, so this compiles and executes. The
        // fixture also gained the `implements` clause every program has
        // required since the interface registry landed -- without it the old
        // test was passing on an unrelated error, not on the assign at all.
        let source = r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create arr : array<number>;
                    arr[0] = 5;
                }
            }
        "#;
        compile_strict(source)
            .expect("an index assign target executes now, so --strict must accept it");

        // The guarantee the old test was really protecting -- that an
        // un-encodable target does not silently vanish -- moves to the
        // shapes that are STILL un-encodable. These are now hard errors in
        // every mode (not just --strict), same rationale as C11.
        let nested = r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create grid : array<number>;
                    grid[0][1] = 5;
                }
            }
        "#;
        let err = compile(nested).expect_err(
            "a nested index target cannot be encoded and must not compile silently");
        assert!(err.to_string().contains("cannot be encoded"),
            "error must say the target cannot be encoded: {}", err);
    }

    #[test]
    fn strict_rejects_throw() {
        // `Stmt::Throw` is parsed but never compiled — compile_stmt's own
        // catch-all (`_ => emit SKIP`) silently no-ops it, same hole family
        // as the Stmt::Expr fall-through above, just one enum variant over.
        let source = r#"
            program T {
                public function run(): void {
                    throw "boom";
                }
            }
        "#;
        let err = compile_strict(source).expect_err("throw must be rejected under --strict");
        assert!(err.to_lowercase().contains("throw"), "error must name throw: {}", err);
    }

    #[test]
    fn strict_rejects_assert_with_non_call_condition() {
        // Stmt::Assert compiles its condition via compile_expr_discard,
        // which only emits real bytecode for a Call/MethodCall. A normal
        // comparison condition (the overwhelmingly common case for a real
        // assert) silently compiled to a single SKIP -- the assert never
        // actually checked anything at runtime.
        let source = r#"
            program T {
                public function run(): void {
                    create x : number;
                    assign x = 1;
                    assert x > 0;
                }
            }
        "#;
        let err = compile_strict(source).expect_err(
            "an assert whose condition compiles to no real check must be rejected");
        assert!(err.to_lowercase().contains("assert"), "error must name assert: {}", err);
    }

    #[test]
    fn strict_still_accepts_qc_decision_after_whitelist_flip() {
        // Regression guard for the whitelist rewrite itself: the safe-subset
        // reference program must still compile clean (duplicates
        // strict_qc_decision_compiles but colocated with the new tests so a
        // future edit to the whitelist trips this immediately too).
        let source = include_str!("../examples/qc_decision.cube");
        compile_strict(source).expect("qc_decision.cube must still compile under --strict");
    }

    #[test]
    fn non_strict_compile_still_allows_unknown_statement_and_bad_assign() {
        // Non-strict path is untouched by the whitelist flip (regression
        // guard, mirrors non_strict_compile_still_allows_trace_only below).
        let source = r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create arr : array<u64>;
                    frobnicate d;
                    arr[0] = 5;
                }
            }
        "#;
        compile(source).expect("non-strict compile must still accept these");
    }

    #[test]
    fn non_strict_compile_still_allows_trace_only() {
        // Non-strict path must keep accepting these (regression guard).
        let source = r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    predict x;
                    infer y;
                }
            }
        "#;
        let programs = compile(source).expect("non-strict compile must accept trace-only ops");
        assert_eq!(programs.len(), 1);
        let func = &programs[0].functions[0];
        assert!(func.bytecode.contains(&op::PREDICT));
        assert!(func.bytecode.contains(&op::INFER));
    }

    #[test]
    fn test_cubebin_gsm8k_full_now_fails_to_compile_on_the_c11_operand_bug() {
        // FIXED 2026-09-14 (DRIFT.md §C11): this test used to compile
        // gsm8k.cube whole and roundtrip it through .cubebin. It can no
        // longer compile at all -- see
        // `test_compile_gsm8k_now_fails_even_loose_on_the_c11_operand_bug`
        // above for why (a `BinOp` used directly as a `let` RHS in
        // `learn()`). Nothing left to roundtrip; this test now just pins
        // the same honest failure so a future change that silently starts
        // tolerating the bug again is caught here too.
        let source = include_str!("../examples/gsm8k.cube");
        compile(source).expect_err(
            "gsm8k.cube must not compile -- see the C11 operand-encoding fix");
    }
}
