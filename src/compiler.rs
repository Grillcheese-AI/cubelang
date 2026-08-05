//! CubeLang bytecode compiler — AST → CubeMind VM bytecodes.
//!
//! Walks the AST and emits binary bytecodes (0x00-0xFF) for each instruction.
//! The output is a `CompiledProgram` containing the bytecode blob, metadata,
//! and a symbol table for debugging.

use std::collections::HashMap;

use crate::ast::*;
use crate::parser::ParseError;
// The registry is a static, deterministic capability table (see its module
// doc), not live VM execution state -- the compiler needs read-only access
// to it purely to validate `override` declarations at compile time.
use crate::vm::registry::ModuleRegistry;
// Task 4: same "static, deterministic table" reasoning as ModuleRegistry
// above, but for standard interfaces -- a separate registry (contracts, not
// callable impls), consulted to validate `implements` declarations.
use crate::vm::interfaces::InterfaceRegistry;

// ── Opcode constants (match opcode-vsa-rs ir.rs) ────────────────────────────

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
const OP_NONE: u8   = 0x00;

/// BLAKE3 2-byte prefix of a string (deterministic hash).
fn name_hash(s: &str) -> [u8; 2] {
    let h = blake3::hash(s.as_bytes());
    [h.as_bytes()[0], h.as_bytes()[1]]
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
pub const CUBEBIN_VERSION: u8 = 3;

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
    pub used_modules: Vec<String>,
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
        if version != 1 && version != 2 && version != 3 {
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

            functions.push(CompiledFunction {
                name: fn_name,
                params,
                bytecode,
                labels: Vec::new(),
                debug_lines: Vec::new(),
                // Task 3: not in the .cubebin format yet -- see the field doc.
                is_override: false,
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

        // Task 3: `used_modules` isn't in the .cubebin format yet (see the
        // field doc on `CompiledProgram`) -- a round-tripped program always
        // comes back with none `use`'d, same forward-compatible shape as a
        // pre-v3 file's empty `params`.
        Ok(CompiledProgram { name, implements, functions, storage_fields, symbol_names, used_modules: Vec::new() })
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
    /// disagrees with its own source. A program using neither is unaffected
    /// -- `to_cubebin`/`from_cubebin` themselves are unchanged.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(reason) = self.capability_conflict() {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, reason));
        }
        std::fs::write(path, self.to_cubebin())
    }

    /// Fix-round-1 (Task 3): `Some(reason)` when this program can't be
    /// faithfully represented by the current `.cubebin` format -- it `use`s
    /// at least one module, or declares at least one `override` function --
    /// `None` when it's safe to serialize as-is.
    fn capability_conflict(&self) -> Option<String> {
        let has_override = self.functions.iter().any(|f| f.is_override);
        if self.used_modules.is_empty() && !has_override {
            return None;
        }
        Some(format!(
            "cannot serialize program `{}` to .cubebin: it uses `use`/`override` \
             (used modules: [{}]) and the binary format does not yet carry \
             capability info -- run from source instead",
            self.name,
            self.used_modules.join(", "),
        ))
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

        let mut programs = Vec::new();
        for item in &source.items {
            if let TopLevel::Program(p) = item {
                programs.push(self.compile_program(p));
            }
        }
        programs
    }

    fn compile_program(&mut self, prog: &ProgramDecl) -> CompiledProgram {
        let mut functions = Vec::new();
        let mut storage_fields = Vec::new();
        self.current_program = prog.name.clone();
        // Reset per-program symbol collection so bindings don't leak across
        // programs compiled in the same pass.
        self.symbol_names.clear();

        // Task 4: validate `implements` against the VM interface registry or
        // an in-file inline decl before compiling the body -- see
        // check_implements's doc for the registry-first resolution order.
        self.check_implements(prog);

        for item in &prog.body {
            match item {
                ProgramItem::Storage(s) => {
                    for f in &s.fields {
                        storage_fields.push(f.name.clone());
                    }
                }
                ProgramItem::Function(f) | ProgramItem::Constructor(f) => {
                    functions.push(self.compile_function(f));
                }
                _ => {}
            }
        }

        CompiledProgram {
            name: prog.name.clone(),
            implements: prog.implements.clone(),
            functions,
            storage_fields,
            symbol_names: std::mem::take(&mut self.symbol_names),
            used_modules: self.used_modules.clone(),
        }
    }

    fn compile_function(&mut self, func: &FunctionDecl) -> CompiledFunction {
        self.bytecode.clear();
        self.labels.clear();
        self.debug_lines.clear();
        self.current_function = func.sig.name.clone();

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

        for stmt in &func.body {
            self.compile_stmt(stmt);
        }

        CompiledFunction {
            name: func.sig.name.clone(),
            params: func.sig.params.iter().map(|p| p.name.clone()).collect(),
            bytecode: self.bytecode.clone(),
            labels: self.labels.clone(),
            debug_lines: self.debug_lines.clone(),
            is_override: func.sig.is_override,
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

    // ── Standard interfaces (Task 4): validated `implements` ────────────
    //
    // `implements X` used to be pure decoration -- parsed into
    // `ProgramDecl::implements`, round-tripped through `.cubebin`, never
    // checked against anything. This makes it real: `X` must resolve to
    // either a VM-internal interface (`vm::interfaces::InterfaceRegistry`)
    // or an in-file inline `interface X { ... }` decl, and the program must
    // provide a same-name/same-arity function for every required member of
    // whichever one it resolved to.
    //
    // Deliberately NOT required yet: an empty (or absent) `implements` list
    // is untouched by this check (Task 7 makes it required, once every
    // example conforms) -- see the early return below.

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
    /// registry does NOT recognize (e.g. `conversation_agent.cube`'s own
    /// `IConversationAgent`), which is exactly the "existing programs that
    /// declare inline interfaces MUST keep working" case the brief calls
    /// out. This ordering is unobservable on every program in examples/
    /// today -- the six inline `ISolver` decls this project's examples
    /// carry are identical in shape to the registry's `ISolver` (Task 7
    /// deletes them as redundant once `implements` is required) -- it only
    /// matters once a program tries to shadow a registry name with a
    /// weaker local one; `a_local_interface_block_cannot_redefine_a_registry_interface_to_something_weaker`
    /// (tests/interfaces.rs) pins this down directly.
    fn check_implements(&mut self, prog: &ProgramDecl) {
        if prog.implements.is_empty() {
            return; // Task 7 makes this required; Task 4 does not.
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
            Stmt::Match(m) => {
                self.strict_violation("match (no arm selection at runtime)",
                    Some(m.span.line));
            }
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
            Stmt::Let(_) | Stmt::If(_) | Stmt::For(_) | Stmt::While(_)
            | Stmt::Return(_) | Stmt::Atomic(_) | Stmt::Emit(_) => {}
            // try/catch bodies are real (LABEL/JMP scaffolding); nested
            // statements are independently re-checked via recursion.
            // NOTE: `finally_body` is a separate, pre-existing gap --
            // compile_stmt's TryCatch arm never compiles it at all (not
            // even to a SKIP), so no per-statement bytecode check can catch
            // it here. Out of scope for this fix; see task report.
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
            Stmt::Assign(a) => {
                // compile_stmt's Assign arm only handles `self.field` and a
                // bare identifier target. An index target (`arr[0] = x`),
                // nested field access, or anything else silently compiles
                // to ZERO bytecode -- the sibling silent-drop from the audit.
                if !matches!(a.target, Expr::SelfAccess(_) | Expr::Ident(_)) {
                    self.strict_violation(
                        &format!("assign (unsupported assignment target `{:?}` -- only \
                         `self.field` and a bare identifier compile)", a.target),
                        Some(a.span.line));
                }
            }
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
            Stmt::BytecodeBlock(b) => {
                self.strict_violation(
                    "bytecode block (not compiled -- falls through to a no-op under the current compiler)",
                    Some(b.span.line));
            }
        }
    }

    // ── Statement compilation ───────────────────────────────────────────

    fn compile_stmt(&mut self, stmt: &Stmt) {
        if self.strict {
            self.strict_check_stmt(stmt);
        }
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
                let catch_label = self.fresh_label("catch");
                self.emit_debug(op::LABEL, "try_start");
                for s in &tc.try_body {
                    self.compile_stmt(s);
                }
                let end_label = self.fresh_label("try_end");
                self.emit_jmp(&end_label);
                self.emit_label(&catch_label);
                for s in &tc.catch_body {
                    self.compile_stmt(s);
                }
                self.emit_label(&end_label);
            }
            Stmt::Assign(a) => {
                // x = expr or x += expr
                if let Expr::SelfAccess(field) = &a.target {
                    self.emit_debug(op::ASSIGN, &format!("self.{} = ...", field));
                    self.emit_byte(op::ASSIGN);
                    self.emit_byte(2);
                    self.emit_named(field);
                    self.compile_expr_operand(&a.value);
                } else if let Expr::Ident(name) = &a.target {
                    self.emit_debug(op::ASSIGN, &format!("{} = ...", name));
                    self.emit_byte(op::ASSIGN);
                    self.emit_byte(2);
                    self.emit_named(name);
                    self.compile_expr_operand(&a.value);
                }
            }
            Stmt::Expr(e) => {
                self.compile_expr_discard(e);
            }
            _ => {
                // Other statements: emit SKIP as placeholder
                self.emit_byte(op::SKIP);
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
                self.emit_type(ty);
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
                self.emit_byte(args.len() as u8);
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

    fn compile_match(&mut self, match_stmt: &MatchStmt) {
        let end_label = self.fresh_label("endmatch");
        self.emit_debug(op::COMPARE, "match");

        for arm in &match_stmt.arms {
            for s in &arm.body {
                self.compile_stmt(s);
            }
        }

        self.emit_label(&end_label);
    }

    fn compile_let(&mut self, let_stmt: &LetStmt) {
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
                self.emit_byte((elems.len() as u8).saturating_add(2));
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
            Expr::Index(arr_expr, idx_expr) => {
                if let Expr::Ident(arr) = arr_expr.as_ref() {
                    self.emit_debug(op::INDEX,
                        &format!("index {} = {}[...]", let_stmt.name, arr));
                    self.emit_byte(op::INDEX);
                    self.emit_byte(3);
                    self.emit_named(&let_stmt.name);
                    self.emit_named(arr);
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
        match callee {
            Expr::Ident(name) => {
                self.emit_debug(op::CALL, &format!("call {}({})", name, args.len()));
                self.emit_byte(op::CALL);
                self.emit_byte((args.len() + 1) as u8);
                self.emit_global(name);
                for a in args { self.compile_expr_operand(a); }
            }
            _ => {
                self.emit_debug(op::CALL, "call <expr>");
                self.emit_byte(op::CALL);
                self.emit_byte((args.len() + 1) as u8);
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
            Expr::Null => self.emit_imm(0),
            Expr::Ident(name) => self.emit_named(name),
            Expr::SelfAccess(field) => self.emit_named(field),
            Expr::Field(base, field) => {
                // For now, emit as the field name (the VM resolves at runtime)
                if let Expr::Ident(base_name) = base.as_ref() {
                    self.emit_named(&format!("{}.{}", base_name, field));
                } else {
                    self.emit_named(field);
                }
            }
            _ => {
                // Complex expressions: emit as opaque reference
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
                self.emit_byte((args.len() + 1) as u8);
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

    fn emit_named(&mut self, name: &str) {
        self.emit_byte(OP_NAMED);
        let hash = name_hash(name);
        self.emit_byte(hash[0]);
        self.emit_byte(hash[1]);
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
        self.emit_byte(OP_TYPE);
        let hash = name_hash(ty);
        self.emit_byte(hash[0]);
        self.emit_byte(hash[1]);
    }

    fn emit_role(&mut self, role: &str) {
        self.emit_byte(OP_ROLE);
        let hash = name_hash(role);
        self.emit_byte(hash[0]);
        self.emit_byte(hash[1]);
    }

    fn emit_global(&mut self, name: &str) {
        // Globals (string literals, mostly) compile to a 2-byte hash. Record the
        // spelling so `VM::load` puts it in the name table and
        // `resolve_symbol_name` can reverse it. Without this an ASK's question
        // text -- the very thing the trunk is supposed to render into language
        // for the user -- comes back as `g_e009`.
        self.record_symbol(name);
        self.emit_byte(OP_GLOBAL);
        let hash = name_hash(name);
        self.emit_byte(hash[0]);
        self.emit_byte(hash[1]);
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
        self.emit_debug(op::COMPARE, "cond truthy");
        self.emit_byte(op::COMPARE);
        self.emit_byte(2);
        self.compile_expr_operand(cond);
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

fn type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Array(_) => "array".into(),
        TypeExpr::Map(_, _) => "map".into(),
        TypeExpr::Void => "void".into(),
        _ => "any".into(),
    }
}

/// True when this extended opcode does NOT execute value-level semantics in
/// the current VM (it only emits a structural trace line). The strict-mode
/// blocklist (P0-1) uses this; it is also the authoritative list for the
/// `validate` subcommand (P0-3). The list mirrors `trace_structural` in
/// `src/vm/engine.rs` — keep in sync as P2 opcodes start executing.
pub fn is_trace_only_ext_op(o: ExtOp) -> bool {
    matches!(o,
        ExtOp::Infer | ExtOp::MapRoles | ExtOp::Filter | ExtOp::Score
        | ExtOp::DetectPattern | ExtOp::Decode | ExtOp::Reduce | ExtOp::Merge
        | ExtOp::Split | ExtOp::Debate | ExtOp::Predict | ExtOp::Discover
        | ExtOp::Diff | ExtOp::Seq | ExtOp::Specialize | ExtOp::Reward
        | ExtOp::Match | ExtOp::TemporalBind | ExtOp::Analogy | ExtOp::Gen
        | ExtOp::Inst | ExtOp::Broadcast | ExtOp::Explore | ExtOp::Forge
        | ExtOp::Ask | ExtOp::Sync
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
    Ok(programs)
}

/// Compile in strict mode (P0-1): rejects any construct that compiles but does
/// not execute under the safe-subset VM — extended trace-only opcodes,
/// `for`, `match`, intra-program `call`. Returns one combined error string
/// listing every violation (named construct + source line when available).
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
    errors.extend(compiler.strict_errors);
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(programs)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Task 4: most fixtures below dropped a decorative `implements ISolver`
    // that predates real `implements` validation -- these tests exercise
    // bytecode/format/strict-mode concerns unrelated to interfaces, and none
    // of them ever defined parse/verify, so the claim was already false
    // before this task, just never checked. `test_compile_gsm8k` (a real
    // `include_str!` of examples/gsm8k.cube) and `test_cubebin_roundtrip`
    // (asserts on `.implements` content directly) are the two exceptions
    // that keep it, and `test_cubebin_roundtrip`'s fixture now provides the
    // full parse/solve/verify triad to match.

    #[test]
    fn test_compile_empty_program() {
        let programs = compile(r#"
            program Empty {
                public function run(): void {
                }
            }
        "#).unwrap();
        assert_eq!(programs.len(), 1);
        assert_eq!(programs[0].name, "Empty");
        assert_eq!(programs[0].functions.len(), 1);
        assert_eq!(programs[0].functions[0].name, "run");
        assert!(programs[0].functions[0].bytecode.is_empty());
    }

    #[test]
    fn test_compile_opcodes() {
        let programs = compile(r#"
            program Test {
                public function run(): void {
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
            program Test {
                public function run(): void {
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
            program Test {
                public function run(): void {
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
            program Test {
                public function run(): void {
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
            program Test {
                public function run(): void {
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
    fn test_compile_gsm8k() {
        let source = include_str!("../examples/gsm8k.cube");
        let programs = compile(source).unwrap();

        assert_eq!(programs.len(), 1);
        assert_eq!(programs[0].name, "GSM8K");
        assert_eq!(programs[0].implements, vec!["ISolver"]);

        // Should have constructor + parse + solve + verify + learn = 5 functions
        assert_eq!(programs[0].functions.len(), 5);

        let names: Vec<&str> = programs[0].functions.iter().map(|f| f.sig_name()).collect();
        println!("Functions: {:?}", names);

        // Print solve function bytecode
        let solve = programs[0].functions.iter().find(|f| f.name == "solve").unwrap();
        println!("solve() bytecode: {} bytes", solve.bytecode.len());
        for dl in &solve.debug_lines {
            println!("  0x{:04x}: 0x{:02x} {}", dl.offset, dl.opcode, dl.description);
        }
    }

    #[test]
    fn test_compile_debug_lines() {
        let programs = compile(r#"
            program Test {
                public function run(): void {
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
                    return Output { result: 9 };
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
            program Test {
                public function run(): void {
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
            program Test {
                public function run(): void {
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
    fn strict_rejects_match() {
        let source = r#"
            program Test {
                public function run(): void {
                    match (x) {
                        1 => { sum x; }
                        _ => { query x; }
                    }
                }
            }
        "#;
        let err = compile_strict(source).expect_err("match must be rejected in --strict");
        let low = err.to_lowercase();
        assert!(low.contains("match"), "error must name 'match': got: {}", err);
        assert!(err.contains("line"), "error must include source line: got: {}", err);
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
    fn strict_accepts_call_now_that_it_executes() {
        // Plan-2: CALL is real. A program whose only "stub" risk is a call
        // should now compile under --strict.
        let source = r#"
            program Test {
                public function helper(): number {
                    return 1;
                }
                public function run(): void {
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
    fn strict_rejects_unsupported_assign_target() {
        // Stmt::Assign only handles `self.field` and a bare identifier
        // target. An index target (`arr[0] = x`) or any other non-trivial
        // target silently compiled to ZERO bytecode, with no error, before
        // this fix — the sibling silent-drop named in the audit.
        let source = r#"
            program T {
                public function run(): void {
                    create arr : array<u64>;
                    arr[0] = 5;
                }
            }
        "#;
        let err = compile_strict(source).expect_err(
            "an unsupported assignment target must be rejected under --strict");
        let low = err.to_lowercase();
        assert!(low.contains("assign"), "error must name the assign statement: {}", err);
        assert!(err.contains("line"), "error must include a source line: got: {}", err);
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
            program T {
                public function run(): void {
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
            program Test {
                public function run(): void {
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
    fn test_cubebin_gsm8k_full() {
        let source = include_str!("../examples/gsm8k.cube");
        let programs = compile(source).unwrap();
        let prog = &programs[0];

        let bin = prog.to_cubebin();
        println!(".cubebin size: {} bytes", bin.len());
        println!("  magic: {:?}", std::str::from_utf8(&bin[0..4]).unwrap());
        println!("  version: {}", bin[4]);
        println!("  program: {}", prog.name);
        println!("  functions: {}", prog.functions.len());
        for f in &prog.functions {
            println!("    {}(): {} bytes bytecode", f.name, f.bytecode.len());
        }

        // Roundtrip
        let loaded = CompiledProgram::from_cubebin(&bin).unwrap();
        assert_eq!(loaded.name, prog.name);
        assert_eq!(loaded.functions.len(), prog.functions.len());
        for (a, b) in loaded.functions.iter().zip(prog.functions.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.bytecode, b.bytecode, "bytecode mismatch for {}", a.name);
        }
    }
}
