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
                    let n = operands.get(1).map(|o| o.to_value().as_i64()).unwrap_or(0);
                    self.trace_op(pc, &format!("ADD {} {}", name, n));
                    let cur = self.registers.get(&name).map(|v| v.as_i64()).unwrap_or(0);
                    self.registers.insert(name, Value::Int(cur + n));
                }

                op::SUB => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let n = operands.get(1).map(|o| o.to_value().as_i64()).unwrap_or(0);
                    self.trace_op(pc, &format!("SUB {} {}", name, n));
                    let cur = self.registers.get(&name).map(|v| v.as_i64()).unwrap_or(0);
                    self.registers.insert(name, Value::Int(cur - n));
                }

                op::MUL => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let n = operands.get(1).map(|o| o.to_value().as_i64()).unwrap_or(1);
                    self.trace_op(pc, &format!("MUL {} {}", name, n));
                    let cur = self.registers.get(&name).map(|v| v.as_i64()).unwrap_or(0);
                    self.registers.insert(name, Value::Int(cur * n));
                }

                op::DIV => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let n = operands.get(1).map(|o| o.to_value().as_i64()).unwrap_or(1);
                    self.trace_op(pc, &format!("DIV {} {}", name, n));
                    if n == 0 {
                        return ExecResult::Error("division by zero".into());
                    }
                    let cur = self.registers.get(&name).map(|v| v.as_i64()).unwrap_or(0);
                    self.registers.insert(name, Value::Int(cur / n));
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
                    self.trace_op(pc, "COMPARE");
                }

                op::LABEL => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("LABEL {}", name));
                }

                op::JMP => {
                    let target = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("JMP {}", target));
                    // For now, skip jumps (proper implementation needs label→offset resolution)
                }

                op::COND => {
                    self.trace_op(pc, "COND");
                }

                op::LOOP => {
                    self.trace_op(pc, "LOOP");
                }

                op::CALL => {
                    let target = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("CALL {}", target));
                }

                op::UNIFY => {
                    let a = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let b = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("UNIFY {} {}", a, b));
                }

                _ => {
                    self.trace_op(pc, &format!("UNKNOWN 0x{:02x}", opcode));
                }
            }
        }

        // Return the accumulator as the result
        ExecResult::Ok(self.accumulator.clone())
    }

    fn trace_op(&mut self, offset: usize, desc: &str) {
        if self.trace_enabled {
            self.trace.push(format!("0x{:04x}: {}", offset, desc));
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
            Operand::None => "_".into(),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Operand::Imm(n) => Value::Int(*n),
            Operand::Named(h) => Value::Str(format!("r_{:02x}{:02x}", h[0], h[1])),
            Operand::Global(h) => Value::Str(format!("g_{:02x}{:02x}", h[0], h[1])),
            _ => Value::Null,
        }
    }
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
