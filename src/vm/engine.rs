//! CubeLang VM — loads and executes `.cubebin` bytecodes.
//!
//! The VM maintains registers, a stack, and storage. It decodes
//! bytecodes instruction-by-instruction and executes them.

use std::collections::HashMap;
use crate::compiler::{CompiledProgram, CompiledFunction, op};
use crate::vm::memory::HippocampalMemory;
use crate::vm::codebook::Codebook;
use crate::vm::hypervec::Hypervec;

/// Hypervector dimension for the VM's hippocampal memory. This is CubeLang's
/// own working value -- it does NOT match opcode-vsa-rs's own default (8192,
/// `hypervec::DEFAULT_DIM`); cross-repo dimension reconciliation is out of
/// scope here. Quasi-orthogonality of distinct keys holds comfortably at
/// this width.
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
    /// A bipolar hypervector value (VSA role-filler binding result). Lives in
    /// the same {-1,+1}^MEM_DIM space as the hippocampal memory's addresses.
    Hvec(Hypervec),
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
            Value::Hvec(h) => write!(f, "<hvec d={}>", h.dim()),
        }
    }
}

/// VM execution result.
#[derive(Debug)]
pub enum ExecResult {
    Ok(Value),
    Return(Value),
    /// The program cannot proceed without information only the caller has.
    ///
    /// This is NOT an error and NOT a result: it is a third outcome. A program
    /// that has grounded several real candidates and cannot choose between them
    /// must neither pick one (that is a confabulation) nor fail (nothing went
    /// wrong). It suspends, hands the question outward, and resumes when told.
    ///
    /// Resumption is cheap because registers/storage/memory/accumulator are
    /// VM-global; only the interpreter's *locals* (pc, flag, jump_budget) need
    /// capturing. See `VM::resume`.
    Suspend(Suspension),
    Error(String),
}

/// Everything needed to continue a suspended program, plus what to ask.
#[derive(Debug, Clone)]
pub struct Suspension {
    /// The question, as the program phrased it (ASK's first operand).
    pub question: Value,
    /// Candidate answers the program is choosing between (ASK's remaining operands).
    pub candidates: Vec<Value>,
    /// Interpreter locals, captured so `resume` can pick up mid-loop.
    pub pc: usize,
    pub flag: bool,
    pub jump_budget: u64,
    /// Which function was executing (resume re-enters it).
    pub function: String,
    /// Which program owns that function. Captured because `call()` restores
    /// `current_program` on the way out, so the VM has forgotten it by the time
    /// the host sees the Suspend. A suspension must be self-describing.
    pub program: String,
}

/// The CubeLang virtual machine.
pub struct VM {
    /// Grounded facts, keyed by canonical surface form. Backs `QUERY`.
    /// Empty by default: a VM with no knowledge abstains on every query, which
    /// is the correct behaviour for a VM with no knowledge.
    pub knowledge: crate::vm::knowledge::KnowledgeStore,
    /// Set by `resume()`; consumed by the next `exec_function` entry to restore
    /// interpreter locals (pc/flag/jump_budget) after an ASK suspension.
    resume_state: Option<Suspension>,
    /// Named registers.
    pub registers: HashMap<String, Value>,
    /// Stack for PUSH/POP chaining.
    pub stack: Vec<Value>,
    /// Persistent storage (STORE/RECALL) — exact-key fast path.
    pub storage: HashMap<String, Value>,
    /// Hippocampal associative memory (VSA cleanup) for STORE/RECALL/REMEMBER/FORGET.
    pub memory: HippocampalMemory,
    /// Symbol/role codebook for VSA role-filler binding (BIND_ROLE/UNBIND).
    /// Deterministic symbol -> hypervector at MEM_DIM/MEM_SEED, so bound
    /// structures are reproducible across runs and share the memory's space.
    vsa: Codebook,
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
    /// Plan-2: name of the program whose function is currently executing. Set
    /// by `call()`; consulted by op::CALL to resolve sibling-function names.
    current_program: Option<String>,
    /// Plan-2: intra-program call recursion depth. Capped at MAX_CALL_DEPTH
    /// so runaway recursion surfaces as an Error rather than a native overflow.
    call_depth: u32,
    /// Task 3: the VM-internal module/capability registry. Built once at
    /// `VM::new` (cheap, deterministic) and consulted by `op::CALL` for
    /// override/registry-impl resolution. See `vm::registry` for the
    /// deny-by-default access model and the override precedence rule.
    registry: crate::vm::registry::ModuleRegistry,
}

/// Conservative recursion cap. exec_function has a sizeable per-frame
/// footprint (registers/stack clones, operands buffer, label_map clone via
/// scan), so 64 is comfortably below the native test-thread stack limit while
/// being plenty for hand-written CubeLang programs.
const MAX_CALL_DEPTH: u32 = 64;

impl VM {
    pub fn new() -> Self {
        let mut vm = Self {
            registers: HashMap::new(),
            stack: Vec::new(),
            storage: HashMap::new(),
            memory: HippocampalMemory::new(MEM_DIM, MEM_SEED),
            vsa: Codebook::with_dim(MEM_SEED, MEM_DIM),
            accumulator: Value::Null,
            events: Vec::new(),
            programs: HashMap::new(),
            trace: Vec::new(),
            trace_enabled: false,
            name_table: HashMap::new(),
            current_program: None,
            call_depth: 0,
            resume_state: None,
            knowledge: crate::vm::knowledge::KnowledgeStore::new(),
            registry: crate::vm::registry::ModuleRegistry::new(),
        };
        // Task 3: pre-register every registry function name into the
        // hash-reversal table, regardless of which modules any given
        // program `use`s. This is NOT a grant of access -- deny-by-default
        // is enforced separately, by op::CALL gating resolution on the
        // calling program's own `used_modules` -- it only makes a name the
        // compiler already hashed (because the source literally called it)
        // reversible to a string the VM can look up in the registry at all.
        let registry_names: Vec<String> =
            vm.registry.all_function_names().map(str::to_string).collect();
        for name in &registry_names {
            vm.register_name(name);
        }
        vm
    }

    /// Resume a program suspended by ASK, supplying the caller's answer.
    ///
    /// THE CALLER IS THE MODEL, NOT A HUMAN. The trunk renders `susp.question`
    /// and `susp.candidates` into language, the user replies in language, and
    /// the trunk maps that reply back to ONE OF THE CANDIDATES. The VM never
    /// talks to a person, and a person never picks an index.
    ///
    /// That division is what keeps the guarantee: the trunk supplies syntax
    /// (rendering) and selection (mapping), never content. So the selection is
    /// CHECKABLE, and this function checks it -- `answer` must be *identical*
    /// to a candidate, not merely plausible. A trunk that returns 1968 when the
    /// candidates are {1959, 1969} has confabulated a value into a slot that
    /// could only ever hold one of two, and is rejected here rather than
    /// silently believed.
    ///
    /// The answer is PUSHED ONTO THE STACK, matching how CALL delivers a
    /// callee's return value. The resumed program binds it with `pop <reg>`.
    /// (An earlier draft used the accumulator; wrong, because `SUM x` overwrites
    /// the accumulator from register x.)
    ///
    /// Resumption needs no call frames: registers/storage/memory are VM-global,
    /// so `Suspension` carries back only the interpreter locals (pc, flag,
    /// jump_budget).
    pub fn resume(&mut self, susp: Suspension, answer: Value) -> ExecResult {
        // Structural guard: the selection must be one the program offered.
        // Deliberately NOT `derive(PartialEq)` on Value -- Hvec holds floats and
        // "equal hypervectors" is a similarity question, not an equality one.
        //
        // Compared structurally, because a QUERY chunk is a Map: the trunk maps
        // the user's reply back to one of the offered chunks and hands that Map
        // straight back. A scalar-only comparison would reject every possible
        // answer and make ASK unusable on grounded facts.
        fn same_selection(a: &Value, b: &Value) -> bool {
            match (a, b) {
                (Value::Int(x), Value::Int(y)) => x == y,
                (Value::Str(x), Value::Str(y)) => x == y,
                (Value::Bool(x), Value::Bool(y)) => x == y,
                (Value::Float(x), Value::Float(y)) => x == y,
                (Value::Null, Value::Null) => true,
                (Value::Array(x), Value::Array(y)) => {
                    x.len() == y.len()
                        && x.iter().zip(y).all(|(i, j)| same_selection(i, j))
                }
                (Value::Map(x), Value::Map(y)) => {
                    x.len() == y.len()
                        && x.iter().all(|(k, v)| {
                            y.get(k).map_or(false, |w| same_selection(v, w))
                        })
                }
                // Hypervectors are never a *selection*; equality is the wrong
                // question to ask of them.
                _ => false,
            }
        }
        if !susp.candidates.is_empty() && !susp.candidates.iter().any(|c| same_selection(c, &answer)) {
            return ExecResult::Error(format!(
                "resume: answer {} is not among the {} candidates the program offered \
                 (a selection must be chosen, not invented)",
                answer,
                susp.candidates.len()
            ));
        }
        let prog_name = susp.program.clone();
        let prog = match self.programs.get(&prog_name) {
            Some(p) => p.clone(),
            None => return ExecResult::Error(format!("resume: program not found: {}", prog_name)),
        };
        let func = match prog.functions.iter().find(|f| f.name == susp.function) {
            Some(f) => f.clone(),
            None => {
                return ExecResult::Error(format!(
                    "resume: function not found: {}.{}",
                    prog_name, susp.function
                ))
            }
        };
        // Deliver the answer the same way CALL delivers a result: via the stack.
        self.stack.push(answer);
        // A nested ASK inside the resumed run must report the same program.
        let prev = self.current_program.replace(prog_name);
        self.resume_state = Some(susp);
        let result = self.exec_function(&func);
        self.current_program = prev;
        result
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
        // Role + filler symbol names used by BIND_ROLE/UNBIND, so their 2-byte
        // hashes in the bytecode reverse to the real strings the VSA codebook
        // keys on (enabling genuine, human-readable role-filler binding).
        for sym in &prog.symbol_names {
            self.register_name(sym);
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

        // Bind host arguments to registers.
        //
        // Previously this ONLY synthesized `arg0`, `arg1`, ... because
        // CompiledFunction discarded the declared parameter names at compile
        // time. So a function written as `solve(mention: str)` read an UNBOUND
        // register when called from a host -- silently, since an unbound
        // register resolves to Int(0)/Null rather than erroring. Every example
        // in the SPEC hits this.
        //
        // Now: bind by DECLARED NAME (v3 .cubebin carries them), and keep the
        // `argN` alias so v2 files and existing callers still work.
        for (i, arg) in args.into_iter().enumerate() {
            if let Some(param) = func.params.get(i) {
                self.register_name(param);
                self.registers.insert(Self::register_key(param), arg.clone());
            }
            let alias = format!("arg{}", i);
            self.register_name(&alias);
            self.registers.insert(Self::register_key(&alias), arg);
        }

        // Plan-2: remember which program owns the running function so an
        // intra-program CALL can look up siblings by name.
        let prev = self.current_program.replace(program.to_string());
        let result = self.exec_function(&func);
        self.current_program = prev;
        result
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

        // Resuming a suspended program: restore the interpreter locals that
        // `Suspension` captured. Registers/storage/memory/accumulator are
        // VM-global and were never lost. `pc` already points PAST the ASK
        // instruction, so execution continues with the answer in scope.
        if let Some(r) = self.resume_state.take() {
            pc = r.pc;
            flag = r.flag;
            jump_budget = r.jump_budget;
        }

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

                op::MAKE_ARRAY => {
                    // Plan-3. operands: [dst, n_imm, e0, e1, ..., e_{n-1}].
                    // n is taken from the Imm; we then collect exactly that
                    // many element operands. If the operand stream is short
                    // (malformed bytecode) we fill with Null.
                    let dst = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let n = match operands.get(1) {
                        Some(Operand::Imm(k)) => (*k).max(0) as usize,
                        _ => 0,
                    };
                    let mut items: Vec<Value> = Vec::with_capacity(n);
                    for i in 0..n {
                        items.push(self.resolve_assign_rhs(operands.get(2 + i)));
                    }
                    self.trace_op(pc, &format!("MAKE_ARRAY {} ({} items)", dst, n));
                    self.registers.insert(dst, Value::Array(items));
                }

                op::LEN => {
                    // operands: [dst, arr]
                    let dst = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let arr = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let len = match self.registers.get(&arr) {
                        Some(Value::Array(v)) => v.len() as i64,
                        Some(Value::Str(s)) => s.len() as i64,
                        _ => 0,
                    };
                    self.trace_op(pc, &format!("LEN {} = len({}) → {}", dst, arr, len));
                    self.registers.insert(dst, Value::Int(len));
                }

                op::INDEX => {
                    // operands: [dst, arr, i]. Out-of-range yields Null so
                    // bad indices surface as untrusted data rather than crash.
                    let dst = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let arr = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let i = self.resolve_i64(operands.get(2));
                    let val = match self.registers.get(&arr) {
                        Some(Value::Array(v)) if i >= 0 && (i as usize) < v.len() => v[i as usize].clone(),
                        _ => Value::Null,
                    };
                    self.trace_op(pc, &format!("INDEX {} = {}[{}] → {}", dst, arr, i, val));
                    self.registers.insert(dst, val);
                }

                op::RETURN => {
                    // Plan-1: early return. With 0 operands, yield the current
                    // accumulator. With 1 operand, resolve it (literal or
                    // register) into the accumulator before exiting.
                    if let Some(o) = operands.get(0) {
                        self.accumulator = self.resolve_value(Some(o));
                    }
                    self.trace_op(pc, &format!("RETURN {}", self.accumulator));
                    return ExecResult::Return(self.accumulator.clone());
                }

                op::ASSIGN => {
                    // P0-2: a Named/Global operand must resolve to the source
                    // register's VALUE, not the synthesized register-name string.
                    // Without this, `assign b = a` stored `Value::Str("r_xxxx")`,
                    // so arithmetic/sum on b returned 0. Literal Imm/FloatImm
                    // assignment behaviour is preserved (see resolve_assign_rhs).
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.resolve_assign_rhs(operands.get(1));
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
                    // QUERY <mention>
                    //
                    // Was: read a register, write a trace line, retrieve nothing.
                    // The SPEC has always typed this as
                    //   rag { chunks: array<{text, score, source}> }
                    // -- provenance in the type, waiting for an implementation.
                    //
                    // Now: an EXACT lookup against the VM's knowledge store. The
                    // chunk array is pushed onto the stack (same convention as
                    // CALL and ASK), so `pop ctx` binds it.
                    //
                    // Zero embeddings. A canonical title IS a key: DBpedia's
                    // 4.6M entities normalize to 4,628,421 unique titles at 0.2%
                    // collisions. Embedding the query would cost a forward pass
                    // per lookup -- a linear marginal cost, which is exactly the
                    // economics this architecture exists to avoid.
                    //
                    // A MISS PUSHES AN EMPTY ARRAY. Not an Error (nothing
                    // failed), and never a nearest neighbour -- that is how you
                    // get "the capital of France is Kyiv" at 0.699. Zero chunks
                    // is the program's cue to say it does not know. The caller
                    // branches on `len`:
                    //     0 -> gap, RETURN     1 -> fact, RETURN     N -> ASK
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let mention = match self.registers.get(&reg) {
                        // a register holding a string is the usual case
                        Some(Value::Str(s)) => s.clone(),
                        // otherwise fall back to the operand's own symbol name,
                        // so `query "printing press";` works as a literal
                        _ => self.resolve_symbol_name(operands.get(0)),
                    };

                    let chunks: Vec<Value> = self
                        .knowledge
                        .lookup(&mention)
                        .iter()
                        .map(|f| {
                            let mut m = HashMap::new();
                            m.insert("text".to_string(), Value::Str(f.text.clone()));
                            // An exact key match is confidence 1.0, not a
                            // similarity. A cosine neighbour at 0.87 is a guess
                            // and must never wear the same number.
                            m.insert("score".to_string(), Value::Float(1.0));
                            m.insert("source".to_string(), Value::Str(f.source.clone()));
                            Value::Map(m)
                        })
                        .collect();

                    self.trace_op(pc, &format!(
                        "QUERY {:?} -> {} chunk(s)", mention, chunks.len()));
                    self.stack.push(Value::Array(chunks));
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
                    // Genuine VSA role-filler binding. The register key is the
                    // operand's display token (matching every other op's
                    // register addressing); the role and filler resolve to
                    // their real symbol strings (via the name table) so the
                    // codebook produces stable, human-recoverable vectors.
                    // reg <- bundle(reg, bind(role, filler)).
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let role = self.resolve_symbol_name(operands.get(1));
                    let filler = self.resolve_symbol_name(operands.get(2));
                    self.trace_op(pc, &format!("BIND_ROLE {} {} {}", reg, role, filler));
                    self.vsa_bind_into(&reg, &role, &filler);
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
                    // Plan-2 intra-program CALL, extended by Task 3 with
                    // `use`/`override` resolution.
                    //   operands[0] = global(fn_name); operands[1..] = arg values.
                    //
                    // Resolution precedence (design doc, "THE CRUX"):
                    //   1. the program's own function, if it's a COMPILE-TIME-
                    //      VALIDATED `override` (CompiledFunction::is_override)
                    //      AND a `use`'d module still exposes `fn_name` as
                    //      overridable  -> the program's override wins.
                    //   2. else, a `use`'d module exposing `fn_name` at all
                    //      -> the registry's native implementation.
                    //   3. else, the program's own function (today's
                    //      pre-Task-3 behaviour, unchanged) -> intra-program
                    //      bytecode call.
                    //   4. else -> error. Deny-by-default: a module NOT
                    //      named in `prog.used_modules` is invisible at step
                    //      2 even if the registry otherwise knows it
                    //      (`ModuleRegistry::resolve`).
                    let key = operands.get(0).and_then(|o| match o {
                        Operand::Global(h) | Operand::Named(h) => Some(*h),
                        _ => None,
                    });
                    let fn_name = match key.and_then(|h| self.name_table.get(&h).cloned()) {
                        Some(n) => n,
                        None => {
                            self.trace_op(pc, "CALL (unresolved name)");
                            return ExecResult::Error("CALL: unresolved function name".into());
                        }
                    };

                    if self.call_depth >= MAX_CALL_DEPTH {
                        return ExecResult::Error(format!(
                            "call depth exceeded ({}); recursion guard tripped at {}",
                            MAX_CALL_DEPTH, fn_name));
                    }

                    let prog_name = match self.current_program.clone() {
                        Some(p) => p,
                        None => return ExecResult::Error(
                            "CALL: no current program context".into()),
                    };
                    let prog = match self.programs.get(&prog_name) {
                        Some(p) => p.clone(),
                        None => return ExecResult::Error(format!(
                            "CALL: program `{}` not loaded", prog_name)),
                    };

                    // Resolve args up front -- shared by every branch below.
                    // Each operand becomes a Value via the same path used by
                    // ASSIGN's RHS (Imm/Float/Named/Global).
                    let mut arg_values: Vec<Value> = Vec::new();
                    for i in 1..operands.len() {
                        arg_values.push(self.resolve_assign_rhs(Some(&operands[i])));
                    }

                    let own_fn = prog.functions.iter().find(|f| f.name == fn_name).cloned();
                    // (bool, fn ptr) is Copy: pulling both out of the
                    // registry entry here ends the immutable borrow of
                    // `self.registry` before any branch below needs
                    // `&mut self` again.
                    let module_hit: Option<(bool, crate::vm::registry::NativeFn)> = self
                        .registry
                        .resolve(&prog.used_modules, &fn_name)
                        .map(|entry| (entry.overridable, entry.call));
                    let is_valid_override = own_fn.as_ref().is_some_and(|f| f.is_override)
                        && module_hit.is_some_and(|(overridable, _)| overridable);

                    if is_valid_override {
                        let callee = own_fn.expect("is_valid_override implies own_fn is Some");
                        self.trace_op(pc, &format!(
                            "CALL {} (program override, {} arg(s))", fn_name, arg_values.len()));
                        if let Some(err) = self.exec_intra_call(&callee, arg_values) {
                            return err;
                        }
                    } else if let Some((_, native)) = module_hit {
                        // Task 6: a registry (native) function gets its OWN
                        // argument resolution, `resolve_registry_arg` --
                        // deliberately NOT the shared `arg_values` above
                        // (still used unchanged by the other two branches).
                        // See that method's doc for why: a native module
                        // like `vsa` is symbol- and register-KEY aware by
                        // nature (`recover(frame, SUBJECT)` names a
                        // register and a role, it does not want their
                        // resolved VALUES), matching how BIND_ROLE/UNBIND's
                        // own bytecode arms already read their operands.
                        let native_args: Vec<Value> = (1..operands.len())
                            .map(|i| self.resolve_registry_arg(Some(&operands[i])))
                            .collect();
                        self.trace_op(pc, &format!(
                            "CALL {} (registry impl, {} arg(s))", fn_name, native_args.len()));
                        let result = native(self, &native_args);
                        self.stack.push(result);
                    } else if let Some(callee) = own_fn {
                        self.trace_op(pc, &format!("CALL {} ({} arg(s))", fn_name, arg_values.len()));
                        if let Some(err) = self.exec_intra_call(&callee, arg_values) {
                            return err;
                        }
                    } else {
                        return ExecResult::Error(format!(
                            "CALL: function `{}` not found in program `{}` (and no `use`'d \
                             module exposes it)", fn_name, prog_name));
                    }
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
                    // Genuine VSA unbind + cleanup. Recover the filler bound to
                    // `role` in the register's bundled hypervector and leave the
                    // recovered symbol (as a Str) in the register. Candidates are
                    // the known symbol names (name table); the cosine-nearest
                    // wins. A register that never held an Hvec is left untouched.
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let role = self.resolve_symbol_name(operands.get(1));
                    // Snapshot candidate symbol strings up front to avoid holding
                    // an immutable borrow of name_table across the &mut self call.
                    let candidates: Vec<String> = self.name_table.values().cloned().collect();
                    let cand_refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
                    match self.vsa_unbind_cleanup(&reg, &role, &cand_refs) {
                        Some((sym, sim)) => {
                            self.trace_op(pc, &format!("UNBIND {} {} → {} (sim {:.3})", reg, role, sym, sim));
                            self.registers.insert(reg, Value::Str(sym));
                        }
                        None => {
                            self.trace_op(pc, &format!("UNBIND {} {} → (no hvec)", reg, role));
                        }
                    }
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
                op::ASK => {
                    // ASK <question>, <candidate>...
                    //
                    // Suspend: hand the question outward and stop. This is the
                    // THIRD outcome, alongside Ok and Error. A program holding
                    // several grounded candidates must not pick one (that is a
                    // confabulation) nor fail (nothing went wrong). It asks.
                    //
                    // `pc` already points past this instruction, so `resume`
                    // continues from here; the answer arrives on the STACK
                    // (same convention as CALL), bound by a following `pop`.
                    // The question is a string literal, which compiles to
                    // Operand::Global (a 2-byte hash). `resolve_symbol_name`
                    // reverses it via the name table populated on load.
                    // Falls back to the `g_xxxx` token if it was never
                    // registered -- deterministic, just not human-readable.
                    let question = Value::Str(self.resolve_symbol_name(operands.get(0)));
                    let mut candidates: Vec<Value> = operands
                        .iter()
                        .skip(1)
                        .map(|o| self.resolve_value(Some(o)))
                        .collect();
                    // `ask "which?", hits;` where `hits` is the array QUERY
                    // pushed means "choose among these", not "here is one
                    // candidate that happens to be a list". Flatten it, or the
                    // caller can never name a selection and `resume`'s guard
                    // rejects every possible answer.
                    if candidates.len() == 1 {
                        if let Value::Array(items) = &candidates[0] {
                            candidates = items.clone();
                        }
                    }
                    self.trace_op(pc, &format!("ASK {} ({} candidates)", question, candidates.len()));
                    return ExecResult::Suspend(Suspension {
                        question,
                        candidates,
                        pc,
                        flag,
                        jump_budget,
                        function: func.name.clone(),
                        program: self.current_program.clone().unwrap_or_default(),
                    });
                }
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

    /// Execute an intra-program bytecode callee exactly as `op::CALL` always
    /// has (Plan-2, factored out unchanged by Task 3 so the SAME snapshot/
    /// restore/depth-guard logic backs both call sites that need it now: a
    /// program's own plain function, and a validated `override`). Installs a
    /// fresh `arg0..argN` register scope, executes, restores the caller's
    /// registers, and on success pushes the result onto `self.stack` (the
    /// caller delivers it into a binding via POP) -- matching how CALL
    /// always has.
    ///
    /// Returns `None` on success (result already on the stack). Returns
    /// `Some(ExecResult::Error(..))` on failure -- the caller should
    /// `return` it immediately, matching the early-return control flow this
    /// replaces. A callee that suspends (ASK) is itself an error here, for
    /// the same reason it always was: by the time it would propagate, the
    /// caller's registers are already restored and call_depth already
    /// unwound, so resuming would re-enter the callee with the wrong frame.
    fn exec_intra_call(&mut self, callee: &CompiledFunction, arg_values: Vec<Value>) -> Option<ExecResult> {
        let caller_regs = std::mem::take(&mut self.registers);
        for (i, v) in arg_values.into_iter().enumerate() {
            let arg_name = format!("arg{}", i);
            self.register_name(&arg_name);
            let h = blake3::hash(arg_name.as_bytes());
            let key = format!("r_{:02x}{:02x}", h.as_bytes()[0], h.as_bytes()[1]);
            self.registers.insert(key, v);
        }

        self.call_depth += 1;
        let r = self.exec_function(callee);
        self.call_depth -= 1;

        // Restore caller-visible registers.
        self.registers = caller_regs;

        match r {
            ExecResult::Ok(v) | ExecResult::Return(v) => {
                self.stack.push(v);
                None
            }
            ExecResult::Error(e) => Some(ExecResult::Error(e)),
            // Lifting this needs real call frames (the deferred `v2` in
            // CUBELANG_OPCODES_PLAN.md) so a Suspension can capture the
            // whole stack, not one function's locals. ASK at the top level
            // of `solve` works today.
            ExecResult::Suspend(s) => Some(ExecResult::Error(format!(
                "ASK inside a CALL is not supported (in {}): the flat \
                 global-register model cannot capture a callee frame. \
                 Move the ASK to the top-level function, or implement \
                 real call frames.",
                s.function
            ))),
        }
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
    /// Global operands (string literals -- e.g. `return "ok";`) resolve via
    /// `resolve_symbol_name`, the same name-table path ASK's question text
    /// and BIND_ROLE/UNBIND's role/filler symbols already use to reverse a
    /// 2-byte hash to its real spelling. Without this arm, RETURN fell
    /// through to the wildcard below and silently returned `Int(0)` for any
    /// string literal. `resolve_symbol_name` falls back to the `g_xxxx`
    /// token only for a genuinely unrecorded literal -- not the normal case,
    /// since `emit_global` always records the spelling before emitting the
    /// operand.
    fn resolve_value(&self, op: Option<&Operand>) -> Value {
        match op {
            Some(Operand::Imm(n)) => Value::Int(*n),
            Some(Operand::FloatImm(f)) => Value::Float(*f),
            Some(o @ Operand::Named(_)) => {
                self.registers.get(&o.as_name()).cloned().unwrap_or(Value::Int(0))
            }
            Some(o @ Operand::Global(_)) => Value::Str(self.resolve_symbol_name(Some(o))),
            _ => Value::Int(0),
        }
    }

    /// Resolve an ASSIGN right-hand-side. Like `resolve_value` for Named (copy
    /// register value), but for Global we fall back to the synthesized name
    /// token when no register is bound — preserving `assign x = "literal"`
    /// behaviour (string literals compile to Global). Imm/FloatImm unchanged.
    fn resolve_assign_rhs(&self, op: Option<&Operand>) -> Value {
        match op {
            Some(Operand::Imm(n)) => Value::Int(*n),
            Some(Operand::FloatImm(f)) => Value::Float(*f),
            Some(o @ Operand::Named(_)) => {
                self.registers.get(&o.as_name()).cloned().unwrap_or(Value::Null)
            }
            Some(o @ Operand::Global(_)) => {
                let key = o.as_name();
                self.registers.get(&key).cloned().unwrap_or(Value::Str(key))
            }
            Some(other) => other.to_value(),
            None => Value::Null,
        }
    }

    fn register_name(&mut self, name: &str) {
        let h = blake3::hash(name.as_bytes());
        self.name_table.insert([h.as_bytes()[0], h.as_bytes()[1]], name.to_string());
    }

    /// The register key a symbol compiles to: `r_` + the first two bytes of its
    /// blake3 hash, matching `Operand::Named(h).as_name()`. Hosts must use this
    /// to reach a register the bytecode refers to by name.
    pub fn register_key(name: &str) -> String {
        let h = blake3::hash(name.as_bytes());
        format!("r_{:02x}{:02x}", h.as_bytes()[0], h.as_bytes()[1])
    }

    /// Reverse a Named/Role/Global operand's 2-byte hash to the original symbol
    /// string via the name table (populated from the program's symbol_names on
    /// load). Falls back to the operand's display token (`role_xxyy` etc.) when
    /// the name was never registered — so binding is still deterministic, just
    /// not human-readable. Used by BIND_ROLE/UNBIND to drive the VSA codebook.
    fn resolve_symbol_name(&self, op: Option<&Operand>) -> String {
        match op {
            Some(Operand::Role(h)) | Some(Operand::Named(h)) | Some(Operand::Global(h)) => {
                self.name_table.get(h).cloned().unwrap_or_else(|| {
                    op.map(|o| o.as_name()).unwrap_or_default()
                })
            }
            Some(o) => o.as_name(),
            None => String::new(),
        }
    }

    /// Task 6: resolve a CALL argument bound for a REGISTRY (native)
    /// function -- distinct from `resolve_assign_rhs` (still used, unchanged,
    /// for ASSIGN and for intra-program/`override` CALL args -- see
    /// `op::CALL`'s dispatch, which computes a SEPARATE argument list only
    /// for the registry-native branch via this method).
    ///
    /// A native module is symbol- and register-KEY aware by nature:
    /// `recover(frame, SUBJECT)` means "the register named frame, the role
    /// named SUBJECT", not "frame's current value, SUBJECT's current
    /// value" -- exactly how `BIND_ROLE`/`UNBIND`'s own bytecode arms
    /// already read their operands (`resolve_symbol_name`, never a
    /// register-value lookup, for a role/reg operand). Reusing that same
    /// resolution here is what lets `vsa::recover` genuinely perform
    /// UNBIND+cleanup instead of receiving `Value::Null` for a bareword
    /// like `SUBJECT` that was never meant to be a register in the first
    /// place -- `resolve_assign_rhs`'s plain register-value lookup has no
    /// way to produce that string, since nothing ever binds a register
    /// literally named `SUBJECT`.
    ///
    /// Imm/FloatImm literals still pass through as real Int/Float values
    /// (unlike `resolve_symbol_name` alone, whose fallback would stringify
    /// them as `imm_5`/`fimm_5`) -- kept general enough for a future
    /// registry function that wants an actual number, not just names.
    fn resolve_registry_arg(&self, op: Option<&Operand>) -> Value {
        match op {
            Some(Operand::Imm(n)) => Value::Int(*n),
            Some(Operand::FloatImm(f)) => Value::Float(*f),
            Some(o) => Value::Str(self.resolve_symbol_name(Some(o))),
            None => Value::Null,
        }
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

    // ── Genuine VSA role-filler binding ──────────────────────────────────
    // These give BIND_ROLE/UNBIND real hyperdimensional semantics: a register
    // holds a *bundled hypervector* of bind(role, filler) terms, and a role is
    // recovered by unbind + codebook cleanup -- the bind(sentence, inv(ROLE))
    // round-trip from the VSA/HDC literature. Built on the bind/bundle/cosine
    // primitives in hypervec.rs; the codebook (self.vsa) maps role and filler
    // symbols to deterministic quasi-orthogonal hypervectors.

    /// The hypervector for a filler symbol (deterministic via the codebook).
    pub fn vsa_symbol_vec(&mut self, symbol: &str) -> Hypervec {
        self.vsa.get_cloned(symbol)
    }

    /// The hypervector for a role symbol. Roles and fillers share one codebook;
    /// a `role:` prefix keeps the role namespace from colliding with fillers of
    /// the same spelling (so a filler "SUBJECT" and the role SUBJECT differ).
    pub fn vsa_role_vec(&mut self, role: &str) -> Hypervec {
        self.vsa.get_cloned(&format!("role:{}", role))
    }

    /// BIND_ROLE: bundle `bind(role, filler)` into register `reg`.
    ///
    /// If `reg` is empty/non-hvec it becomes exactly `bind(role, filler)`;
    /// otherwise the new pair is bundled (majority-vote superposition) with the
    /// existing structure, so multiple roles compose into one vector:
    ///   reg = bundle(reg, bind(role_i, filler_i))
    pub fn vsa_bind_into(&mut self, reg: &str, role: &str, filler: &str) {
        let rv = self.vsa_role_vec(role);
        let fv = self.vsa_symbol_vec(filler);
        let pair = rv.bind(&fv);
        let combined = match self.registers.get(reg) {
            Some(Value::Hvec(existing)) => Hypervec::bundle(&[existing, &pair]),
            _ => pair,
        };
        self.registers.insert(reg.to_string(), Value::Hvec(combined));
    }

    /// UNBIND + cleanup: recover the filler bound to `role` in register `reg`.
    ///
    /// Computes `unbind(reg_hvec, role)` (= bind, self-inverse) and returns the
    /// candidate symbol whose codebook vector is cosine-nearest to the result,
    /// with that similarity. `None` if `reg` holds no hypervector. A high
    /// similarity means a real binding was present; near-zero means the role
    /// was not bound (the unbind yields quasi-orthogonal noise).
    pub fn vsa_unbind_cleanup(
        &mut self,
        reg: &str,
        role: &str,
        candidates: &[&str],
    ) -> Option<(String, f64)> {
        let bound = match self.registers.get(reg) {
            Some(Value::Hvec(h)) => h.clone(),
            _ => return None,
        };
        let rv = self.vsa_role_vec(role);
        let recovered = bound.unbind(&rv);
        let mut best: Option<(String, f64)> = None;
        for &cand in candidates {
            let cv = self.vsa_symbol_vec(cand);
            let sim = recovered.cosine_sim(&cv);
            match &best {
                Some((_, bs)) if *bs >= sim => {}
                _ => best = Some((cand.to_string(), sim)),
            }
        }
        best
    }

    /// Task 6: `vsa_unbind_cleanup` against every symbol the VM currently
    /// knows -- the same candidate pool `op::UNBIND`'s own bytecode arm
    /// builds inline (`self.name_table.values()`). For callers, like the
    /// `vsa` registry module's `recover`, that have no curated candidate
    /// list of their own and just want "the real answer, if the VM has
    /// ever seen it as a name" -- exposed as its own method (rather than
    /// making `registry.rs` reach into the private `name_table` field
    /// directly) so the "candidates = every known symbol" policy stays
    /// defined in one place.
    pub fn vsa_unbind_cleanup_known(&mut self, reg: &str, role: &str) -> Option<(String, f64)> {
        let candidates: Vec<String> = self.name_table.values().cloned().collect();
        let cand_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        self.vsa_unbind_cleanup(reg, role, &cand_refs)
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

    // Task 4: these fixtures dropped a decorative `implements ISolver` that
    // predates real `implements` validation -- every test here exercises VM
    // opcode/execution semantics unrelated to interfaces, and none defined
    // parse/verify, so the claim was already false before this task, just
    // never checked.

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
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
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
        let result = vm.call("Test", "solve", Vec::new());

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
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 16;
                    sub x, 3;
                    sub x, 4;
                    sum x;
                    push x;
                    create y : number;
                    pop y;
                    mul y, 2;
                    sum y;
                    query y;
                    remember y;
                }
            }
        "#);

        let result = vm.call("Test", "solve", Vec::new());

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
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 42;
                    div x, 0;
                }
            }
        "#);

        let result = vm.call("Test", "solve", Vec::new());
        match result {
            ExecResult::Error(e) => assert!(e.contains("division by zero")),
            _ => panic!("expected division by zero error"),
        }
    }

    #[test]
    fn test_vm_store_recall() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 42;
                    remember x;
                }
            }
        "#);

        vm.call("Test", "solve", Vec::new());

        // The register should be in storage after REMEMBER
        assert!(vm.storage.values().any(|v| v.as_i64() == 42));
    }

    #[test]
    fn test_vm_multiple_functions() {
        let mut vm = compile_and_load(r#"
            program Calc implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 10;
                    add x, 20;
                    sum x;
                }
                public function mul_numbers(): void {
                    create y : number;
                    assign y = 5;
                    mul y, 6;
                    sum y;
                }
            }
        "#);

        let r1 = vm.call("Calc", "solve", Vec::new());
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
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
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

        let result = vm.call("Test", "solve", Vec::new());
        match result {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 63, "100 - 37 = 63"),
            _ => panic!("expected ok"),
        }

        let _ = std::fs::remove_file(&path);
    }

    // Plan-1 (RETURN). Early `return v` must halt execution AND yield v.
    // Probe: a "poison" `sum poison` after the return rewrites the accumulator
    // if fall-through happens, so the returned value distinguishes halt vs fall.
    #[test]
    fn return_early_halts_with_value() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create r : number;
                    create poison : number;
                    assign r = 1;
                    assign poison = 99;
                    return r;
                    sum poison;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) =>
                assert_eq!(v.as_i64(), 1,
                    "early return must yield 1, not the poisoned 99 (= fall-through)"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn return_early_inside_if_skips_rest() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create r : number;
                    create poison : number;
                    assign r = 5;
                    assign poison = 99;
                    if (r > 0) {
                        return r;
                    }
                    sum poison;
                    return poison;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 5,
                "return inside if must halt before sum poison runs"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn return_bare_yields_current_accumulator() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    create poison : number;
                    assign x = 7;
                    assign poison = 99;
                    sum x;
                    return;
                    sum poison;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 7,
                "bare return must yield the accumulator set by sum x, not the poisoned sum"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    // Plan-4 (LOOP for-each). `for (let x of arr) { body }` lowers to an
    // index-based while using LEN/INDEX/COMPARE/COND/JMP. The loop must
    // iterate every element, terminate on empty arrays, and nest correctly.
    #[test]
    fn for_each_sums_three_elements() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [10, 20, 30];
                    create total : number;
                    assign total = 0;
                    for (let x of xs) {
                        add total, x;
                    }
                    sum total;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 60),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn for_each_empty_array_runs_zero_iterations() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [];
                    create total : number;
                    assign total = 0;
                    for (let x of xs) {
                        add total, 999;
                    }
                    sum total;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 0),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn for_each_nested_loops_visit_every_pair() {
        // sum over (xi + yj) for xi in [1,2], yj in [10,20] = 4*1 + 4*2 ... actually
        // = (1+10)+(1+20)+(2+10)+(2+20) = 11+21+12+22 = 66.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [1, 2];
                    let ys = [10, 20];
                    create total : number;
                    assign total = 0;
                    for (let x of xs) {
                        for (let y of ys) {
                            add total, x;
                            add total, y;
                        }
                    }
                    sum total;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 66),
            other => panic!("unexpected: {:?}", other),
        }
    }

    // Plan-3 (MAKE_ARRAY / LEN / INDEX). Array literals must construct a
    // real Value::Array, `arr.len()` must return its length, and `arr[i]`
    // must index it. Out-of-range indexing yields Null.
    #[test]
    fn array_literal_len_is_three() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [10, 20, 30];
                    let n = xs.len();
                    sum n;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 3),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn array_index_returns_element() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [10, 20, 30];
                    let x = xs[1];
                    sum x;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 20),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn array_index_out_of_range_is_null() {
        // Out-of-range indexing produces Null; sum yields 0 (as_i64 of Null).
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [10, 20, 30];
                    let x = xs[99];
                    sum x;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 0,
                "out-of-range index must be Null, not panic"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    // Plan-2 (CALL). Intra-program call must transfer arg0..argN, execute the
    // callee, and deliver its return value to the caller via the stack.
    #[test]
    fn call_returns_value_via_stack() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function dbl(): number {
                    create r : number;
                    assign r = 0;
                    add r, arg0;
                    add r, arg0;
                    return r;
                }
                public function solve(input: str): void {
                    let y = dbl(21);
                    sum y;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) =>
                assert_eq!(v.as_i64(), 42, "dbl(21) must return 42"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn call_caller_locals_isolated() {
        // The callee writes to a register with the same name as a caller local.
        // After return, the caller's value must be intact.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function helper(): number {
                    create r : number;
                    assign r = 99;
                    return r;
                }
                public function solve(input: str): void {
                    create r : number;
                    assign r = 7;
                    let _t = helper();
                    sum r;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) =>
                assert_eq!(v.as_i64(), 7,
                    "caller's local r must still be 7 after helper() (got {:?})", v),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn call_recursion_factorial() {
        // fact(n) = if n <= 1 then 1 else n * fact(n-1)
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function fact(): number {
                    create n : number;
                    assign n = 0;
                    add n, arg0;
                    if (n > 1) {
                        create m : number;
                        assign m = 0;
                        add m, n;
                        sub m, 1;
                        let sub = fact(m);
                        create out : number;
                        assign out = 0;
                        add out, n;
                        mul out, sub;
                        return out;
                    }
                    return 1;
                }
                public function solve(input: str): void {
                    let r = fact(5);
                    sum r;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) =>
                assert_eq!(v.as_i64(), 120, "fact(5) must return 120"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn call_depth_guard_returns_clean_error() {
        // Unbounded self-recursion should produce a clean Error, not a native
        // stack overflow. fact() without a base case keeps calling.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function loop_forever(): number {
                    let r = loop_forever();
                    return r;
                }
                public function solve(input: str): void {
                    let r = loop_forever();
                    sum r;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Error(e) => assert!(e.to_lowercase().contains("depth"),
                "expected depth-guard error, got: {}", e),
            other => panic!("expected depth-guard Error, got {:?}", other),
        }
    }

    // P0-2 regression: `assign b = a` must copy a's value, not store the
    // register-name token. Before the fix, ASSIGN called `to_value()` on a
    // Named operand and stored "r_xxxx" (a string) — sum returned 0 / a name.
    #[test]
    fn assign_register_to_register_copies_value() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create a : number;
                    create b : number;
                    assign a = 5;
                    assign b = a;
                    sum b;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 5,
                "assign b = a must propagate the value 5 (got {:?})", v),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn assign_register_to_register_then_arithmetic() {
        // Compose with arithmetic so a regression also breaks the chain.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create a : number;
                    create b : number;
                    assign a = 7;
                    assign b = a;
                    add b, 3;
                    sum b;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 10),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn assign_literal_int_unchanged() {
        // Guard: the bug fix must NOT regress literal assignment.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create a : number;
                    assign a = 42;
                    sum a;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 42),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_trace() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 5;
                    add x, 3;
                    sum x;
                }
            }
        "#);

        vm.trace_enabled = true;
        vm.call("Test", "solve", Vec::new());

        assert!(!vm.trace.is_empty());
        assert!(vm.trace.iter().any(|t| t.contains("CREATE")));
        assert!(vm.trace.iter().any(|t| t.contains("ASSIGN")));
        assert!(vm.trace.iter().any(|t| t.contains("ADD")));
        assert!(vm.trace.iter().any(|t| t.contains("SUM")));
    }

    #[test]
    fn test_vm_if_then_branch() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 5;
                    if (x > 3) { assign x = 100; } else { assign x = 200; }
                    sum x;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 100, "x=5, x>3 true → then-branch → 100"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_if_else_branch() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 2;
                    if (x > 3) { assign x = 100; } else { assign x = 200; }
                    sum x;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 200, "x=2, x>3 false → else-branch → 200"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_while_loop_terminates_and_accumulates() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create total : number;
                    assign total = 0;
                    create i : number;
                    assign i = 5;
                    while (i > 0) { add total, 10; sub i, 1; }
                    sum total;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 50, "5 iterations × 10 = 50"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_compare_sets_cmp_register() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create a : number;
                    assign a = 7;
                    create b : number;
                    assign b = 7;
                    compare a, b;
                }
            }
        "#);
        vm.call("T", "solve", Vec::new());
        assert_eq!(vm.get("__cmp").map(|v| v.as_str()), Some("equal".to_string()));
    }

    #[test]
    fn test_vm_float_arithmetic() {
        // 80000 * 1.5 = 120000 (real fractional multiply, integral result).
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create v : number;
                    assign v = 80000;
                    mul v, 1.5;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 120000),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_float_nonintegral_result() {
        // 7 / 2 = 3.5 — non-integral float preserved.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create v : number;
                    assign v = 7;
                    div v, 2.0;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert!((v.as_f64() - 3.5).abs() < 1e-9, "got {}", v.as_f64()),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_true_division_int_operands() {
        // 60 / 100 = 0.6 (true division on integer operands, not floor → 0).
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create v : number;
                    assign v = 60;
                    div v, 100;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert!((v.as_f64() - 0.6).abs() < 1e-9, "got {}", v.as_f64()),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_exact_int_division_stays_int() {
        // 16 / 2 = 8 stays integer.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create v : number;
                    assign v = 16;
                    div v, 2;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
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


#[cfg(test)]
mod vsa_binding_tests {
    //! Genuine VSA role-filler binding over hypervectors.
    //!
    //! These drive the VM's hyperdimensional capability directly (not via
    //! bytecode): a register can hold a bundled hypervector built from
    //! bind(role, filler) terms, and a role can be unbound + cleaned up against
    //! a candidate filler set to recover the original symbol -- the
    //! bind(sentence, inverse(ROLE)) round-trip. The primitives live in
    //! hypervec.rs; this wires them into the VM as Value::Hvec + helper methods.
    use super::*;

    // A bound register must hold an actual hypervector, not a string key.
    #[test]
    fn bind_role_produces_hypervector_value() {
        let mut vm = VM::new();
        vm.vsa_bind_into("sentence", "SUBJECT", "cat");
        match vm.get("sentence") {
            Some(Value::Hvec(h)) => assert_eq!(h.dim(), MEM_DIM,
                "bound register must hold a MEM_DIM hypervector"),
            other => panic!("expected Value::Hvec, got {:?}", other),
        }
    }

    // Single role: unbind(bind(ROLE, filler), ROLE) ~= filler, recovered by
    // cleanup against the candidate set at similarity 1.0 (self-inverse bind).
    #[test]
    fn single_role_unbinds_to_filler() {
        let mut vm = VM::new();
        vm.vsa_bind_into("s", "SUBJECT", "cat");
        let candidates = ["cat", "dog", "mouse", "tree"];
        let (sym, sim) = vm.vsa_unbind_cleanup("s", "SUBJECT", &candidates)
            .expect("must recover a filler");
        assert_eq!(sym, "cat", "unbinding SUBJECT must recover 'cat'");
        assert!(sim > 0.99, "single-term unbind must be near-exact, got sim={sim:.4}");
    }

    // The headline example: cat chases mouse, bundled into one vector, each
    // role recoverable. With 3 bundled terms the recovered filler is the
    // cosine-nearest candidate (noisy but correct -- the cleanup memory's job).
    #[test]
    fn three_role_sentence_recovers_each_filler() {
        let mut vm = VM::new();
        vm.vsa_bind_into("sent", "SUBJECT", "cat");
        vm.vsa_bind_into("sent", "VERB", "chases");
        vm.vsa_bind_into("sent", "OBJECT", "mouse");

        let cand = ["cat", "dog", "chases", "eats", "mouse", "tree", "runs"];
        let (s, ss) = vm.vsa_unbind_cleanup("sent", "SUBJECT", &cand).unwrap();
        let (v, vs) = vm.vsa_unbind_cleanup("sent", "VERB", &cand).unwrap();
        let (o, os) = vm.vsa_unbind_cleanup("sent", "OBJECT", &cand).unwrap();
        assert_eq!(s, "cat", "SUBJECT must clean up to cat (sim {ss:.3})");
        assert_eq!(v, "chases", "VERB must clean up to chases (sim {vs:.3})");
        assert_eq!(o, "mouse", "OBJECT must clean up to mouse (sim {os:.3})");
    }

    // Querying a role the sentence does not contain must NOT confidently
    // return a bound filler: the best cleanup similarity should be low
    // (quasi-orthogonal noise), distinguishing "absent" from a real hit.
    #[test]
    fn absent_role_yields_low_similarity() {
        let mut vm = VM::new();
        vm.vsa_bind_into("sent", "SUBJECT", "cat");
        vm.vsa_bind_into("sent", "OBJECT", "mouse");
        let cand = ["cat", "mouse", "dog"];
        // LOCATION was never bound -> unbinding it yields noise vs all candidates.
        let best = vm.vsa_unbind_cleanup("sent", "LOCATION", &cand);
        if let Some((_, sim)) = best {
            assert!(sim < 0.3,
                "unbinding an absent role must be low-similarity noise, got {sim:.4}");
        }
    }

    // Determinism: same symbols -> same hypervector across VM instances
    // (fixed codebook seed), so bound structures are reproducible across runs.
    #[test]
    fn role_and_symbol_vectors_are_deterministic() {
        let mut vm1 = VM::new();
        let mut vm2 = VM::new();
        let a = vm1.vsa_symbol_vec("cat");
        let b = vm2.vsa_symbol_vec("cat");
        assert_eq!(a, b, "same symbol must map to same hypervector across VMs");
        let r1 = vm1.vsa_role_vec("SUBJECT");
        let r2 = vm2.vsa_role_vec("SUBJECT");
        assert_eq!(r1, r2, "same role must map to same hypervector across VMs");
        // Distinct symbols are quasi-orthogonal (so cleanup can separate them).
        assert!(a.cosine_sim(&vm1.vsa_symbol_vec("dog")).abs() < 0.1,
            "distinct fillers must be quasi-orthogonal");
    }
}


#[cfg(test)]
mod vsa_opcode_wiring_tests {
    //! End-to-end: `bind` statements in real CubeLang source must produce
    //! genuine VSA hypervector bindings in the target register (not string-key
    //! inserts), recoverable by unbinding the role. This drives the compiler
    //! to retain role/filler symbol names (so the VM can reverse the 2-byte
    //! hashes the bytecode carries) and the BIND_ROLE/UNBIND opcode arms to
    //! call the vsa_* methods.
    use super::*;

    fn compile_and_load(source: &str) -> VM {
        let programs = crate::compiler::compile(source).unwrap();
        let mut vm = VM::new();
        for prog in programs {
            vm.load(prog);
        }
        vm
    }

    // After `bind sentence, SUBJECT, "cat"`, the register `sentence` must hold
    // a Value::Hvec -- a real bound hypervector -- not a Str/Null.
    #[test]
    fn bind_statement_produces_hypervector_register() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create sentence : frame;
                    bind sentence, SUBJECT, "cat";
                }
            }
        "#);
        vm.call("T", "solve", Vec::new());
        // The register key is the hashed name; find the single Hvec value.
        let has_hvec = vm.registers.values()
            .any(|v| matches!(v, Value::Hvec(_)));
        assert!(has_hvec,
            "after `bind`, some register must hold a Value::Hvec (genuine VSA bind)");
    }

    // The headline round-trip from source: bind three roles into one register,
    // then each role unbinds+cleans-up to the right filler. We reach into the
    // VM's vsa methods for cleanup using the SAME register the bytecode wrote.
    #[test]
    fn bound_frame_recovers_fillers_via_unbind() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create frame : frame;
                    bind frame, SUBJECT, "cat";
                    bind frame, VERB, "chases";
                    bind frame, OBJECT, "mouse";
                }
            }
        "#);
        vm.call("T", "solve", Vec::new());

        // Resolve the register the compiler used for `frame` (hashed name key).
        let reg_key = vm.registers.iter()
            .find(|(_, v)| matches!(v, Value::Hvec(_)))
            .map(|(k, _)| k.clone())
            .expect("a bound hypervector register must exist");

        let cand = ["cat", "dog", "chases", "eats", "mouse", "tree"];
        let (s, _) = vm.vsa_unbind_cleanup(&reg_key, "SUBJECT", &cand).unwrap();
        let (v, _) = vm.vsa_unbind_cleanup(&reg_key, "VERB", &cand).unwrap();
        let (o, _) = vm.vsa_unbind_cleanup(&reg_key, "OBJECT", &cand).unwrap();
        assert_eq!(s, "cat", "SUBJECT must recover cat");
        assert_eq!(v, "chases", "VERB must recover chases");
        assert_eq!(o, "mouse", "OBJECT must recover mouse");
    }

    // UNBIND opcode (engine-level): after binding, executing UNBIND on the role
    // must remove/replace the bound structure with the recovered filler symbol
    // in the target register. We hand-build the bytecode since no surface
    // syntax emits UNBIND yet, mirroring the compiler's operand layout.
    #[test]
    fn unbind_opcode_recovers_filler_into_register() {
        use crate::compiler::{op, CompiledProgram, CompiledFunction};

        // Bytecode for: BIND_ROLE frame, SUBJECT, "cat"; UNBIND frame, SUBJECT
        // operand tags: NAMED=0x01, ROLE=0x04, GLOBAL=0x05
        fn h(s: &str) -> [u8; 2] {
            let d = blake3::hash(s.as_bytes());
            [d.as_bytes()[0], d.as_bytes()[1]]
        }
        let frame = h("frame");
        let subj = h("SUBJECT");
        let cat = h("cat");

        let mut bc: Vec<u8> = Vec::new();
        // BIND_ROLE (3 operands): NAMED frame, ROLE SUBJECT, GLOBAL "cat"
        bc.push(op::BIND_ROLE); bc.push(3);
        bc.push(0x01); bc.push(frame[0]); bc.push(frame[1]);
        bc.push(0x04); bc.push(subj[0]);  bc.push(subj[1]);
        bc.push(0x05); bc.push(cat[0]);   bc.push(cat[1]);
        // UNBIND (2 operands): NAMED frame, ROLE SUBJECT
        bc.push(op::UNBIND); bc.push(2);
        bc.push(0x01); bc.push(frame[0]); bc.push(frame[1]);
        bc.push(0x04); bc.push(subj[0]);  bc.push(subj[1]);

        let func = CompiledFunction {
            name: "run".to_string(),
            params: Vec::new(),
            bytecode: bc,
            labels: Vec::new(),
            debug_lines: Vec::new(),
            is_override: false,
        };
        let prog = CompiledProgram {
            name: "T".to_string(),
            implements: vec!["ISolver".to_string()],
            functions: vec![func],
            storage_fields: Vec::new(),
            symbol_names: vec![
                "frame".to_string(), "SUBJECT".to_string(), "cat".to_string(),
            ],
            used_modules: Vec::new(),
        };

        let mut vm = VM::new();
        vm.load(prog);
        vm.call("T", "run", Vec::new());

        // After UNBIND, the frame register should carry the recovered filler
        // symbol "cat" (as a Str), the cleanup result of unbinding SUBJECT.
        let frame_key = format!("r_{:02x}{:02x}", frame[0], frame[1]);
        match vm.get(&frame_key) {
            Some(Value::Str(s)) => assert_eq!(s, "cat",
                "UNBIND must leave the recovered filler 'cat' in the register"),
            other => panic!("expected recovered Str(\"cat\"), got {:?}", other),
        }
    }
}
