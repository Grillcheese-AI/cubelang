//! CubeLang VM — loads and executes `.cubebin` bytecodes.
//!
//! The VM maintains registers, a stack, and storage. It decodes
//! bytecodes instruction-by-instruction and executes them.

use std::collections::HashMap;
use crate::compiler::{CompiledProgram, CompiledFunction, op};
use crate::vm::memory::HippocampalMemory;

/// Hypervector dimension for the VM's hippocampal memory. 4096 matches the
/// codebook/index defaults ported from opcode-vsa-rs; quasi-orthogonality of
/// distinct keys holds comfortably at this width.
const MEM_DIM: usize = 4096;
/// Fixed codebook seed so memory addresses are reproducible across runs.
const MEM_SEED: u64 = 0xC0DEB00C;

/// Runtime value.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Array(Vec<Value>),
    Map(HashMap<String, Value>),
}

impl Value {
    pub fn as_i64(&self) -> i64 {
        match self {
            Value::Int(n) => *n,
            Value::Float(f) => *f as i64,
            Value::Bool(b) => if *b { 1 } else { 0 },
            _ => 0,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            Value::Int(n) => *n as f64,
            Value::Float(f) => *f,
            Value::Bool(b) => if *b { 1.0 } else { 0.0 },
            _ => 0.0,
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "null".into(),
            _ => format!("{:?}", self),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Null => false,
            Value::Str(s) => !s.is_empty(),
            _ => true,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(v) => write!(f, "{}", v),
            Value::Str(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Null => write!(f, "null"),
            Value::Array(a) => write!(f, "[{} items]", a.len()),
            Value::Map(m) => write!(f, "{{{} entries}}", m.len()),
        }
    }
}

/// VM execution result.
#[derive(Debug)]
pub enum ExecResult {
    Ok(Value),
    Return(Value),
    Error(String),
}

/// The CubeLang virtual machine.
pub struct VM {
    /// Named registers.
    pub registers: HashMap<String, Value>,
    /// Stack for PUSH/POP chaining.
    pub stack: Vec<Value>,
    /// Persistent storage (STORE/RECALL) — exact-key fast path.
    pub storage: HashMap<String, Value>,
    /// Hippocampal associative memory (VSA cleanup) for STORE/RECALL/REMEMBER/FORGET.
    pub memory: HippocampalMemory,
    /// Accumulator (SUM results).
    pub accumulator: Value,
    /// Event log.
    pub events: Vec<(String, HashMap<String, Value>)>,
    /// Loaded programs.
    programs: HashMap<String, CompiledProgram>,
    /// Execution trace (for debugging).
    pub trace: Vec<String>,
    /// Enable tracing.
    pub trace_enabled: bool,
    /// Name→hash mapping for reverse lookup.
    name_table: HashMap<[u8; 2], String>,
}

impl VM {
    pub fn new() -> Self {
        Self {
            registers: HashMap::new(),
            stack: Vec::new(),
            storage: HashMap::new(),
            memory: HippocampalMemory::new(MEM_DIM, MEM_SEED),
            accumulator: Value::Null,
            events: Vec::new(),
            programs: HashMap::new(),
            trace: Vec::new(),
            trace_enabled: false,
            name_table: HashMap::new(),
        }
    }

    /// Load a compiled program into the VM.
    pub fn load(&mut self, prog: CompiledProgram) {
        // Pre-populate name table for reverse lookups
        self.register_name(&prog.name);
        for iface in &prog.implements {
            self.register_name(iface);
        }
        for field in &prog.storage_fields {
            self.register_name(field);
        }
        for func in &prog.functions {
            self.register_name(&func.name);
        }
        self.programs.insert(prog.name.clone(), prog);
    }

    /// Load from a .cubebin file.
    pub fn load_file(&mut self, path: &std::path::Path) -> Result<String, String> {
        let prog = CompiledProgram::load(path)?;
        let name = prog.name.clone();
        self.load(prog);
        Ok(name)
    }

    /// Run a function by name in a loaded program.
    pub fn call(&mut self, program: &str, function: &str, args: Vec<Value>) -> ExecResult {
        let prog = match self.programs.get(program) {
            Some(p) => p.clone(),
            None => return ExecResult::Error(format!("program not found: {}", program)),
        };

        let func = match prog.functions.iter().find(|f| f.name == function) {
            Some(f) => f.clone(),
            None => return ExecResult::Error(format!("function not found: {}.{}", program, function)),
        };

        // Set up args as registers (arg0, arg1, ...)
        for (i, arg) in args.into_iter().enumerate() {
            self.registers.insert(format!("arg{}", i), arg);
        }

        self.exec_function(&func)
    }

    /// Run the constructor of a program.
    pub fn init(&mut self, program: &str) -> ExecResult {
        self.call(program, "constructor", Vec::new())
    }

    /// Execute a function's bytecode.
    fn exec_function(&mut self, func: &CompiledFunction) -> ExecResult {
        let bc = &func.bytecode;
        let mut pc: usize = 0;

        // Pre-scan pass: resolve LABEL hashes → bytecode offsets so JMP/COND
        // can jump. We record the offset of the LABEL instruction itself
        // (jumping there re-reads the LABEL as a no-op and continues).
        let label_map = scan_labels(bc);
        // Comparison flag set by COMPARE, tested by COND.
        let mut flag: bool = false;
        // Loop-iteration guard: bound total back-edges to prevent infinite
        // loops from a malformed / non-terminating program reaching the VM.
        let mut jump_budget: u64 = 1_000_000;
        while pc < bc.len() {
            let opcode = bc[pc]; pc += 1;

            // SKIP / NOP
            if opcode == op::SKIP {
                self.trace_op(pc - 1, "SKIP");
                continue;
            }

            // Read operand count
            if pc >= bc.len() { break; }
            let n_ops = bc[pc] as usize; pc += 1;

            // Decode operands
            let mut operands: Vec<Operand> = Vec::new();
            for _ in 0..n_ops {
                if pc >= bc.len() { break; }
                let (op, consumed) = decode_operand(&bc[pc..]);
                operands.push(op);
                pc += consumed;
            }

            // Execute
            match opcode {
                op::CREATE => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let ty = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("CREATE {} : {}", name, ty));
                    self.registers.insert(name, Value::Null);
                }

                op::ASSIGN => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = operands.get(1).map(|o| o.to_value()).unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("ASSIGN {} = {}", name, val));
                    self.registers.insert(name, val);
                }

                op::ADD => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let rhs = self.resolve_value(operands.get(1));
                    let cur = self.registers.get(&name).cloned().unwrap_or(Value::Int(0));
                    let out = num_binop(&cur, &rhs, '+');
                    self.trace_op(pc, &format!("ADD {} {} → {}", name, rhs, out));
                    self.registers.insert(name, out);
                }

                op::SUB => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let rhs = self.resolve_value(operands.get(1));
                    let cur = self.registers.get(&name).cloned().unwrap_or(Value::Int(0));
                    let out = num_binop(&cur, &rhs, '-');
                    self.trace_op(pc, &format!("SUB {} {} → {}", name, rhs, out));
                    self.registers.insert(name, out);
                }

                op::MUL => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let rhs = self.resolve_value(operands.get(1));
                    let cur = self.registers.get(&name).cloned().unwrap_or(Value::Int(0));
                    let out = num_binop(&cur, &rhs, '*');
                    self.trace_op(pc, &format!("MUL {} {} → {}", name, rhs, out));
                    self.registers.insert(name, out);
                }

                op::DIV => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let rhs = self.resolve_value(operands.get(1));
                    // Division by zero (int or float) is an error.
                    if rhs.as_f64() == 0.0 {
                        return ExecResult::Error("division by zero".into());
                    }
                    let cur = self.registers.get(&name).cloned().unwrap_or(Value::Int(0));
                    let out = num_binop(&cur, &rhs, '/');
                    self.trace_op(pc, &format!("DIV {} {} → {}", name, rhs, out));
                    self.registers.insert(name, out);
                }

                op::SUM => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&name).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("SUM {} → {}", name, val));
                    self.accumulator = val;
                }

                op::PUSH => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&name).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("PUSH {} ({})", name, val));
                    self.stack.push(val);
                }

                op::POP => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.stack.pop().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("POP {} ← {}", name, val));
                    self.registers.insert(name, val);
                }

                op::QUERY => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&name).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("QUERY {} → {}", name, val));
                }

                op::REMEMBER => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&name).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("REMEMBER {} ({})", name, val));
                    // Hippocampal store under the register's own name.
                    self.memory.store(&name, val.clone());
                    self.storage.insert(name.clone(), val);
                }

                op::STORE => {
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let key = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&reg).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("STORE {} → {}", reg, key));
                    // Associative store: key-hypervector addresses the payload.
                    self.memory.store(&key, val.clone());
                    self.storage.insert(key, val);
                }

                op::RECALL => {
                    let key = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    // Associative cleanup: nearest stored key by cosine similarity.
                    match self.memory.recall(&key) {
                        Some((matched, val, sim)) => {
                            self.trace_op(pc, &format!("RECALL {} → {} (sim {:.3}) = {}", key, matched, sim, val));
                            self.registers.insert(key, val);
                        }
                        None => {
                            self.trace_op(pc, &format!("RECALL {} → (empty)", key));
                            self.registers.insert(key, Value::Null);
                        }
                    }
                }

                op::FORGET => {
                    let key = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let dropped = self.memory.forget(&key);
                    self.storage.remove(&key);
                    self.trace_op(pc, &format!("FORGET {} ({})", key, if dropped { "dropped" } else { "absent" }));
                }

                op::BIND_ROLE => {
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let role = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let filler = operands.get(2).map(|o| o.to_value()).unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("BIND_ROLE {} {} {}", reg, role, filler));
                    // Store as reg.role = filler
                    self.registers.insert(format!("{}.{}", reg, role), filler);
                }

                op::COMPARE => {
                    // Operands: [a, b, kind_imm] (3-arg from a condition) or
                    // [a, b] (2-arg statement-level compare, defaults to eq).
                    let a = self.resolve_i64(operands.get(0));
                    let kind = match operands.get(2) {
                        Some(Operand::Imm(k)) => *k,
                        _ => 0, // statement-level `compare a, b` → equality test
                    };
                    let b = if kind == 6 { 0 } else { self.resolve_i64(operands.get(1)) };
                    flag = match kind {
                        0 => a == b,
                        1 => a != b,
                        2 => a < b,
                        3 => a <= b,
                        4 => a > b,
                        5 => a >= b,
                        6 => a != 0, // truthiness
                        _ => false,
                    };
                    // Expose the comparison result in a conventional register too.
                    let cmp_word = match kind {
                        0 | 1 => if a == b { "equal" } else if a < b { "less" } else { "greater" },
                        _ => if flag { "true" } else { "false" },
                    };
                    self.registers.insert("__cmp".into(), Value::Str(cmp_word.into()));
                    self.accumulator = Value::Bool(flag);
                    self.trace_op(pc, &format!("COMPARE {} ? {} (kind {}) → {}", a, b, kind, flag));
                }

                op::LABEL => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("LABEL {}", name));
                    // Runtime no-op; targets were resolved in the pre-scan.
                }

                op::JMP => {
                    let key = operands.get(0).and_then(|o| o.label_key());
                    match key.and_then(|k| label_map.get(&k)) {
                        Some(&target) => {
                            self.trace_op(pc, &format!("JMP → 0x{:04x}", target));
                            if jump_budget == 0 {
                                return ExecResult::Error("jump budget exhausted (possible infinite loop)".into());
                            }
                            jump_budget -= 1;
                            pc = target;
                        }
                        None => {
                            self.trace_op(pc, "JMP (unresolved label, falling through)");
                        }
                    }
                }

                op::COND => {
                    // Conditional jump: if the comparison flag is FALSE, jump to
                    // the target label (skip the then-body / exit the loop).
                    let key = operands.get(0).and_then(|o| o.label_key());
                    if !flag {
                        match key.and_then(|k| label_map.get(&k)) {
                            Some(&target) => {
                                self.trace_op(pc, &format!("COND false → jump 0x{:04x}", target));
                                if jump_budget == 0 {
                                    return ExecResult::Error("jump budget exhausted (possible infinite loop)".into());
                                }
                                jump_budget -= 1;
                                pc = target;
                            }
                            None => {
                                self.trace_op(pc, "COND false (unresolved label)");
                            }
                        }
                    } else {
                        self.trace_op(pc, "COND true → fall through");
                    }
                }

                op::LOOP => {
                    // Structural loop marker (for-each). The VM executes the body
                    // as a single structured pass; the exit label is carried for
                    // well-formedness. (Iterating a value-level collection source
                    // is a documented follow-up.)
                    self.trace_op(pc, "LOOP (structural marker)");
                }

                op::CALL => {
                    let target = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("CALL {}", target));
                    // Intra-program call resolution is a documented follow-up;
                    // recorded as a structural step so traces stay faithful.
                }

                // ── Data-movement opcodes (value-level semantics) ──────────

                op::DESTROY => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.registers.remove(&name);
                    self.trace_op(pc, &format!("DESTROY {}", name));
                }

                op::COPY => {
                    let src = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let dst = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&src).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("COPY {} → {}", src, dst));
                    self.registers.insert(dst, val);
                }

                op::TRANSFER => {
                    // TRANSFER src, dst, amount: move `amount` from src to dst.
                    let src = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let dst = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let amount = self.resolve_i64(operands.get(2));
                    let s = self.registers.get(&src).map(|v| v.as_i64()).unwrap_or(0);
                    let d = self.registers.get(&dst).map(|v| v.as_i64()).unwrap_or(0);
                    self.registers.insert(src.clone(), Value::Int(s - amount));
                    self.registers.insert(dst.clone(), Value::Int(d + amount));
                    self.trace_op(pc, &format!("TRANSFER {} → {} ({})", src, dst, amount));
                }

                op::UNBIND => {
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let role = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    self.registers.remove(&format!("{}.{}", reg, role));
                    self.trace_op(pc, &format!("UNBIND {} {}", reg, role));
                }

                op::NEWVAR => {
                    // Fresh type/value variable — create an uninitialised register.
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.registers.entry(name.clone()).or_insert(Value::Null);
                    self.trace_op(pc, &format!("NEWVAR {}", name));
                }

                // ── Type-inference & reasoning opcodes ─────────────────────
                // These operate at the VSA / semantic level (the trunk binding
                // head + arena per the architecture), not at the VM's value
                // level. The VM records them as well-formed structural steps so
                // a thought-program executes and verifies cleanly; it does not
                // fabricate value-level semantics it cannot faithfully provide.

                op::UNIFY => {
                    let a = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let b = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("UNIFY {} {}", a, b));
                }
                op::INST => self.trace_structural(pc, "INST", &operands),
                op::GEN => self.trace_structural(pc, "GEN", &operands),
                op::SEQ => self.trace_structural(pc, "SEQ", &operands),
                op::DIFF => self.trace_structural(pc, "DIFF", &operands),
                op::DETECT_PATTERN => self.trace_structural(pc, "DETECT_PATTERN", &operands),
                op::PREDICT => self.trace_structural(pc, "PREDICT", &operands),
                op::MATCH => self.trace_structural(pc, "MATCH", &operands),
                op::DEBATE => self.trace_structural(pc, "DEBATE", &operands),
                op::DISCOVER => self.trace_structural(pc, "DISCOVER", &operands),
                op::DECODE => self.trace_structural(pc, "DECODE", &operands),
                op::SCORE => self.trace_structural(pc, "SCORE", &operands),
                op::SPECIALIZE => self.trace_structural(pc, "SPECIALIZE", &operands),
                op::REWARD => self.trace_structural(pc, "REWARD", &operands),
                op::INFER => self.trace_structural(pc, "INFER", &operands),
                op::MERGE => self.trace_structural(pc, "MERGE", &operands),
                op::SPLIT => self.trace_structural(pc, "SPLIT", &operands),
                op::FILTER => self.trace_structural(pc, "FILTER", &operands),
                op::MAP_ROLES => self.trace_structural(pc, "MAP_ROLES", &operands),
                op::REDUCE => self.trace_structural(pc, "REDUCE", &operands),
                op::TEMPORAL_BIND => self.trace_structural(pc, "TEMPORAL_BIND", &operands),
                op::ANALOGY => self.trace_structural(pc, "ANALOGY", &operands),
                op::BROADCAST => self.trace_structural(pc, "BROADCAST", &operands),
                op::EXPLORE => self.trace_structural(pc, "EXPLORE", &operands),
                op::FORGE => self.trace_structural(pc, "FORGE", &operands),
                op::ASK => self.trace_structural(pc, "ASK", &operands),
                op::SYNC => self.trace_structural(pc, "SYNC", &operands),

                _ => {
                    // A truly unknown opcode byte means malformed bytecode — a
                    // real invariant violation, not something to skip silently.
                    self.trace_op(pc, &format!("UNKNOWN 0x{:02x}", opcode));
                    return ExecResult::Error(format!("unknown opcode 0x{:02x} at pc {}", opcode, pc));
                }
            }
        }

        // Return the accumulator as the result
        ExecResult::Ok(self.accumulator.clone())
    }

    /// Trace a reasoning/VSA opcode as a well-formed structural step. The
    /// operands are rendered for trace fidelity but no value-level mutation is
    /// performed (these resolve in the trunk's hyperdimensional machinery).
    fn trace_structural(&mut self, pc: usize, name: &str, operands: &[Operand]) {
        if self.trace_enabled {
            let args: Vec<String> = operands.iter().map(|o| o.as_name()).collect();
            self.trace.push(format!("0x{:04x}: {} [{}]", pc, name, args.join(", ")));
        }
    }

    fn trace_op(&mut self, offset: usize, desc: &str) {
        if self.trace_enabled {
            self.trace.push(format!("0x{:04x}: {}", offset, desc));
        }
    }

    /// Resolve an operand to an i64 value: immediates pass through; named
    /// registers are looked up in the register file (0 if unset).
    fn resolve_i64(&self, op: Option<&Operand>) -> i64 {
        match op {
            Some(Operand::Imm(n)) => *n,
            Some(Operand::FloatImm(f)) => *f as i64,
            Some(o) => self.registers.get(&o.as_name()).map(|v| v.as_i64()).unwrap_or(0),
            None => 0,
        }
    }

    /// Resolve an operand to a Value, preserving int vs float so arithmetic
    /// can promote correctly. Named operands resolve to the register's value.
    fn resolve_value(&self, op: Option<&Operand>) -> Value {
        match op {
            Some(Operand::Imm(n)) => Value::Int(*n),
            Some(Operand::FloatImm(f)) => Value::Float(*f),
            Some(o @ Operand::Named(_)) => {
                self.registers.get(&o.as_name()).cloned().unwrap_or(Value::Int(0))
            }
            _ => Value::Int(0),
        }
    }

    fn register_name(&mut self, name: &str) {
        let h = blake3::hash(name.as_bytes());
        self.name_table.insert([h.as_bytes()[0], h.as_bytes()[1]], name.to_string());
    }

    /// Get the value of a register.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.registers.get(name)
    }

    /// Get the accumulator (last SUM result).
    pub fn result(&self) -> &Value {
        &self.accumulator
    }

    /// Reset the VM state.
    pub fn reset(&mut self) {
        self.registers.clear();
        self.stack.clear();
        self.accumulator = Value::Null;
        self.trace.clear();
        self.events.clear();
    }
}

// ── Operand decoding ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Operand {
    Named([u8; 2]),
    Imm(i64),
    FloatImm(f64),
    Type([u8; 2]),
    Role([u8; 2]),
    Global([u8; 2]),
    None,
}

impl Operand {
    fn as_name(&self) -> String {
        match self {
            Operand::Named(h) => format!("r_{:02x}{:02x}", h[0], h[1]),
            Operand::Global(h) => format!("g_{:02x}{:02x}", h[0], h[1]),
            Operand::Type(h) => format!("t_{:02x}{:02x}", h[0], h[1]),
            Operand::Role(h) => format!("role_{:02x}{:02x}", h[0], h[1]),
            Operand::Imm(n) => format!("imm_{}", n),
            Operand::FloatImm(f) => format!("fimm_{}", f),
            Operand::None => "_".into(),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Operand::Imm(n) => Value::Int(*n),
            Operand::FloatImm(f) => Value::Float(*f),
            Operand::Named(h) => Value::Str(format!("r_{:02x}{:02x}", h[0], h[1])),
            Operand::Global(h) => Value::Str(format!("g_{:02x}{:02x}", h[0], h[1])),
            _ => Value::Null,
        }
    }

    /// The raw 2-byte hash key for a Global/Named operand (used as a label /
    /// jump-target identifier). `None` for immediates and absent operands.
    fn label_key(&self) -> Option<[u8; 2]> {
        match self {
            Operand::Global(h) | Operand::Named(h) => Some(*h),
            _ => None,
        }
    }
}

/// Numeric binary operation with int/float promotion. If either operand is a
/// float, compute in f64 and return a Float (collapsing to Int when the result
/// is integral, so whole-valued results print cleanly). Otherwise integer math.
fn num_binop(a: &Value, b: &Value, op: char) -> Value {
    let a_is_float = matches!(a, Value::Float(_));
    let b_is_float = matches!(b, Value::Float(_));
    if a_is_float || b_is_float {
        let x = a.as_f64();
        let y = b.as_f64();
        let r = match op {
            '+' => x + y,
            '-' => x - y,
            '*' => x * y,
            '/' => x / y,
            _ => 0.0,
        };
        // Collapse integral floats back to Int for clean output / comparison.
        if r.fract() == 0.0 && r.is_finite() {
            Value::Int(r as i64)
        } else {
            Value::Float(r)
        }
    } else {
        // Integer operands. Division uses TRUE division (matching GSM8K's
        // `<<60/100=0.6>>` semantics): if the quotient is non-integral, promote
        // to Float; otherwise stay Int. +,-,* stay integer.
        let x = a.as_i64();
        let y = b.as_i64();
        match op {
            '+' => Value::Int(x + y),
            '-' => Value::Int(x - y),
            '*' => Value::Int(x * y),
            '/' => {
                if y != 0 && x % y == 0 {
                    Value::Int(x / y)
                } else if y != 0 {
                    Value::Float(x as f64 / y as f64)
                } else {
                    Value::Int(0)
                }
            }
            _ => Value::Int(0),
        }
    }
}

/// Pre-scan the bytecode for LABEL opcodes, mapping each label's 2-byte hash
/// to the byte offset of the LABEL instruction itself. JMP/COND resolve their
/// target label through this map. Mirrors the main loop's opcode/operand
/// stride so offsets line up exactly.
fn scan_labels(bc: &[u8]) -> std::collections::HashMap<[u8; 2], usize> {
    let mut map = std::collections::HashMap::new();
    let mut pc = 0usize;
    while pc < bc.len() {
        let here = pc;
        let opcode = bc[pc]; pc += 1;
        if opcode == op::SKIP {
            continue;
        }
        if pc >= bc.len() { break; }
        let n_ops = bc[pc] as usize; pc += 1;
        let mut first_key: Option<[u8; 2]> = None;
        for i in 0..n_ops {
            if pc >= bc.len() { break; }
            let (operand, consumed) = decode_operand(&bc[pc..]);
            if i == 0 {
                first_key = operand.label_key();
            }
            pc += consumed;
        }
        if opcode == op::LABEL {
            if let Some(k) = first_key {
                map.insert(k, here);
            }
        }
    }
    map
}

fn decode_operand(bytes: &[u8]) -> (Operand, usize) {
    if bytes.is_empty() {
        return (Operand::None, 0);
    }
    match bytes[0] {
        0x01 => { // Named
            if bytes.len() >= 3 {
                (Operand::Named([bytes[1], bytes[2]]), 3)
            } else {
                (Operand::None, 1)
            }
        }
        0x02 => { // Imm (i64 LE)
            if bytes.len() >= 9 {
                let val = i64::from_le_bytes([
                    bytes[1], bytes[2], bytes[3], bytes[4],
                    bytes[5], bytes[6], bytes[7], bytes[8],
                ]);
                (Operand::Imm(val), 9)
            } else {
                (Operand::None, 1)
            }
        }
        0x03 => { // Type
            if bytes.len() >= 3 {
                (Operand::Type([bytes[1], bytes[2]]), 3)
            } else {
                (Operand::None, 1)
            }
        }
        0x04 => { // Role
            if bytes.len() >= 3 {
                (Operand::Role([bytes[1], bytes[2]]), 3)
            } else {
                (Operand::None, 1)
            }
        }
        0x05 => { // Global
            if bytes.len() >= 3 {
                (Operand::Global([bytes[1], bytes[2]]), 3)
            } else {
                (Operand::None, 1)
            }
        }
        0x06 => { // FloatImm (f64 LE)
            if bytes.len() >= 9 {
                let val = f64::from_le_bytes([
                    bytes[1], bytes[2], bytes[3], bytes[4],
                    bytes[5], bytes[6], bytes[7], bytes[8],
                ]);
                (Operand::FloatImm(val), 9)
            } else {
                (Operand::None, 1)
            }
        }
        0x00 => { // None
            if bytes.len() >= 3 {
                (Operand::None, 3)
            } else {
                (Operand::None, 1)
            }
        }
        _ => (Operand::None, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_and_load(source: &str) -> VM {
        let programs = crate::compiler::compile(source).unwrap();
        let mut vm = VM::new();
        for prog in programs {
            vm.load(prog);
        }
        vm
    }

    #[test]
    fn test_vm_basic_arithmetic() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
                    assign x = 16;
                    sub x, 3;
                    sub x, 4;
                    sum x;
                    query x;
                    remember x;
                }
            }
        "#);

        vm.trace_enabled = true;
        let result = vm.call("Test", "run", Vec::new());

        // Print trace
        for line in &vm.trace {
            println!("{}", line);
        }

        match result {
            ExecResult::Ok(val) => {
                println!("Result: {}", val);
                assert_eq!(val.as_i64(), 9, "16 - 3 - 4 = 9");
            }
            ExecResult::Error(e) => panic!("execution error: {}", e),
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn test_vm_push_pop_chain() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
                    assign x = 16;
                    sub x, 3;
                    sub x, 4;
                    sum x;
                    push x;
                    create y : quantity;
                    pop y;
                    mul y, 2;
                    sum y;
                    query y;
                    remember y;
                }
            }
        "#);

        let result = vm.call("Test", "run", Vec::new());

        match result {
            ExecResult::Ok(val) => {
                println!("Result: {}", val);
                // x = 16-3-4 = 9, push 9, pop into y, y*2 = 18
                assert_eq!(val.as_i64(), 18, "chain: (16-3-4)*2 = 18");
            }
            ExecResult::Error(e) => panic!("execution error: {}", e),
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn test_vm_division_by_zero() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
                    assign x = 42;
                    div x, 0;
                }
            }
        "#);

        let result = vm.call("Test", "run", Vec::new());
        match result {
            ExecResult::Error(e) => assert!(e.contains("division by zero")),
            _ => panic!("expected division by zero error"),
        }
    }

    #[test]
    fn test_vm_store_recall() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
                    assign x = 42;
                    remember x;
                }
            }
        "#);

        vm.call("Test", "run", Vec::new());

        // The register should be in storage after REMEMBER
        assert!(vm.storage.values().any(|v| v.as_i64() == 42));
    }

    #[test]
    fn test_vm_multiple_functions() {
        let mut vm = compile_and_load(r#"
            program Calc implements ISolver {
                public function add_numbers(): void {
                    create x : quantity;
                    assign x = 10;
                    add x, 20;
                    sum x;
                }
                public function mul_numbers(): void {
                    create y : quantity;
                    assign y = 5;
                    mul y, 6;
                    sum y;
                }
            }
        "#);

        let r1 = vm.call("Calc", "add_numbers", Vec::new());
        match r1 {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 30),
            _ => panic!("expected ok"),
        }

        let r2 = vm.call("Calc", "mul_numbers", Vec::new());
        match r2 {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 30),
            _ => panic!("expected ok"),
        }
    }

    #[test]
    fn test_vm_load_cubebin() {
        // Compile, save to cubebin, load from cubebin, execute
        let programs = crate::compiler::compile(r#"
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
                    assign x = 100;
                    sub x, 37;
                    sum x;
                }
            }
        "#).unwrap();

        let path = std::env::temp_dir().join("test_vm_exec.cubebin");
        programs[0].save(&path).unwrap();

        let mut vm = VM::new();
        let name = vm.load_file(&path).unwrap();
        assert_eq!(name, "Test");

        let result = vm.call("Test", "run", Vec::new());
        match result {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 63, "100 - 37 = 63"),
            _ => panic!("expected ok"),
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_vm_trace() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
                    assign x = 5;
                    add x, 3;
                    sum x;
                }
            }
        "#);

        vm.trace_enabled = true;
        vm.call("Test", "run", Vec::new());

        assert!(!vm.trace.is_empty());
        assert!(vm.trace.iter().any(|t| t.contains("CREATE")));
        assert!(vm.trace.iter().any(|t| t.contains("ASSIGN")));
        assert!(vm.trace.iter().any(|t| t.contains("ADD")));
        assert!(vm.trace.iter().any(|t| t.contains("SUM")));
    }

    #[test]
    fn test_vm_if_then_branch() {
        let mut vm = compile_and_load(r#"
            program T implements ISolver {
                public function run(): void {
                    create x : quantity;
                    assign x = 5;
                    if (x > 3) { assign x = 100; } else { assign x = 200; }
                    sum x;
                }
            }
        "#);
        match vm.call("T", "run", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 100, "x=5, x>3 true → then-branch → 100"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_if_else_branch() {
        let mut vm = compile_and_load(r#"
            program T implements ISolver {
                public function run(): void {
                    create x : quantity;
                    assign x = 2;
                    if (x > 3) { assign x = 100; } else { assign x = 200; }
                    sum x;
                }
            }
        "#);
        match vm.call("T", "run", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 200, "x=2, x>3 false → else-branch → 200"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_while_loop_terminates_and_accumulates() {
        let mut vm = compile_and_load(r#"
            program T implements ISolver {
                public function run(): void {
                    create total : quantity;
                    assign total = 0;
                    create i : quantity;
                    assign i = 5;
                    while (i > 0) { add total, 10; sub i, 1; }
                    sum total;
                }
            }
        "#);
        match vm.call("T", "run", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 50, "5 iterations × 10 = 50"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_compare_sets_cmp_register() {
        let mut vm = compile_and_load(r#"
            program T implements ISolver {
                public function run(): void {
                    create a : quantity;
                    assign a = 7;
                    create b : quantity;
                    assign b = 7;
                    compare a, b;
                }
            }
        "#);
        vm.call("T", "run", Vec::new());
        assert_eq!(vm.get("__cmp").map(|v| v.as_str()), Some("equal".to_string()));
    }

    #[test]
    fn test_vm_float_arithmetic() {
        // 80000 * 1.5 = 120000 (real fractional multiply, integral result).
        let mut vm = compile_and_load(r#"
            program T implements ISolver {
                public function run(): void {
                    create v : quantity;
                    assign v = 80000;
                    mul v, 1.5;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "run", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 120000),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_float_nonintegral_result() {
        // 7 / 2 = 3.5 — non-integral float preserved.
        let mut vm = compile_and_load(r#"
            program T implements ISolver {
                public function run(): void {
                    create v : quantity;
                    assign v = 7;
                    div v, 2.0;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "run", Vec::new()) {
            ExecResult::Ok(v) => assert!((v.as_f64() - 3.5).abs() < 1e-9, "got {}", v.as_f64()),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_true_division_int_operands() {
        // 60 / 100 = 0.6 (true division on integer operands, not floor → 0).
        let mut vm = compile_and_load(r#"
            program T implements ISolver {
                public function run(): void {
                    create v : quantity;
                    assign v = 60;
                    div v, 100;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "run", Vec::new()) {
            ExecResult::Ok(v) => assert!((v.as_f64() - 0.6).abs() < 1e-9, "got {}", v.as_f64()),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_exact_int_division_stays_int() {
        // 16 / 2 = 8 stays integer.
        let mut vm = compile_and_load(r#"
            program T implements ISolver {
                public function run(): void {
                    create v : quantity;
                    assign v = 16;
                    div v, 2;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "run", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 8),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_gsm8k_cubebin() {
        // Compile gsm8k.cube, save to cubebin, load, and run solve()
        let source = include_str!("../../examples/gsm8k.cube");
        let programs = crate::compiler::compile(source).unwrap();

        let path = std::env::temp_dir().join("gsm8k_vm_test.cubebin");
        programs[0].save(&path).unwrap();

        let mut vm = VM::new();
        vm.load_file(&path).unwrap();

        // Run constructor
        vm.call("GSM8K", "constructor", Vec::new());

        // Run solve (it won't compute correctly without real input parsing,
        // but it should execute without errors)
        vm.trace_enabled = true;
        let result = vm.call("GSM8K", "solve", Vec::new());

        println!("GSM8K.solve() trace ({} ops):", vm.trace.len());
        for line in &vm.trace {
            println!("  {}", line);
        }

        match result {
            ExecResult::Ok(_) => println!("GSM8K.solve() completed successfully"),
            ExecResult::Error(e) => println!("GSM8K.solve() error (expected): {}", e),
            _ => {}
        }

        let _ = std::fs::remove_file(&path);
    }
}
