//! CubeLang token definitions.

/// Source location for error reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: u32,
    pub col: u32,
    pub offset: u32,
}

/// A token with its span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ── Literals ─────────────────────────────────────────────
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),

    // ── Keywords: declarations ───────────────────────────────
    Interface,
    Program,
    Container,
    World,
    Modal,
    Robot,
    Extends,
    Implements,
    Struct,
    Enum,
    Type,
    Event,
    Extend,
    Deploy,
    Proxy,
    Grant,
    Revoke,

    // ── Keywords: blocks ─────────────────────────────────────
    Config,
    Io,
    Inputs,
    Outputs,
    Formats,
    Storage,
    Programs,
    Agents,
    Modalities,
    Fusion,
    Sensors,
    Actuators,
    Safety,
    WorldKw,        // `world` as a block keyword (inside WorldContainer)
    Permissions,

    // ── Keywords: functions ──────────────────────────────────
    Function,
    Return,
    Let,
    Const,
    If,
    Else,
    For,
    While,
    Match,
    In,
    Of,
    As,
    Null,
    SelfKw,
    Super,
    Throw,
    Await,
    Emit,
    On,
    From,
    To,
    Override,
    Constructor,

    // ── Keywords: error handling ─────────────────────────────
    Try,            // try { ... } catch (e) { ... } finally { ... }
    Catch,
    Finally,
    Assert,         // assert condition, "message";

    // ── Keywords: transactions / ACID ────────────────────────
    Atomic,         // atomic { ... } — all-or-nothing execution
    Rollback,       // rollback; — undo current atomic block
    Commit,         // commit; — finalize atomic block

    // ── Keywords: bytecode / low-level ───────────────────────
    BytecodeKw,     // bytecode { asm } — inline VM assembly
    Import,         // import "file" (reserved; real top-level import lands in a later task)

    // ── Keywords: encryption / security ────────────────────────
    Encrypt,        // encrypt(data, key) — encrypt data at rest or in transit
    Decrypt,        // decrypt(data, key) — decrypt
    Sealed,         // sealed { ... } — encrypted storage block, decrypted only at access
    CryptoHash,     // hash(data) — BLAKE3 hash
    CryptoSign,     // sign(data, key) — cryptographic signature
    CryptoVerify,   // verify(data, signature, key) — verify signature

    // ── Keywords: debug / logging ──────────────────────────────
    Log,            // log("message") — info level
    Debug,          // debug("message") — debug level, stripped in release
    Warn,           // warn("message") — warning level
    Error,          // error("message") — error level

    // ── Keywords: modifiers ──────────────────────────────────
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

    // ── Keywords: permissions (@ prefixed) ───────────────────
    AtExternal,     // @external
    AtInternal,     // @internal
    AtSystem,       // @system
    AtHook,         // @hook(...)
    AtBefore,       // @before(...)
    AtAfter,        // @after(...)
    AtCron,         // @cron(...)
    AtOnce,         // @once
    AtRestricted,   // @restricted(...)
    AtRatelimit,    // @ratelimit(...)

    // ── Keywords: opcodes (compile directly to bytecodes) ────
    OpCreate,
    OpAssign,
    OpAdd,
    OpSub,
    OpMul,
    OpDiv,
    OpSum,
    OpPush,
    OpPop,
    OpQuery,
    OpRemember,
    OpStore,
    OpRecall,
    OpBind,
    OpUnify,
    OpBindRole,
    OpTransfer,
    OpCompare,
    // extended reasoning opcodes
    OpInfer,
    OpMapRoles,
    OpFilter,
    OpScore,
    OpDetectPattern,
    OpDecode,
    OpReduce,
    OpMerge,
    OpSplit,
    OpDebate,
    OpPredict,
    OpDiscover,
    OpDiff,
    OpSeq,
    OpSpecialize,
    OpReward,
    OpTemporalBind,
    OpCond,
    OpLoop,
    OpAnalogy,
    OpGen,
    OpInst,
    OpBroadcast,
    OpExplore,
    OpForge,
    OpAsk,
    OpSync,
    OpForget,

    // ── Keywords: types ──────────────────────────────────────
    TyU8, TyU16, TyU32, TyU64,
    TyI8, TyI16, TyI32, TyI64,
    TyF32, TyF64,
    TyBool, TyStr, TyByte, TyVoid,
    TyVec, TyEmb, TyRole, TyOpcode,
    TyCtx, TyRag, TyModule, TyMdx,
    TyAgent, TyClonedAgent,
    TyFile, TyUrl, TyDataset,
    TyExternalProgram,
    TyRequest,          // incoming request (headers, body, method, params)
    TyResponse,         // outgoing response (status, headers, body)
    TyArray,        // array<T>
    TyMap,          // map<K,V>
    TySet,          // set<T>
    TyPromise,      // promise<T>
    TyChannel,      // channel<T>
    TyTuple,        // tuple<T1,T2>

    // ── Punctuation ──────────────────────────────────────────
    LParen,         // (
    RParen,         // )
    LBrace,         // {
    RBrace,         // }
    LBracket,       // [
    RBracket,       // ]
    LAngle,         // <
    RAngle,         // >
    Semicolon,      // ;
    Colon,          // :
    Comma,          // ,
    Dot,            // .
    DotDot,         // ..
    Arrow,          // ->
    FatArrow,       // =>
    At,             // @
    Hash,           // #
    Dollar,         // $
    Pipe,           // |
    Ampersand,      // &
    Question,       // ?
    Exclamation,    // !
    Ellipsis,       // ...

    // ── Operators ────────────────────────────────────────────
    Eq,             // =
    EqEq,           // ==
    NotEq,          // !=
    LtEq,          // <=
    GtEq,          // >=
    Plus,           // +
    Minus,          // -
    Star,           // *
    Slash,          // /
    Percent,        // %
    PlusEq,         // +=
    MinusEq,        // -=
    StarEq,         // *=
    SlashEq,        // /=
    AmpAmp,         // &&
    PipePipe,       // ||

    // ── Special ──────────────────────────────────────────────
    Newline,
    Eof,
}

impl TokenKind {
    /// Check if this token is a modifier keyword.
    pub fn is_modifier(&self) -> bool {
        matches!(self,
            TokenKind::Public | TokenKind::Private | TokenKind::Abstract |
            TokenKind::Global | TokenKind::Mutable | TokenKind::Immutable |
            TokenKind::Async | TokenKind::Sequential | TokenKind::Parallel |
            TokenKind::Joined | TokenKind::Pure | TokenKind::Optional |
            TokenKind::Singleton | TokenKind::Override
        )
    }

    /// Check if this token is a permission annotation.
    pub fn is_permission(&self) -> bool {
        matches!(self,
            TokenKind::AtExternal | TokenKind::AtInternal | TokenKind::AtSystem |
            TokenKind::AtHook | TokenKind::AtBefore | TokenKind::AtAfter |
            TokenKind::AtCron | TokenKind::AtOnce | TokenKind::AtRestricted |
            TokenKind::AtRatelimit
        )
    }

    /// Check if this token is an opcode keyword.
    pub fn is_opcode(&self) -> bool {
        matches!(self,
            TokenKind::OpCreate | TokenKind::OpAssign | TokenKind::OpAdd |
            TokenKind::OpSub | TokenKind::OpMul | TokenKind::OpDiv |
            TokenKind::OpSum | TokenKind::OpPush | TokenKind::OpPop |
            TokenKind::OpQuery | TokenKind::OpRemember | TokenKind::OpStore |
            TokenKind::OpRecall | TokenKind::OpBind | TokenKind::OpUnify |
            TokenKind::OpBindRole | TokenKind::OpTransfer | TokenKind::OpCompare |
            TokenKind::OpInfer | TokenKind::OpMapRoles | TokenKind::OpFilter | TokenKind::OpScore | TokenKind::OpDetectPattern | TokenKind::OpDecode | TokenKind::OpReduce | TokenKind::OpMerge | TokenKind::OpSplit | TokenKind::OpDebate | TokenKind::OpPredict | TokenKind::OpDiscover | TokenKind::OpDiff | TokenKind::OpSeq | TokenKind::OpSpecialize | TokenKind::OpReward | TokenKind::OpTemporalBind |
            TokenKind::OpCond | TokenKind::OpLoop |
            TokenKind::OpAnalogy | TokenKind::OpGen | TokenKind::OpInst | TokenKind::OpBroadcast | TokenKind::OpExplore | TokenKind::OpForge | TokenKind::OpAsk | TokenKind::OpSync | TokenKind::OpForget
        )
    }
}
