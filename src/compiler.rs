//! CubeLang bytecode compiler — AST → CubeMind VM bytecodes.
//!
//! Walks the AST and emits binary bytecodes (0x00-0xFF) for each instruction.
//! The output is a `CompiledProgram` containing the bytecode blob, metadata,
//! and a symbol table for debugging.

use crate::ast::*;

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
    pub const SKIP: u8      = 0xFF;

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
pub const CUBEBIN_VERSION: u8 = 1;

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
            let bc_len = func.bytecode.len() as u32;
            buf.extend_from_slice(&bc_len.to_le_bytes());
            buf.extend_from_slice(&func.bytecode);
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
        if version != CUBEBIN_VERSION {
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

            let bc_len = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
            pos += 4;
            let bytecode = data[pos..pos + bc_len].to_vec();
            pos += bc_len;

            functions.push(CompiledFunction {
                name: fn_name,
                bytecode,
                labels: Vec::new(),
                debug_lines: Vec::new(),
            });
        }

        Ok(CompiledProgram { name, implements, functions, storage_fields })
    }

    /// Save to a `.cubebin` file.
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
    pub bytecode: Vec<u8>,
    pub labels: Vec<(String, usize)>,   // label name → byte offset
    pub debug_lines: Vec<DebugLine>,    // source mapping
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
}

impl Compiler {
    pub fn new() -> Self {
        Self {
            bytecode: Vec::new(),
            labels: Vec::new(),
            debug_lines: Vec::new(),
            label_counter: 0,
        }
    }

    /// Compile a full source file.
    pub fn compile(&mut self, source: &SourceFile) -> Vec<CompiledProgram> {
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
        }
    }

    fn compile_function(&mut self, func: &FunctionDecl) -> CompiledFunction {
        self.bytecode.clear();
        self.labels.clear();
        self.debug_lines.clear();

        for stmt in &func.body {
            self.compile_stmt(stmt);
        }

        CompiledFunction {
            name: func.sig.name.clone(),
            bytecode: self.bytecode.clone(),
            labels: self.labels.clone(),
            debug_lines: self.debug_lines.clone(),
        }
    }

    // ── Statement compilation ───────────────────────────────────────────

    fn compile_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Opcode(op) => self.compile_opcode(op),
            Stmt::Let(l) => self.compile_let(l),
            Stmt::If(i) => self.compile_if(i),
            Stmt::For(f) => self.compile_for(f),
            Stmt::While(w) => self.compile_while(w),
            Stmt::Match(m) => self.compile_match(m),
            Stmt::Return(_) => {
                // Return is handled by the VM runtime, emit SKIP as placeholder
                self.emit_debug(op::SKIP, "return");
                self.emit_byte(op::SKIP);
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

        // Condition → COND jump to else
        self.emit_debug(op::COND, "if");
        self.compile_expr_discard(&if_stmt.condition);
        self.emit_jmp(&else_label);

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

        if if_stmt.else_body.is_some() {
            self.emit_label(&end_label);
        }
    }

    fn compile_for(&mut self, for_stmt: &ForStmt) {
        let loop_label = self.fresh_label("for");
        let end_label = self.fresh_label("endfor");

        self.emit_label(&loop_label);
        self.emit_debug(op::LOOP, &format!("for {} of ...", for_stmt.binding));

        for s in &for_stmt.body {
            self.compile_stmt(s);
        }

        self.emit_jmp(&loop_label);
        self.emit_label(&end_label);
    }

    fn compile_while(&mut self, while_stmt: &WhileStmt) {
        let loop_label = self.fresh_label("while");
        let end_label = self.fresh_label("endwhile");

        self.emit_label(&loop_label);
        self.emit_debug(op::LOOP, "while");
        self.compile_expr_discard(&while_stmt.condition);

        for s in &while_stmt.body {
            self.compile_stmt(s);
        }

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

        self.emit_byte(op::ASSIGN);
        self.emit_byte(2);
        self.emit_named(&let_stmt.name);
        self.compile_expr_operand(&let_stmt.value);
    }

    // ── Expression compilation (to operand bytes) ───────────────────────

    fn compile_expr_operand(&mut self, expr: &Expr) {
        match expr {
            Expr::IntLit(n) => self.emit_imm(*n),
            Expr::FloatLit(f) => self.emit_imm(*f as i64),
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
                self.emit_debug(op::CALL, "call");
                self.emit_byte(op::CALL);
                self.emit_byte(1);
                if let Expr::Ident(name) = callee.as_ref() {
                    self.emit_global(name);
                } else {
                    self.emit_byte(OP_NONE);
                    self.emit_byte(0x00);
                    self.emit_byte(0x00);
                }
            }
            Expr::MethodCall(obj, method, args) => {
                self.emit_debug(op::CALL, &format!(".{}()", method));
                self.emit_byte(op::CALL);
                self.emit_byte(1);
                self.emit_global(method);
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

// ── Public API ──────────────────────────────────────────────────────────────

/// Compile CubeLang source to bytecode programs.
pub fn compile(source: &str) -> Result<Vec<CompiledProgram>, crate::parser::ParseError> {
    let ast = crate::parser::parse(source)?;
    let mut compiler = Compiler::new();
    Ok(compiler.compile(&ast))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_empty_program() {
        let programs = compile(r#"
            program Empty implements ISolver {
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
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
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
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
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
            program Test implements ISolver {
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
            program Test implements ISolver {
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
            program Test implements ISolver {
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
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
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
                    create x : quantity;
                    assign x = 16;
                    sub x, 3;
                    sub x, 4;
                    sum x;
                    query x;
                    remember x;
                    return Output { result: 9 };
                }
            }
        "#).unwrap();

        let prog = &programs[0];

        // Serialize
        let bin = prog.to_cubebin();
        assert_eq!(&bin[0..4], b"CUBE");
        assert_eq!(bin[4], 1); // version

        // Deserialize
        let loaded = CompiledProgram::from_cubebin(&bin).unwrap();
        assert_eq!(loaded.name, "GSM8K");
        assert_eq!(loaded.implements, vec!["ISolver"]);
        assert_eq!(loaded.storage_fields, vec!["patterns", "count"]);
        assert_eq!(loaded.functions.len(), 2);
        assert_eq!(loaded.functions[0].name, "constructor");
        assert_eq!(loaded.functions[1].name, "solve");

        // Bytecode should match exactly
        assert_eq!(loaded.functions[1].bytecode, prog.functions[1].bytecode);
    }

    #[test]
    fn test_cubebin_file_roundtrip() {
        let programs = compile(r#"
            program Test implements ISolver {
                public function run(): void {
                    create x : quantity;
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
