//! CubeLang AST — abstract syntax tree for parsed `.cube` programs.

use crate::token::Span;

/// A complete `.cube` source file.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub items: Vec<TopLevel>,
}

/// Top-level declarations.
#[derive(Debug, Clone)]
pub enum TopLevel {
    Interface(InterfaceDecl),
    Program(ProgramDecl),
    Container(ContainerDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    TypeAlias(TypeAliasDecl),
    EventDecl(EventDecl),
    ExtendBlock(ExtendBlock),
    /// `use <name>;` — brings a VM-internal registry module into scope for
    /// the whole compilation unit (every `Program` compiled from the same
    /// `SourceFile`). See `compiler::Compiler::compile` (gathers these
    /// before compiling any program) and `vm::registry::ModuleRegistry`.
    Use(String),
    /// `import "path.cube";` — Task 5: pulls another user file's top-level
    /// declarations into this compilation. Unlike `Use` (a VM-internal
    /// registry module, name-addressed, no filesystem involved), `Import`
    /// carries a filesystem path (relative to whichever file declares it)
    /// that a loader pre-pass resolves BEFORE compilation ever sees a
    /// `SourceFile` — see `crate::loader`. By the time `compiler::compile`
    /// (or `compile_ast`) runs, every `Import` a `loader::load` pass
    /// produced has already been expanded away; a bare `parser::parse` on a
    /// single file's text (as every `&str`-input free function still does)
    /// can still leave `Import` items in `SourceFile::items` — those are
    /// simply not visited by `Compiler::compile` (mirrors how it already
    /// ignores `ExtendBlock`/etc.), so parsing a file with an `import` in
    /// isolation is harmless, just incomplete without the loader.
    ///
    /// NOT the old in-body ES-module `Stmt::Import`/`ImportStmt` (deleted in
    /// Task 1 as the wrong shape for real file import — an in-body
    /// statement can't express "pull in these top-level declarations").
    Import(String),
}

// ── Interfaces ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct InterfaceDecl {
    pub name: String,
    pub members: Vec<InterfaceMember>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum InterfaceMember {
    TypeDecl { name: String, span: Span },
    Function(FunctionSig),
}

// ── Programs ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ProgramDecl {
    pub name: String,
    pub implements: Vec<String>,
    pub body: Vec<ProgramItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ProgramItem {
    TypeAlias(TypeAliasDecl),
    Storage(StorageBlock),
    Function(FunctionDecl),
    Constructor(FunctionDecl),
}

// ── Containers ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ContainerDecl {
    pub kind: ContainerKind,
    pub name: String,
    pub extends: Option<String>,
    pub implements: Vec<String>,
    pub body: Vec<ContainerItem>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Container,
    World,
    Modal,
    Robot,
}

#[derive(Debug, Clone)]
pub enum ContainerItem {
    Config(Vec<ConfigField>),
    Io(IoBlock),
    Storage(StorageBlock),
    Programs(Vec<ProgramBinding>),
    Agents(Vec<AgentBinding>),
    Permissions(Vec<PermissionStmt>),
    World(Vec<StorageField>),
    Modalities(Vec<ModalityDef>),
    Sensors(Vec<SensorDef>),
    Actuators(Vec<ActuatorDef>),
    Safety(Vec<ConfigField>),
    Fusion(Vec<ConfigField>),
    Function(FunctionDecl),
}

// ── Structs, Enums, Types ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<StructField>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<EnumVariant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub value: Option<Expr>,           // = 0x03
    pub data: Option<Vec<TypeExpr>>,   // (f64) for tagged unions
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeAliasDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EventDecl {
    pub name: String,
    pub fields: Vec<StructField>,
    pub span: Span,
}

// ── Functions ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FunctionSig {
    pub permissions: Vec<Permission>,
    pub modifiers: Vec<Modifier>,
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    /// A POSTFIX `override` marker (`function name() override { ... }`,
    /// parsed after the return type) — Task 3's capability/module system.
    /// Deliberately a SEPARATE field from `modifiers`'s pre-existing PREFIX
    /// `Modifier::Override` (`override function name() { ... }`, parsed
    /// above in `modifiers`), which SPEC.md reserves for a different,
    /// not-yet-built feature (inheritance-style overriding inside an
    /// `extend` block). The two are unrelated mechanisms that happen to
    /// share an English word; keeping them in separate fields means neither
    /// implementation can silently start meaning the other.
    ///
    /// Only meaningful in a program-level `FunctionDecl`, and only valid
    /// when `name` is exposed as `overridable` by a module the enclosing
    /// `SourceFile` `use`s — `Compiler::check_override_validity` enforces
    /// this at compile time (see `vm::registry::ModuleRegistry`); by the
    /// time bytecode exists, `is_override == true` always means a
    /// legitimate override.
    pub is_override: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunctionDecl {
    pub sig: FunctionSig,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Public,
    Private,
    Abstract,
    Global,
    Mutable,
    Immutable,
    Async,
    Sequential,
    Parallel,
    Joined,
    Pure,
    Optional,
    Singleton,
    Override,
}

#[derive(Debug, Clone)]
pub enum Permission {
    External,
    Internal,
    System,
    Hook(String),       // @hook(Program.Event)
    Before(String),     // @before(fn_name)
    After(String),      // @after(fn_name)
    Cron(String),       // @cron("1h")
    Once,
    Restricted(Vec<String>),
    Ratelimit(i64, String),
}

// ── Storage, IO, Config ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StorageBlock {
    pub is_global: bool,
    pub fields: Vec<StorageField>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StorageField {
    pub name: String,
    pub mutable: bool,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IoBlock {
    pub inputs: Vec<IoField>,
    pub outputs: Vec<IoField>,
    pub formats: Option<FormatsBlock>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IoField {
    pub name: String,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FormatsBlock {
    pub accept: Vec<String>,
    pub emit: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ConfigField {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

// ── Container-specific blocks ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ProgramBinding {
    pub name: String,
    pub expr: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AgentBinding {
    pub name: String,
    pub expr: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct PermissionStmt {
    pub action: PermAction,
    pub subject: String,    // program.function
    pub target: String,     // to/from whom
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub enum PermAction {
    Grant,
    Revoke,
}

#[derive(Debug, Clone)]
pub struct ModalityDef {
    pub name: String,
    pub fields: Vec<ConfigField>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct SensorDef {
    pub name: String,
    pub fields: Vec<ConfigField>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ActuatorDef {
    pub name: String,
    pub fields: Vec<ConfigField>,
    pub span: Span,
}

// ── Extend ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ExtendBlock {
    pub target: String,
    pub functions: Vec<FunctionDecl>,
    pub span: Span,
}

// ── Type Expressions ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum TypeExpr {
    Named(String),                             // u32, str, Input, ISolver
    Array(Box<TypeExpr>),                      // array<T>
    Map(Box<TypeExpr>, Box<TypeExpr>),         // map<K, V>
    Set(Box<TypeExpr>),                        // set<T>
    Promise(Box<TypeExpr>),                    // promise<T>
    Channel(Box<TypeExpr>),                    // channel<T>
    Tuple(Vec<TypeExpr>),                      // tuple<T1, T2>
    Union(Vec<TypeExpr>),                      // T1 | T2
    Nullable(Box<TypeExpr>),                   // T?
    Fn(Vec<TypeExpr>, Box<TypeExpr>),          // (T1, T2) -> R
    Void,
}

// ── Statements ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(LetStmt),
    Assign(AssignStmt),
    If(IfStmt),
    For(ForStmt),
    While(WhileStmt),
    Match(MatchStmt),
    Return(Option<Expr>),
    Throw(Expr),
    Emit(EmitStmt),
    Opcode(OpcodeStmt),
    Expr(Expr),

    // Error handling
    TryCatch(TryCatchStmt),
    Assert(AssertStmt),

    // Transactions / ACID
    Atomic(AtomicStmt),
    Rollback,
    Commit,

    // Bytecode / low-level
    BytecodeBlock(BytecodeBlock),
}

#[derive(Debug, Clone)]
pub struct LetStmt {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AssignStmt {
    pub target: Expr,
    pub op: AssignOp,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub enum AssignOp { Eq, PlusEq, MinusEq, StarEq, SlashEq }

#[derive(Debug, Clone)]
pub struct IfStmt {
    pub condition: Expr,
    pub then_body: Vec<Stmt>,
    pub else_body: Option<Vec<Stmt>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ForStmt {
    pub binding: String,
    pub iter: Expr,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub condition: Expr,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MatchStmt {
    pub expr: Expr,
    pub arms: Vec<MatchArm>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Expr,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct EmitStmt {
    pub event_name: String,
    pub fields: Vec<(String, Expr)>,
    pub span: Span,
}

// ── Error handling ──────────────────────────────────────────────────────────

/// try { ... } catch (e: Error) { ... } finally { ... }
#[derive(Debug, Clone)]
pub struct TryCatchStmt {
    pub try_body: Vec<Stmt>,
    pub catch_binding: Option<String>,    // the `e` in catch(e)
    pub catch_body: Vec<Stmt>,
    pub finally_body: Option<Vec<Stmt>>,
    pub span: Span,
}

/// assert condition, "message";
#[derive(Debug, Clone)]
pub struct AssertStmt {
    pub condition: Expr,
    pub message: Option<Expr>,
    pub span: Span,
}

// ── Transactions / ACID ─────────────────────────────────────────────────────

/// atomic { ... } — all-or-nothing. Snapshots ctx before, restores on failure.
///
/// ```cubelang
/// atomic {
///     assign x = 12;
///     sub x, 4;
///     sub x, 3;
///     commit;       # finalize — changes are permanent
/// }
/// # if anything throws inside, ctx rolls back to before the atomic block
/// ```
#[derive(Debug, Clone)]
pub struct AtomicStmt {
    pub body: Vec<Stmt>,
    pub span: Span,
}

// ── Bytecode / low-level ────────────────────────────────────────────────────

/// bytecode { ... } — inline VM assembly or raw bytecode.
///
/// Two forms:
/// 1. Inline raw bytes: `bytecode { 0x00 0x02 0x01 0xf8 0x3c ... }`
/// 2. Raw CubeLang asm:  `bytecode { create x : number; assign x = 16; }`
#[derive(Debug, Clone)]
pub struct BytecodeBlock {
    pub kind: BytecodeKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum BytecodeKind {
    /// Inline raw bytes: bytecode { 0x00 0x02 ... }
    Inline(Vec<u8>),
    /// Raw CubeLang asm: bytecode { create x : number; assign x = 16; }
    Asm(Vec<OpcodeStmt>),
}

/// VM opcode statements — compile directly to bytecodes.
#[derive(Debug, Clone)]
pub enum OpcodeStmt {
    Create { reg: String, ty: String },         // create x : number
    Assign { reg: String, value: Expr },        // assign x = 16
    Add { reg: String, value: Expr },           // add x, 3
    Sub { reg: String, value: Expr },           // sub x, 3
    Mul { reg: String, value: Expr },           // mul x, 3
    Div { reg: String, value: Expr },           // div x, 3
    Sum { reg: String },                         // sum x
    Push { reg: String },                        // push x
    Pop { reg: String },                         // pop x
    Query { reg: String },                       // query x
    Remember { reg: String },                    // remember x
    Store { reg: String, key: Expr },           // store x, "key"
    Recall { reg: Option<String>, key: Expr }, // recall "key"  |  recall x, "key"
    Bind { reg: String, role: String, val: Expr }, // bind x, AGENT, val
    Unify { a: String, b: String },             // unify a, b
    BindRole { reg: String, role: String },     // bind_role x, AGENT (self-fill)
    Transfer { src: String, dst: String, amount: Expr }, // transfer a, b, 1
    Compare { a: String, b: String },           // compare a, b
    /// Extended reasoning opcodes (infer, score, detect_pattern, map_roles,
    /// filter, decode, reduce, merge, split, debate, predict, discover, diff,
    /// seq, specialize, reward). Generic: a canonical mnemonic + ordered args.
    Extended { op: ExtOp, args: Vec<ExtArg> },
}

/// An argument to an extended opcode: either a register/name or a value expr.
#[derive(Debug, Clone)]
pub enum ExtArg {
    Reg(String),   // a bare identifier ? a register / role name
    Val(Expr),     // a literal, array, field access, etc.
}

/// Canonical extended opcode identity (carries mnemonic + bytecode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtOp {
    Infer, MapRoles, Filter, Score, DetectPattern, Decode, Reduce, Merge,
    Split, Debate, Predict, Discover, Diff, Seq, Specialize, Reward,
    Match, Push, Sum, Compare, TemporalBind,
    Analogy, Gen, Inst, Broadcast, Explore, Forge, Ask, Sync, Forget,
}

// ── Expressions ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Expr {
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),
    Null,
    Ident(String),
    SelfAccess(String),                        // self.field
    Field(Box<Expr>, String),                  // expr.field
    Index(Box<Expr>, Box<Expr>),               // expr[index]
    Call(Box<Expr>, Vec<Expr>),                // expr(args)
    MethodCall(Box<Expr>, String, Vec<Expr>),  // expr.method(args)
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    UnaryOp(UnaryOp, Box<Expr>),
    StructLit(String, Vec<(String, Expr)>),    // Name { field: val }
    ArrayLit(Vec<Expr>),                       // [a, b, c]
    MapLit(Vec<(Expr, Expr)>),                 // { k: v, ... }
    Lambda(Vec<Param>, Box<Expr>),             // (x) => expr
    Await(Box<Expr>),                          // await expr
    Deploy(String, Vec<Expr>),                 // deploy Program(args)
    Proxy(String, Vec<(String, Expr)>),        // proxy Name(target: expr)
}

#[derive(Debug, Clone, Copy)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, NotEq, Lt, Gt, LtEq, GtEq,
    And, Or,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg,    // -
    Not,    // !
}
