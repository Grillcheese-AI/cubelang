//! CubeLang recursive descent parser.
//!
//! Consumes a token stream from the lexer and produces an AST.

use crate::ast::*;
use crate::token::{Token, TokenKind, Span};

/// Parse error with location.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.span.line, self.span.col, self.message)
    }
}

type PResult<T> = Result<T, ParseError>;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    fn peek(&self) -> &TokenKind {
        self.skip_newlines_peek()
    }

    fn skip_newlines_peek(&self) -> &TokenKind {
        let mut i = self.pos;
        while i < self.tokens.len() {
            if self.tokens[i].kind != TokenKind::Newline {
                return &self.tokens[i].kind;
            }
            i += 1;
        }
        &TokenKind::Eof
    }

    fn span(&self) -> Span {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].span
        } else {
            Span { line: 0, col: 0, offset: 0 }
        }
    }

    fn advance(&mut self) -> &Token {
        self.skip_newlines();
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        tok
    }

    fn skip_newlines(&mut self) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].kind == TokenKind::Newline {
            self.pos += 1;
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> PResult<Span> {
        self.skip_newlines();
        if self.pos >= self.tokens.len() {
            return Err(self.err(format!("expected {:?}, got EOF", kind)));
        }
        if std::mem::discriminant(&self.tokens[self.pos].kind) == std::mem::discriminant(kind) {
            let span = self.tokens[self.pos].span;
            self.pos += 1;
            Ok(span)
        } else {
            Err(self.err(format!("expected {:?}, got {:?}", kind, self.tokens[self.pos].kind)))
        }
    }

    fn expect_ident(&mut self) -> PResult<String> {
        self.skip_newlines();
        // Accept plain identifiers AND contextual keywords used as names
        let name = match &self.tokens[self.pos].kind {
            TokenKind::Ident(n) => n.clone(),
            // Keywords that can appear in identifier position
            TokenKind::Constructor  => "constructor".into(),
            TokenKind::Config       => "config".into(),
            TokenKind::Program      => "program".into(),
            TokenKind::Container    => "container".into(),
            TokenKind::Type         => "type".into(),
            TokenKind::Event        => "event".into(),
            TokenKind::Storage      => "storage".into(),
            TokenKind::Io           => "io".into(),
            TokenKind::Inputs       => "inputs".into(),
            TokenKind::Outputs      => "outputs".into(),
            TokenKind::Formats      => "formats".into(),
            TokenKind::Programs     => "programs".into(),
            TokenKind::Agents       => "agents".into(),
            TokenKind::Permissions  => "permissions".into(),
            TokenKind::WorldKw      => "world".into(),
            TokenKind::Modal        => "modal".into(),
            TokenKind::Robot        => "robot".into(),
            TokenKind::Import       => "import".into(),
            TokenKind::Use          => "use".into(),
            TokenKind::Log          => "log".into(),
            TokenKind::Debug        => "debug".into(),
            TokenKind::Warn         => "warn".into(),
            TokenKind::Error        => "error".into(),
            TokenKind::TyModule     => "module".into(),
            TokenKind::TyAgent      => "agent".into(),
            TokenKind::TyFile       => "file".into(),
            TokenKind::TyUrl        => "url".into(),
            TokenKind::TyDataset    => "dataset".into(),
            TokenKind::TyRole       => "role".into(),
            TokenKind::CryptoVerify => "verify".into(),
            TokenKind::CryptoHash   => "hash".into(),
            TokenKind::CryptoSign   => "sign".into(),
            TokenKind::Encrypt      => "encrypt".into(),
            TokenKind::Decrypt      => "decrypt".into(),
            TokenKind::Sealed       => "sealed".into(),
            TokenKind::Assert       => "assert".into(),
            TokenKind::Atomic       => "atomic".into(),
            TokenKind::Emit         => "emit".into(),
            TokenKind::Deploy       => "deploy".into(),
            TokenKind::Proxy        => "proxy".into(),
            TokenKind::Grant        => "grant".into(),
            TokenKind::Revoke       => "revoke".into(),
            TokenKind::Extend       => "extend".into(),
            TokenKind::Struct       => "struct".into(),
            TokenKind::Enum         => "enum".into(),
            TokenKind::Interface    => "interface".into(),
            TokenKind::Fusion       => "fusion".into(),
            TokenKind::Sensors      => "sensors".into(),
            TokenKind::Actuators    => "actuators".into(),
            TokenKind::Safety       => "safety".into(),
            TokenKind::Modalities   => "modalities".into(),
            TokenKind::AsmKw        => "asm".into(),
            // Opcodes used as method names (e.g. steps.push(), self.store())
            TokenKind::OpCreate     => "create".into(),
            TokenKind::OpAssign     => "assign".into(),
            TokenKind::OpAdd        => "add".into(),
            TokenKind::OpSub        => "sub".into(),
            TokenKind::OpMul        => "mul".into(),
            TokenKind::OpDiv        => "div".into(),
            TokenKind::OpSum        => "sum".into(),
            TokenKind::OpPush       => "push".into(),
            TokenKind::OpPop        => "pop".into(),
            TokenKind::OpQuery      => "query".into(),
            TokenKind::OpRemember   => "remember".into(),
            TokenKind::OpStore      => "store".into(),
            TokenKind::OpRecall     => "recall".into(),
            TokenKind::OpBind       => "bind".into(),
            TokenKind::OpUnify      => "unify".into(),
            TokenKind::Rollback     => "rollback".into(),
            TokenKind::Commit       => "commit".into(),
            TokenKind::Try          => "try".into(),
            TokenKind::Catch        => "catch".into(),
            TokenKind::Finally      => "finally".into(),
            TokenKind::Throw        => "throw".into(),
            TokenKind::Return       => "return".into(),
            TokenKind::Match        => "match".into(),
            TokenKind::Async        => "async".into(),
            TokenKind::TyResponse   => "response".into(),
            TokenKind::TyRequest    => "request".into(),
            // Extended reasoning opcodes — usable as identifiers everywhere
            // except statement-leading position (where parse_stmt routes them
            // to opcode parsing). This keeps natural names like `diff`, `score`,
            // `match`, `filter`, `split` available as variables/types/fields.
            TokenKind::OpSeq           => "seq".into(),
            TokenKind::OpDiff          => "diff".into(),
            TokenKind::OpDetectPattern => "detect_pattern".into(),
            TokenKind::OpPredict       => "predict".into(),
            TokenKind::OpDebate        => "debate".into(),
            TokenKind::OpDiscover      => "discover".into(),
            TokenKind::OpDecode        => "decode".into(),
            TokenKind::OpScore         => "score".into(),
            TokenKind::OpSpecialize    => "specialize".into(),
            TokenKind::OpReward        => "reward".into(),
            TokenKind::OpInfer         => "infer".into(),
            TokenKind::OpMerge         => "merge".into(),
            TokenKind::OpSplit         => "split".into(),
            TokenKind::OpFilter        => "filter".into(),
            TokenKind::OpMapRoles      => "map_roles".into(),
            TokenKind::OpReduce        => "reduce".into(),
            TokenKind::OpTemporalBind  => "temporal_bind".into(),
            TokenKind::OpAnalogy       => "analogy".into(),
            TokenKind::OpGen           => "gen".into(),
            TokenKind::OpInst          => "inst".into(),
            TokenKind::OpBroadcast     => "broadcast".into(),
            TokenKind::OpExplore       => "explore".into(),
            TokenKind::OpForge         => "forge".into(),
            TokenKind::OpAsk           => "ask".into(),
            TokenKind::OpSync          => "sync".into(),
            TokenKind::OpForget        => "forget".into(),
            TokenKind::OpCond          => "cond".into(),
            TokenKind::OpLoop          => "loop".into(),
            TokenKind::OpTransfer      => "transfer".into(),
            TokenKind::OpCompare       => "compare".into(),
            TokenKind::OpBindRole      => "bind_role".into(),

            // Type keywords — also soft. Many are ordinary English words a
            // programmer will reach for as a variable name: `ctx`, `role`,
            // `map`, `set`, `file`, `url`, `request`, `response`, `agent`.
            // The SPEC's own example cannot otherwise be written:
            //     let ctx: rag = knowledge.query("...", top_k: 5);
            // Position disambiguates: after `create`/`let`/`.` an identifier is
            // required, after `:` a type is. `create ctx : ctx;` parses.
            TokenKind::TyCtx             => "ctx".into(),
            TokenKind::TyRag             => "rag".into(),
            TokenKind::TyRole            => "role".into(),
            TokenKind::TyOpcode          => "opcode".into(),
            TokenKind::TyModule          => "module".into(),
            TokenKind::TyMdx             => "mdx".into(),
            TokenKind::TyAgent           => "agent".into(),
            TokenKind::TyClonedAgent     => "cloned_agent".into(),
            TokenKind::TyFile            => "file".into(),
            TokenKind::TyUrl             => "url".into(),
            TokenKind::TyDataset         => "dataset".into(),
            TokenKind::TyExternalProgram => "external_program".into(),
            TokenKind::TyRequest         => "request".into(),
            TokenKind::TyResponse        => "response".into(),
            TokenKind::TyArray           => "array".into(),
            TokenKind::TyMap             => "map".into(),
            TokenKind::TySet             => "set".into(),
            TokenKind::TyPromise         => "promise".into(),
            TokenKind::TyChannel         => "channel".into(),
            TokenKind::TyTuple           => "tuple".into(),
            TokenKind::TyVec             => "vec".into(),
            TokenKind::TyEmb             => "emb".into(),
            TokenKind::TyByte            => "byte".into(),
            // Primitive width/scalar names are left RESERVED on purpose. A
            // variable called `u8` or `bool` is a bug wearing a name.

            _ => return Err(self.err(format!("expected identifier, got {:?}", self.tokens[self.pos].kind))),
        };
        self.pos += 1;
        Ok(name)
    }

    /// True if `kind` can stand in as an identifier in expression / name
    /// position — any opcode keyword, plus the non-primitive type keywords.
    /// These are contextual: an opcode at a statement start parses as an opcode
    /// (via parse_stmt's dispatch), but as a value/name elsewhere it is an
    /// ordinary identifier. Likewise `ctx`/`map`/`role`/... are types after `:`
    /// and identifiers everywhere else.
    ///
    /// Must agree with the soft-keyword arms in `expect_ident`, or a name will
    /// parse in one position and not the other.
    fn kind_is_ident_like(&self, kind: &TokenKind) -> bool {
        if kind.is_opcode() {
            return true;
        }
        matches!(
            kind,
            TokenKind::TyCtx
                | TokenKind::TyRag
                | TokenKind::TyRole
                | TokenKind::TyOpcode
                | TokenKind::TyModule
                | TokenKind::TyMdx
                | TokenKind::TyAgent
                | TokenKind::TyClonedAgent
                | TokenKind::TyFile
                | TokenKind::TyUrl
                | TokenKind::TyDataset
                | TokenKind::TyExternalProgram
                | TokenKind::TyRequest
                | TokenKind::TyResponse
                | TokenKind::TyArray
                | TokenKind::TyMap
                | TokenKind::TySet
                | TokenKind::TyPromise
                | TokenKind::TyChannel
                | TokenKind::TyTuple
                | TokenKind::TyVec
                | TokenKind::TyEmb
                | TokenKind::TyByte
        )
    }

    fn expect_string(&mut self) -> PResult<String> {
        self.skip_newlines();
        if let TokenKind::StringLit(s) = &self.tokens[self.pos].kind {
            let s = s.clone();
            self.pos += 1;
            Ok(s)
        } else {
            Err(self.err(format!("expected string literal, got {:?}", self.tokens[self.pos].kind)))
        }
    }

    fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(kind)
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn err(&self, message: String) -> ParseError {
        ParseError { message, span: self.span() }
    }

    // ── Top-level ───────────────────────────────────────────────────────

    pub fn parse(&mut self) -> PResult<SourceFile> {
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                TokenKind::Eof => break,
                TokenKind::Interface => items.push(TopLevel::Interface(self.parse_interface()?)),
                TokenKind::Program => items.push(TopLevel::Program(self.parse_program()?)),
                TokenKind::Container | TokenKind::WorldKw | TokenKind::Modal | TokenKind::Robot => {
                    items.push(TopLevel::Container(self.parse_container()?));
                }
                TokenKind::Struct => items.push(TopLevel::Struct(self.parse_struct()?)),
                TokenKind::Enum => items.push(TopLevel::Enum(self.parse_enum()?)),
                TokenKind::Type => items.push(TopLevel::TypeAlias(self.parse_type_alias()?)),
                TokenKind::Event => items.push(TopLevel::EventDecl(self.parse_event()?)),
                TokenKind::Extend => items.push(TopLevel::ExtendBlock(self.parse_extend()?)),
                TokenKind::Use => items.push(TopLevel::Use(self.parse_use()?)),
                TokenKind::Import => items.push(TopLevel::Import(self.parse_import()?)),
                _ => return Err(self.err(format!("unexpected token at top level: {:?}", self.peek()))),
            }
        }
        Ok(SourceFile { items })
    }

    // ── Import (Task 5: external user-file modularity) ────────────────────

    /// `import "path.cube";` — a top-level declaration naming another user
    /// file whose top-level declarations should be pulled into this
    /// compilation. Just the path: resolution (relative to the FILE THAT
    /// DECLARES THIS import, not necessarily the entry file), cycle
    /// detection, and the actual AST-level merge are the loader's job
    /// (`crate::loader`), not the parser's — mirrors how `parse_use` only
    /// extracts the bare name and leaves registry resolution to
    /// `compiler.rs`. The old in-body `import "x" as y` / `import {a,b}
    /// from "x"` ES-module shape (`Stmt::Import`/`ImportStmt`) was deleted
    /// in Task 1 for being the wrong shape (a statement can't introduce new
    /// top-level names) — this is intentionally simpler: one bare string,
    /// top-level only, no `as`/`from`/selective-import syntax.
    fn parse_import(&mut self) -> PResult<String> {
        self.expect(&TokenKind::Import)?;
        let path = self.expect_string()?;
        self.eat(&TokenKind::Semicolon);
        Ok(path)
    }

    // ── Use (Task 3: VM-internal capability/module system) ────────────────

    /// `use <name>;` — a top-level declaration bringing a VM registry
    /// module into scope. Just the module name: the registry itself is
    /// name-addressed (`vm::registry::ModuleRegistry`), so there is nothing
    /// else to parse here yet (no path, no selective import list).
    fn parse_use(&mut self) -> PResult<String> {
        self.expect(&TokenKind::Use)?;
        let name = self.expect_ident()?;
        self.eat(&TokenKind::Semicolon);
        Ok(name)
    }

    // ── Interface ───────────────────────────────────────────────────────

    fn parse_interface(&mut self) -> PResult<InterfaceDecl> {
        let span = self.span();
        self.expect(&TokenKind::Interface)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut members = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }

            // type Input;
            if self.at(&TokenKind::Type) {
                self.advance();
                let tname = self.expect_ident()?;
                self.expect(&TokenKind::Semicolon)?;
                members.push(InterfaceMember::TypeDecl { name: tname, span: self.span() });
                continue;
            }

            // [permissions] [modifiers] function name(...): Type;
            let sig = self.parse_function_sig()?;
            self.expect(&TokenKind::Semicolon)?;
            members.push(InterfaceMember::Function(sig));
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(InterfaceDecl { name, members, span })
    }

    // ── Program ─────────────────────────────────────────────────────────

    fn parse_program(&mut self) -> PResult<ProgramDecl> {
        let span = self.span();
        self.expect(&TokenKind::Program)?;
        let name = self.expect_ident()?;

        let implements = if self.eat(&TokenKind::Implements) {
            self.parse_ident_list()?
        } else {
            Vec::new()
        };

        self.expect(&TokenKind::LBrace)?;

        let mut body = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }

            match self.peek() {
                TokenKind::Type => {
                    body.push(ProgramItem::TypeAlias(self.parse_type_alias()?));
                }
                TokenKind::Storage | TokenKind::Global => {
                    body.push(ProgramItem::Storage(self.parse_storage_block()?));
                }
                TokenKind::Io => {
                    // skip IO block for now — programs can have it
                    self.parse_io_block()?;
                }
                _ => {
                    // Must be a function (with optional permissions/modifiers)
                    let func = self.parse_function_decl()?;
                    if func.sig.name == "constructor" {
                        body.push(ProgramItem::Constructor(func));
                    } else {
                        body.push(ProgramItem::Function(func));
                    }
                }
            }
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(ProgramDecl { name, implements, body, span })
    }

    // ── Container ───────────────────────────────────────────────────────

    fn parse_container(&mut self) -> PResult<ContainerDecl> {
        let span = self.span();
        let kind = match self.peek() {
            TokenKind::Container => { self.advance(); ContainerKind::Container }
            TokenKind::WorldKw   => { self.advance(); ContainerKind::World }
            TokenKind::Modal     => { self.advance(); ContainerKind::Modal }
            TokenKind::Robot     => { self.advance(); ContainerKind::Robot }
            _ => return Err(self.err("expected container keyword".into())),
        };

        let name = self.expect_ident()?;

        let extends = if self.eat(&TokenKind::Extends) {
            Some(self.expect_ident()?)
        } else {
            None
        };

        let implements = if self.eat(&TokenKind::Implements) {
            self.parse_ident_list()?
        } else {
            Vec::new()
        };

        self.expect(&TokenKind::LBrace)?;

        let mut body = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }

            match self.peek() {
                TokenKind::Config => {
                    self.advance();
                    self.expect(&TokenKind::LBrace)?;
                    let fields = self.parse_config_fields()?;
                    self.expect(&TokenKind::RBrace)?;
                    body.push(ContainerItem::Config(fields));
                }
                TokenKind::Io => {
                    body.push(ContainerItem::Io(self.parse_io_block()?));
                }
                TokenKind::Storage | TokenKind::Global => {
                    body.push(ContainerItem::Storage(self.parse_storage_block()?));
                }
                TokenKind::Programs => {
                    self.advance();
                    self.expect(&TokenKind::LBrace)?;
                    let bindings = self.parse_bindings()?;
                    self.expect(&TokenKind::RBrace)?;
                    body.push(ContainerItem::Programs(bindings.into_iter().map(|(n, e, s)| {
                        ProgramBinding { name: n, expr: e, span: s }
                    }).collect()));
                }
                TokenKind::Agents => {
                    self.advance();
                    self.expect(&TokenKind::LBrace)?;
                    let bindings = self.parse_bindings()?;
                    self.expect(&TokenKind::RBrace)?;
                    body.push(ContainerItem::Agents(bindings.into_iter().map(|(n, e, s)| {
                        AgentBinding { name: n, expr: e, span: s }
                    }).collect()));
                }
                TokenKind::Permissions => {
                    self.advance();
                    self.expect(&TokenKind::LBrace)?;
                    // skip permission parsing for now
                    self.skip_block()?;
                    body.push(ContainerItem::Permissions(Vec::new()));
                }
                _ => {
                    let func = self.parse_function_decl()?;
                    body.push(ContainerItem::Function(func));
                }
            }
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(ContainerDecl { kind, name, extends, implements, body, span })
    }

    // ── Struct ──────────────────────────────────────────────────────────

    fn parse_struct(&mut self) -> PResult<StructDecl> {
        let span = self.span();
        self.expect(&TokenKind::Struct)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type()?;
            let default = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.eat(&TokenKind::Semicolon);
            fields.push(StructField { name: fname, ty, default, span: self.span() });
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(StructDecl { name, fields, span })
    }

    // ── Enum ────────────────────────────────────────────────────────────

    fn parse_enum(&mut self) -> PResult<EnumDecl> {
        let span = self.span();
        self.expect(&TokenKind::Enum)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut variants = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            let vname = self.expect_ident()?;

            let value = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };

            let data = if self.eat(&TokenKind::LParen) {
                let mut types = Vec::new();
                loop {
                    if self.at(&TokenKind::RParen) { break; }
                    types.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) { break; }
                }
                self.expect(&TokenKind::RParen)?;
                Some(types)
            } else {
                None
            };

            self.eat(&TokenKind::Comma);
            variants.push(EnumVariant { name: vname, value, data, span: self.span() });
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(EnumDecl { name, variants, span })
    }

    // ── Type alias ──────────────────────────────────────────────────────

    fn parse_type_alias(&mut self) -> PResult<TypeAliasDecl> {
        let span = self.span();
        self.expect(&TokenKind::Type)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Eq)?;
        let ty = self.parse_type()?;
        self.eat(&TokenKind::Semicolon);
        Ok(TypeAliasDecl { name, ty, span })
    }

    // ── Event ───────────────────────────────────────────────────────────

    fn parse_event(&mut self) -> PResult<EventDecl> {
        let span = self.span();
        self.expect(&TokenKind::Event)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type()?;
            self.eat(&TokenKind::Semicolon);
            fields.push(StructField { name: fname, ty, default: None, span: self.span() });
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(EventDecl { name, fields, span })
    }

    // ── Extend ──────────────────────────────────────────────────────────

    fn parse_extend(&mut self) -> PResult<ExtendBlock> {
        let span = self.span();
        self.expect(&TokenKind::Extend)?;
        let target = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut functions = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            functions.push(self.parse_function_decl()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(ExtendBlock { target, functions, span })
    }

    // ── Storage block ───────────────────────────────────────────────────

    fn parse_storage_block(&mut self) -> PResult<StorageBlock> {
        let span = self.span();
        let is_global = self.eat(&TokenKind::Global);
        self.expect(&TokenKind::Storage)?;
        self.expect(&TokenKind::LBrace)?;

        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let mutable = self.eat(&TokenKind::Mutable);
            if !mutable { self.eat(&TokenKind::Immutable); }
            let ty = self.parse_type()?;
            let default = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.eat(&TokenKind::Semicolon);
            fields.push(StorageField { name: fname, mutable, ty, default, span: self.span() });
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(StorageBlock { is_global, fields, span })
    }

    // ── IO block ────────────────────────────────────────────────────────

    fn parse_io_block(&mut self) -> PResult<IoBlock> {
        let span = self.span();
        self.expect(&TokenKind::Io)?;
        self.expect(&TokenKind::LBrace)?;
        // For now, skip the contents
        self.skip_block()?;
        Ok(IoBlock { inputs: Vec::new(), outputs: Vec::new(), formats: None, span })
    }

    // ── Function signature (no body) ────────────────────────────────────

    fn parse_function_sig(&mut self) -> PResult<FunctionSig> {
        let span = self.span();
        let permissions = self.parse_permissions()?;
        let modifiers = self.parse_modifiers()?;

        // "function" or "constructor"
        let name = if self.eat(&TokenKind::Function) {
            self.expect_ident()?
        } else if self.eat(&TokenKind::Constructor) {
            "constructor".to_string()
        } else {
            return Err(self.err(format!("expected 'function' or 'constructor', got {:?}", self.peek())));
        };

        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;

        let return_type = if self.eat(&TokenKind::Colon) {
            self.parse_type()?
        } else {
            TypeExpr::Void
        };

        // Task 3: POSTFIX `override` marker — `function name() override {`
        // or `function name(): T override {`. See `FunctionSig::is_override`
        // for why this is a distinct field/grammar position from the
        // pre-existing PREFIX `override` in `modifiers`.
        let is_override = self.eat(&TokenKind::Override);

        Ok(FunctionSig { permissions, modifiers, name, params, return_type, is_override, span })
    }

    // ── Function declaration (with body) ────────────────────────────────

    fn parse_function_decl(&mut self) -> PResult<FunctionDecl> {
        let sig = self.parse_function_sig()?;
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_stmt_block()?;
        self.expect(&TokenKind::RBrace)?;
        Ok(FunctionDecl { sig, body })
    }

    // ── Permissions (@external, @system, etc.) ──────────────────────────

    fn parse_permissions(&mut self) -> PResult<Vec<Permission>> {
        let mut perms = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                TokenKind::AtExternal => { self.advance(); perms.push(Permission::External); }
                TokenKind::AtInternal => { self.advance(); perms.push(Permission::Internal); }
                TokenKind::AtSystem   => { self.advance(); perms.push(Permission::System); }
                TokenKind::AtOnce     => { self.advance(); perms.push(Permission::Once); }
                TokenKind::AtHook => {
                    self.advance();
                    self.expect(&TokenKind::LParen)?;
                    let target = self.expect_ident()?;
                    let full = if self.eat(&TokenKind::Dot) {
                        let event = self.expect_ident()?;
                        format!("{}.{}", target, event)
                    } else {
                        target
                    };
                    self.expect(&TokenKind::RParen)?;
                    perms.push(Permission::Hook(full));
                }
                TokenKind::AtBefore => {
                    self.advance();
                    self.expect(&TokenKind::LParen)?;
                    let name = self.expect_ident()?;
                    self.expect(&TokenKind::RParen)?;
                    perms.push(Permission::Before(name));
                }
                TokenKind::AtAfter => {
                    self.advance();
                    self.expect(&TokenKind::LParen)?;
                    let name = self.expect_ident()?;
                    self.expect(&TokenKind::RParen)?;
                    perms.push(Permission::After(name));
                }
                TokenKind::AtCron => {
                    self.advance();
                    self.expect(&TokenKind::LParen)?;
                    let interval = self.expect_string()?;
                    self.expect(&TokenKind::RParen)?;
                    perms.push(Permission::Cron(interval));
                }
                _ => break,
            }
        }
        Ok(perms)
    }

    // ── Modifiers ───────────────────────────────────────────────────────

    fn parse_modifiers(&mut self) -> PResult<Vec<Modifier>> {
        let mut mods = Vec::new();
        loop {
            self.skip_newlines();
            let m = match self.peek() {
                TokenKind::Public     => Modifier::Public,
                TokenKind::Private    => Modifier::Private,
                TokenKind::Abstract   => Modifier::Abstract,
                TokenKind::Global     => Modifier::Global,
                TokenKind::Mutable    => Modifier::Mutable,
                TokenKind::Immutable  => Modifier::Immutable,
                TokenKind::Async      => Modifier::Async,
                TokenKind::Sequential => Modifier::Sequential,
                TokenKind::Parallel   => Modifier::Parallel,
                TokenKind::Joined     => Modifier::Joined,
                TokenKind::Pure       => Modifier::Pure,
                TokenKind::Optional   => Modifier::Optional,
                TokenKind::Singleton  => Modifier::Singleton,
                TokenKind::Override   => Modifier::Override,
                _ => break,
            };
            self.advance();
            mods.push(m);
        }
        Ok(mods)
    }

    // ── Parameters ──────────────────────────────────────────────────────

    fn parse_params(&mut self) -> PResult<Vec<Param>> {
        let mut params = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RParen) { break; }
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type()?;
            params.push(Param { name, ty, span: self.span() });
            if !self.eat(&TokenKind::Comma) { break; }
        }
        Ok(params)
    }

    // ── Types ───────────────────────────────────────────────────────────

    fn parse_type(&mut self) -> PResult<TypeExpr> {
        let base = self.parse_base_type()?;

        // Union: T | U
        if self.at(&TokenKind::Pipe) {
            let mut types = vec![base];
            while self.eat(&TokenKind::Pipe) {
                types.push(self.parse_base_type()?);
            }
            return Ok(TypeExpr::Union(types));
        }

        // Nullable: T?
        if self.eat(&TokenKind::Question) {
            return Ok(TypeExpr::Nullable(Box::new(base)));
        }

        Ok(base)
    }

    fn parse_base_type(&mut self) -> PResult<TypeExpr> {
        self.skip_newlines();
        match self.peek().clone() {
            TokenKind::TyVoid => { self.advance(); Ok(TypeExpr::Void) }

            // Generic container types: array<T>, map<K,V>, etc.
            TokenKind::TyArray => {
                self.advance();
                self.expect(&TokenKind::LAngle)?;
                let inner = self.parse_type()?;
                self.expect(&TokenKind::RAngle)?;
                Ok(TypeExpr::Array(Box::new(inner)))
            }
            TokenKind::TyMap => {
                self.advance();
                self.expect(&TokenKind::LAngle)?;
                let k = self.parse_type()?;
                self.expect(&TokenKind::Comma)?;
                let v = self.parse_type()?;
                self.expect(&TokenKind::RAngle)?;
                Ok(TypeExpr::Map(Box::new(k), Box::new(v)))
            }
            TokenKind::TySet => {
                self.advance();
                self.expect(&TokenKind::LAngle)?;
                let inner = self.parse_type()?;
                self.expect(&TokenKind::RAngle)?;
                Ok(TypeExpr::Set(Box::new(inner)))
            }
            TokenKind::TyPromise => {
                self.advance();
                self.expect(&TokenKind::LAngle)?;
                let inner = self.parse_type()?;
                self.expect(&TokenKind::RAngle)?;
                Ok(TypeExpr::Promise(Box::new(inner)))
            }
            TokenKind::TyChannel => {
                self.advance();
                self.expect(&TokenKind::LAngle)?;
                let inner = self.parse_type()?;
                self.expect(&TokenKind::RAngle)?;
                Ok(TypeExpr::Channel(Box::new(inner)))
            }
            TokenKind::TyTuple => {
                self.advance();
                self.expect(&TokenKind::LAngle)?;
                let mut types = Vec::new();
                loop {
                    if self.at(&TokenKind::RAngle) { break; }
                    types.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) { break; }
                }
                self.expect(&TokenKind::RAngle)?;
                Ok(TypeExpr::Tuple(types))
            }

            // All named types (built-in + user-defined)
            _ => {
                let name = self.parse_type_name()?;
                Ok(TypeExpr::Named(name))
            }
        }
    }

    fn parse_type_name(&mut self) -> PResult<String> {
        self.skip_newlines();
        let name = match &self.tokens[self.pos].kind {
            TokenKind::Ident(n) => n.clone(),
            TokenKind::TyU8 => "u8".into(), TokenKind::TyU16 => "u16".into(),
            TokenKind::TyU32 => "u32".into(), TokenKind::TyU64 => "u64".into(),
            TokenKind::TyI8 => "i8".into(), TokenKind::TyI16 => "i16".into(),
            TokenKind::TyI32 => "i32".into(), TokenKind::TyI64 => "i64".into(),
            TokenKind::TyF32 => "f32".into(), TokenKind::TyF64 => "f64".into(),
            TokenKind::TyBool => "bool".into(), TokenKind::TyStr => "str".into(),
            TokenKind::TyByte => "byte".into(),
            TokenKind::TyVec => "vec".into(), TokenKind::TyEmb => "emb".into(),
            TokenKind::TyRole => "role".into(), TokenKind::TyOpcode => "opcode".into(),
            TokenKind::TyCtx => "ctx".into(), TokenKind::TyRag => "rag".into(),
            TokenKind::TyModule => "module".into(), TokenKind::TyMdx => "mdx".into(),
            TokenKind::TyAgent => "agent".into(), TokenKind::TyClonedAgent => "cloned_agent".into(),
            TokenKind::TyFile => "file".into(), TokenKind::TyUrl => "url".into(),
            TokenKind::TyDataset => "dataset".into(),
            TokenKind::TyExternalProgram => "external_program".into(),
            TokenKind::TyRequest => "request".into(), TokenKind::TyResponse => "response".into(),
            TokenKind::Function => "function".into(),
            TokenKind::Null => "null".into(),
            _ => return Err(self.err(format!("expected type name, got {:?}", self.tokens[self.pos].kind))),
        };
        self.pos += 1;
        Ok(name)
    }

    // ── Statements ──────────────────────────────────────────────────────

    fn parse_stmt_block(&mut self) -> PResult<Vec<Stmt>> {
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) || self.at(&TokenKind::Eof) { break; }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        self.skip_newlines();
        match self.peek().clone() {
            TokenKind::Let | TokenKind::Const => self.parse_let_stmt(),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::While => self.parse_while_stmt(),
            TokenKind::Match => self.parse_match_stmt(),
            TokenKind::Return => { self.advance(); let e = if self.at(&TokenKind::Semicolon) || self.at(&TokenKind::RBrace) { None } else { Some(self.parse_expr()?) }; self.eat(&TokenKind::Semicolon); Ok(Stmt::Return(e)) }
            TokenKind::Throw => { self.advance(); let e = self.parse_expr()?; self.eat(&TokenKind::Semicolon); Ok(Stmt::Throw(e)) }
            TokenKind::Emit => self.parse_emit_stmt(),
            TokenKind::Atomic => self.parse_atomic_stmt(),
            TokenKind::AsmKw => self.parse_asm_stmt(),
            TokenKind::Rollback => { self.advance(); self.eat(&TokenKind::Semicolon); Ok(Stmt::Rollback) }
            TokenKind::Commit => { self.advance(); self.eat(&TokenKind::Semicolon); Ok(Stmt::Commit) }
            TokenKind::Assert => self.parse_assert_stmt(),
            TokenKind::Try => self.parse_try_catch_stmt(),

            TokenKind::OpCond => self.parse_cond_stmt(),
            TokenKind::OpLoop => self.parse_loop_stmt(),
            // Opcode statements
            tok if TokenKind::OpCreate == tok => self.parse_opcode_stmt(),
            tok if matches!(tok, TokenKind::OpAssign | TokenKind::OpAdd | TokenKind::OpSub |
                TokenKind::OpMul | TokenKind::OpDiv | TokenKind::OpSum | TokenKind::OpPush |
                TokenKind::OpPop | TokenKind::OpQuery | TokenKind::OpRemember | TokenKind::OpStore |
                TokenKind::OpRecall | TokenKind::OpBind | TokenKind::OpUnify |
                TokenKind::OpBindRole | TokenKind::OpTransfer | TokenKind::OpCompare |
                TokenKind::OpInfer | TokenKind::OpMapRoles | TokenKind::OpFilter | TokenKind::OpScore | TokenKind::OpDetectPattern | TokenKind::OpDecode | TokenKind::OpReduce | TokenKind::OpMerge | TokenKind::OpSplit | TokenKind::OpDebate | TokenKind::OpPredict | TokenKind::OpDiscover | TokenKind::OpDiff | TokenKind::OpSeq | TokenKind::OpSpecialize | TokenKind::OpReward | TokenKind::OpTemporalBind | TokenKind::OpAnalogy | TokenKind::OpGen | TokenKind::OpInst | TokenKind::OpBroadcast | TokenKind::OpExplore | TokenKind::OpForge | TokenKind::OpAsk | TokenKind::OpSync | TokenKind::OpForget) => self.parse_opcode_stmt(),

            // Logging
            TokenKind::Log | TokenKind::Debug | TokenKind::Warn | TokenKind::Error => {
                let e = self.parse_expr()?;
                self.eat(&TokenKind::Semicolon);
                Ok(Stmt::Expr(e))
            }

            // Expression statement (assignments, calls, etc.)
            _ => {
                let e = self.parse_expr()?;
                // Check for assignment operators
                match self.peek() {
                    TokenKind::Eq | TokenKind::PlusEq | TokenKind::MinusEq |
                    TokenKind::StarEq | TokenKind::SlashEq => {
                        let op = match self.peek() {
                            TokenKind::Eq => AssignOp::Eq,
                            TokenKind::PlusEq => AssignOp::PlusEq,
                            TokenKind::MinusEq => AssignOp::MinusEq,
                            TokenKind::StarEq => AssignOp::StarEq,
                            TokenKind::SlashEq => AssignOp::SlashEq,
                            _ => unreachable!(),
                        };
                        let span = self.span();
                        self.advance();
                        let value = self.parse_expr()?;
                        self.eat(&TokenKind::Semicolon);
                        Ok(Stmt::Assign(AssignStmt { target: e, op, value, span }))
                    }
                    _ => {
                        self.eat(&TokenKind::Semicolon);
                        Ok(Stmt::Expr(e))
                    }
                }
            }
        }
    }

    fn parse_let_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.advance(); // let or const
        let name = self.expect_ident()?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq)?;
        let value = self.parse_expr()?;
        self.eat(&TokenKind::Semicolon);
        Ok(Stmt::Let(LetStmt { name, ty, value, span }))
    }

    fn parse_if_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::If)?;
        self.expect(&TokenKind::LParen)?;
        let condition = self.parse_expr()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::LBrace)?;
        let then_body = self.parse_stmt_block()?;
        self.expect(&TokenKind::RBrace)?;
        let else_body = if self.eat(&TokenKind::Else) {
            self.expect(&TokenKind::LBrace)?;
            let body = self.parse_stmt_block()?;
            self.expect(&TokenKind::RBrace)?;
            Some(body)
        } else {
            None
        };
        Ok(Stmt::If(IfStmt { condition, then_body, else_body, span }))
    }

    fn parse_for_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::For)?;
        self.expect(&TokenKind::LParen)?;
        // for (let x of iter) or for (let x in iter)
        self.expect(&TokenKind::Let)?;
        let binding = self.expect_ident()?;
        if !self.eat(&TokenKind::Of) { self.expect(&TokenKind::In)?; }
        let iter = self.parse_expr()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_stmt_block()?;
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::For(ForStmt { binding, iter, body, span }))
    }

    fn parse_while_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::While)?;
        // Parentheses optional for while
        let has_paren = self.eat(&TokenKind::LParen);
        let condition = self.parse_expr()?;
        if has_paren { self.expect(&TokenKind::RParen)?; }
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_stmt_block()?;
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::While(WhileStmt { condition, body, span }))
    }

    fn parse_match_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::Match)?;
        // Opcode form: `match a, b` (no parens) ? MATCH target candidates (0x1B).
        // Distinct from the statement form `match (expr) { arms }`.
        if !self.at(&TokenKind::LParen) {
            let args = self.parse_ext_args()?;
            self.eat(&TokenKind::Semicolon);
            return Ok(Stmt::Opcode(OpcodeStmt::Extended {
                op: ExtOp::Match,
                args,
            }));
        }
        self.expect(&TokenKind::LParen)?;
        let expr = self.parse_expr()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::LBrace)?;
        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            let pattern = self.parse_expr()?;
            self.expect(&TokenKind::FatArrow)?;
            // Single statement or block
            if self.at(&TokenKind::LBrace) {
                self.advance();
                let body = self.parse_stmt_block()?;
                self.expect(&TokenKind::RBrace)?;
                arms.push(MatchArm { pattern, body });
            } else {
                let stmt = self.parse_stmt()?;
                arms.push(MatchArm { pattern, body: vec![stmt] });
            }
            self.eat(&TokenKind::Semicolon);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::Match(MatchStmt { expr, arms, span }))
    }

    fn parse_emit_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::Emit)?;
        let event_name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let val = self.parse_expr()?;
            self.eat(&TokenKind::Comma);
            fields.push((fname, val));
        }
        self.expect(&TokenKind::RBrace)?;
        self.eat(&TokenKind::Semicolon);
        Ok(Stmt::Emit(EmitStmt { event_name, fields, span }))
    }

    fn parse_atomic_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::Atomic)?;
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_stmt_block()?;
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::Atomic(AtomicStmt { body, span }))
    }

    // ── Task 6: `asm { MNEMONIC operand, ...; ... }` ─────────────────────

    /// `asm { ... }` — a flat sequence of `MNEMONIC operand, operand, ...`
    /// lines, each terminated by an optional `;` (the brief's own example
    /// omits the trailing one before the closing `}`, matching every other
    /// block-statement's tolerance for a missing final semicolon).
    fn parse_asm_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::AsmKw)?;
        self.expect(&TokenKind::LBrace)?;
        let mut instrs = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            instrs.push(self.parse_asm_instr()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::Asm(AsmBlock { instrs, span }))
    }

    /// One `MNEMONIC operand, operand, ...` line. The mnemonic is not
    /// validated here — an unrecognized one is a COMPILE error from
    /// `compiler.rs`'s mnemonic table (which can name the exact bad
    /// spelling with a span), not a parse error, so `asm` stays a single,
    /// uniform grammar regardless of how many mnemonics the op table knows.
    fn parse_asm_instr(&mut self) -> PResult<AsmInstr> {
        let span = self.span();
        let mnemonic = self.expect_ident()?;
        let mut operands = Vec::new();
        if !self.at(&TokenKind::Semicolon) && !self.at(&TokenKind::RBrace) {
            loop {
                operands.push(self.parse_asm_operand()?);
                if self.eat(&TokenKind::Comma) { continue; }
                break;
            }
        }
        self.eat(&TokenKind::Semicolon);
        Ok(AsmInstr { mnemonic, operands, span })
    }

    /// Classify one `asm` operand by its own lexical form (see
    /// `ast::AsmOperand`'s doc for the exact rule) — independent of the
    /// mnemonic it belongs to.
    fn parse_asm_operand(&mut self) -> PResult<AsmOperand> {
        match self.peek().clone() {
            TokenKind::StringLit(s) => { self.advance(); Ok(AsmOperand::Global(s)) }
            TokenKind::IntLit(n) => { self.advance(); Ok(AsmOperand::Imm(n)) }
            TokenKind::FloatLit(f) => { self.advance(); Ok(AsmOperand::FloatImm(f)) }
            _ => {
                // Falls through to `expect_ident`, which also tolerates the
                // same contextual keywords `parse_reg_ref` does (e.g. a
                // register literally named `event`) -- an operand that is
                // genuinely unparseable as any of the above surfaces as a
                // normal parse error from there.
                let name = self.expect_ident()?;
                if is_role_bareword(&name) {
                    Ok(AsmOperand::Role(name))
                } else {
                    Ok(AsmOperand::Named(name))
                }
            }
        }
    }

    fn parse_assert_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::Assert)?;
        let condition = self.parse_expr()?;
        let message = if self.eat(&TokenKind::Comma) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.eat(&TokenKind::Semicolon);
        Ok(Stmt::Assert(AssertStmt { condition, message, span }))
    }

    fn parse_try_catch_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::Try)?;
        self.expect(&TokenKind::LBrace)?;
        let try_body = self.parse_stmt_block()?;
        self.expect(&TokenKind::RBrace)?;

        self.expect(&TokenKind::Catch)?;
        let catch_binding = if self.eat(&TokenKind::LParen) {
            let name = self.expect_ident()?;
            self.expect(&TokenKind::RParen)?;
            Some(name)
        } else {
            None
        };
        self.expect(&TokenKind::LBrace)?;
        let catch_body = self.parse_stmt_block()?;
        self.expect(&TokenKind::RBrace)?;

        let finally_body = if self.eat(&TokenKind::Finally) {
            self.expect(&TokenKind::LBrace)?;
            let body = self.parse_stmt_block()?;
            self.expect(&TokenKind::RBrace)?;
            Some(body)
        } else {
            None
        };

        Ok(Stmt::TryCatch(TryCatchStmt { try_body, catch_binding, catch_body, finally_body, span }))
    }

    // ── Opcode statements ───────────────────────────────────────────────

    fn parse_cond_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::OpCond)?;
        let reg = self.parse_reg_ref()?;
        self.expect(&TokenKind::Comma)?;
        let val = self.parse_expr()?;
        self.expect(&TokenKind::Comma)?;
        self.expect(&TokenKind::LBrace)?;
        let then_body = self.parse_stmt_block()?;
        self.expect(&TokenKind::RBrace)?;
        let else_body = if self.eat(&TokenKind::Else) {
            self.expect(&TokenKind::LBrace)?;
            let b = self.parse_stmt_block()?;
            self.expect(&TokenKind::RBrace)?;
            Some(b)
        } else { None };
        self.eat(&TokenKind::Semicolon);
        let condition = Expr::BinOp(Box::new(Expr::Ident(reg)), BinOp::Eq, Box::new(val));
        Ok(Stmt::If(IfStmt { condition, then_body, else_body, span }))
    }

    fn parse_loop_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        self.expect(&TokenKind::OpLoop)?;
        let reg = self.parse_reg_ref()?;
        self.expect(&TokenKind::Comma)?;
        let _target = self.parse_expr()?;
        self.expect(&TokenKind::Comma)?;
        let _cond = self.parse_expr()?;
        self.expect(&TokenKind::Comma)?;
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_stmt_block()?;
        self.expect(&TokenKind::RBrace)?;
        self.eat(&TokenKind::Semicolon);
        let condition = Expr::Ident(reg);
        Ok(Stmt::While(WhileStmt { condition, body, span }))
    }

    fn parse_opcode_stmt(&mut self) -> PResult<Stmt> {
        let op = self.peek().clone();
        self.advance();

        let stmt = match op {
            TokenKind::OpCreate => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Colon)?;
                // Accept full type exprs (array<str>, map<k,v>, tuple<...>), not
                // just bare idents. Stringify to the canonical name for emit_type.
                let ty_expr = self.parse_type()?;
                let ty = type_expr_to_string(&ty_expr);
                OpcodeStmt::Create { reg, ty }
            }
            TokenKind::OpAssign => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Eq)?;
                let value = self.parse_expr()?;
                OpcodeStmt::Assign { reg, value }
            }
            TokenKind::OpAdd => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Comma)?;
                let value = self.parse_expr()?;
                OpcodeStmt::Add { reg, value }
            }
            TokenKind::OpSub => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Comma)?;
                let value = self.parse_expr()?;
                OpcodeStmt::Sub { reg, value }
            }
            TokenKind::OpMul => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Comma)?;
                let value = self.parse_expr()?;
                OpcodeStmt::Mul { reg, value }
            }
            TokenKind::OpDiv => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Comma)?;
                let value = self.parse_expr()?;
                OpcodeStmt::Div { reg, value }
            }
            TokenKind::OpSum      => OpcodeStmt::Sum { reg: self.expect_ident()? },
            TokenKind::OpPush     => OpcodeStmt::Push { reg: self.expect_ident()? },
            TokenKind::OpPop      => OpcodeStmt::Pop { reg: self.parse_reg_ref()? },
            TokenKind::OpQuery    => OpcodeStmt::Query { reg: self.parse_reg_ref()? },
            TokenKind::OpRemember => OpcodeStmt::Remember { reg: self.parse_reg_ref()? },
            TokenKind::OpStore => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Comma)?;
                let key = self.parse_expr()?;
                OpcodeStmt::Store { reg, key }
            }
            TokenKind::OpRecall => {
                // recall "key"  |  recall reg, "key"
                let mut reg = None;
                if let TokenKind::Ident(name) = self.peek().clone() {
                    let save = self.pos;
                    self.advance(); // consume the ident
                    if self.at(&TokenKind::Comma) {
                        self.advance(); // consume the comma → 2-arg form
                        reg = Some(name);
                    } else {
                        self.pos = save; // single-arg form; ident is the key expr
                    }
                }
                let key = self.parse_expr()?;
                OpcodeStmt::Recall { reg, key }
            }
            TokenKind::OpBind => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Comma)?;
                let role = self.expect_ident()?;
                self.expect(&TokenKind::Comma)?;
                let val = self.parse_expr()?;
                OpcodeStmt::Bind { reg, role, val }
            }
            TokenKind::OpUnify => {
                let a = self.expect_ident()?;
                self.expect(&TokenKind::Comma)?;
                let b = self.expect_ident()?;
                OpcodeStmt::Unify { a, b }
            }
            TokenKind::OpBindRole => {
                // bind_role reg, ROLE  (2-arg; filler defaults to the register itself)
                let reg = self.parse_reg_ref()?;
                self.expect(&TokenKind::Comma)?;
                let role = self.expect_ident()?;
                OpcodeStmt::BindRole { reg, role }
            }
            TokenKind::OpTransfer => {
                // transfer src, dst, amount
                let src = self.parse_reg_ref()?;
                self.expect(&TokenKind::Comma)?;
                let dst = self.parse_reg_ref()?;
                self.expect(&TokenKind::Comma)?;
                let amount = self.parse_expr()?;
                OpcodeStmt::Transfer { src, dst, amount }
            }
            TokenKind::OpCompare => {
                // compare a, b  (b may be a register or a value expression)
                let a = self.parse_reg_ref()?;
                self.expect(&TokenKind::Comma)?;
                let save = self.pos;
                if let Some(b) = self.try_parse_reg_ref() {
                    if self.at(&TokenKind::Semicolon) || self.at(&TokenKind::RBrace)
                        || self.at(&TokenKind::Eof) || self.at(&TokenKind::Comma) {
                        if self.at(&TokenKind::Comma) {
                            // 3+ args -> Extended
                            self.advance();
                            let mut args = vec![ExtArg::Reg(a), ExtArg::Reg(b)];
                            args.extend(self.parse_ext_args()?);
                            OpcodeStmt::Extended { op: ExtOp::Compare, args }
                        } else {
                            OpcodeStmt::Compare { a, b }
                        }
                    } else {
                        self.pos = save;
                        let v = self.parse_expr()?;
                        OpcodeStmt::Extended { op: ExtOp::Compare, args: vec![ExtArg::Reg(a), ExtArg::Val(v)] }
                    }
                } else {
                    self.pos = save;
                    let v = self.parse_expr()?;
                    OpcodeStmt::Extended { op: ExtOp::Compare, args: vec![ExtArg::Reg(a), ExtArg::Val(v)] }
                }
            }
            TokenKind::OpInfer => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Infer, args }
            }
            TokenKind::OpMapRoles => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::MapRoles, args }
            }
            TokenKind::OpFilter => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Filter, args }
            }
            TokenKind::OpScore => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Score, args }
            }
            TokenKind::OpDetectPattern => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::DetectPattern, args }
            }
            TokenKind::OpDecode => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Decode, args }
            }
            TokenKind::OpReduce => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Reduce, args }
            }
            TokenKind::OpMerge => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Merge, args }
            }
            TokenKind::OpSplit => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Split, args }
            }
            TokenKind::OpDebate => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Debate, args }
            }
            TokenKind::OpPredict => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Predict, args }
            }
            TokenKind::OpDiscover => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Discover, args }
            }
            TokenKind::OpDiff => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Diff, args }
            }
            TokenKind::OpSeq => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Seq, args }
            }
            TokenKind::OpSpecialize => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Specialize, args }
            }
            TokenKind::OpReward => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Reward, args }
            }
            TokenKind::OpTemporalBind => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::TemporalBind, args }
            }
            TokenKind::OpAnalogy => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Analogy, args }
            }
            TokenKind::OpGen => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Gen, args }
            }
            TokenKind::OpInst => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Inst, args }
            }
            TokenKind::OpBroadcast => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Broadcast, args }
            }
            TokenKind::OpExplore => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Explore, args }
            }
            TokenKind::OpForge => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Forge, args }
            }
            TokenKind::OpAsk => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Ask, args }
            }
            TokenKind::OpSync => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Sync, args }
            }
            TokenKind::OpForget => {
                let args = self.parse_ext_args()?;
                OpcodeStmt::Extended { op: ExtOp::Forget, args }
            }
            _ => return Err(self.err(format!("unknown opcode: {:?}", op))),
        };
        self.eat(&TokenKind::Semicolon);
        Ok(Stmt::Opcode(stmt))
    }

    /// Parse a register reference: a name (identifier or contextual keyword)
    /// optionally followed by positional/field access (`input.0`, `obj.field`).
    /// Returns the rendered dotted name. Used for opcode register operands so
    /// reserved words (event, response) and tuple access work as register names.
    fn parse_reg_ref(&mut self) -> PResult<String> {
        let mut name = self.expect_ident()?;
        loop {
            if self.at(&TokenKind::Dot) {
                self.advance();
                if let TokenKind::IntLit(n) = self.peek().clone() {
                    self.advance();
                    name = format!("{}.{}", name, n);
                } else {
                    let field = self.expect_ident()?;
                    name = format!("{}.{}", name, field);
                }
            } else {
                break;
            }
        }
        Ok(name)
    }

    /// Like parse_reg_ref but returns None instead of erroring (no rewind side
    /// effects beyond what the caller saves/restores).
    fn try_parse_reg_ref(&mut self) -> Option<String> {
        let save = self.pos;
        match self.parse_reg_ref() {
            Ok(n) => Some(n),
            Err(_) => { self.pos = save; None }
        }
    }

    /// Parse comma-separated arguments for an extended opcode statement.
    /// Each argument is a bare identifier (treated as a register/role name) or
    /// a value expression (string, int, array, field access, etc.). Stops at
    /// the statement terminator (`;`) or end of the enclosing block.
    fn parse_ext_args(&mut self) -> PResult<Vec<ExtArg>> {
        let mut args = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::Semicolon) || self.at(&TokenKind::RBrace)
                || self.at(&TokenKind::Eof) {
                break;
            }
            // A register reference (name, possibly with .field/.0 access) is a
            // Reg arg when it is immediately followed by an argument boundary;
            // anything else (operators, literals, arrays, calls) is a Val expr.
            let save = self.pos;
            let reg = self.try_parse_reg_ref();
            match reg {
                Some(name) if self.at(&TokenKind::Comma)
                    || self.at(&TokenKind::Semicolon)
                    || self.at(&TokenKind::RBrace)
                    || self.at(&TokenKind::Eof) => {
                    args.push(ExtArg::Reg(name));
                }
                _ => {
                    self.pos = save;
                    args.push(ExtArg::Val(self.parse_expr()?));
                }
            }
            if !self.eat(&TokenKind::Comma) { break; }
        }
        Ok(args)
    }

    // ── Expressions ─────────────────────────────────────────────────────

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> PResult<Expr> {
        let mut left = self.parse_and_expr()?;
        while self.eat(&TokenKind::PipePipe) {
            let right = self.parse_and_expr()?;
            left = Expr::BinOp(Box::new(left), BinOp::Or, Box::new(right));
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> PResult<Expr> {
        let mut left = self.parse_eq_expr()?;
        while self.eat(&TokenKind::AmpAmp) {
            let right = self.parse_eq_expr()?;
            left = Expr::BinOp(Box::new(left), BinOp::And, Box::new(right));
        }
        Ok(left)
    }

    fn parse_eq_expr(&mut self) -> PResult<Expr> {
        let mut left = self.parse_cmp_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::NotEq => BinOp::NotEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_cmp_expr()?;
            left = Expr::BinOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_cmp_expr(&mut self) -> PResult<Expr> {
        let mut left = self.parse_add_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::LAngle => BinOp::Lt,
                TokenKind::RAngle => BinOp::Gt,
                TokenKind::LtEq => BinOp::LtEq,
                TokenKind::GtEq => BinOp::GtEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_add_expr()?;
            left = Expr::BinOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_add_expr(&mut self) -> PResult<Expr> {
        let mut left = self.parse_mul_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul_expr()?;
            left = Expr::BinOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_mul_expr(&mut self) -> PResult<Expr> {
        let mut left = self.parse_unary_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary_expr()?;
            left = Expr::BinOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary_expr(&mut self) -> PResult<Expr> {
        match self.peek() {
            TokenKind::Minus => {
                self.advance();
                let e = self.parse_postfix_expr()?;
                Ok(Expr::UnaryOp(UnaryOp::Neg, Box::new(e)))
            }
            TokenKind::Exclamation => {
                self.advance();
                let e = self.parse_postfix_expr()?;
                Ok(Expr::UnaryOp(UnaryOp::Not, Box::new(e)))
            }
            TokenKind::Await => {
                self.advance();
                let e = self.parse_postfix_expr()?;
                Ok(Expr::Await(Box::new(e)))
            }
            _ => self.parse_postfix_expr(),
        }
    }

    fn parse_postfix_expr(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary_expr()?;
        loop {
            match self.peek() {
                TokenKind::Dot => {
                    self.advance();
                    // Tuple/positional field access: expr.0, expr.1 (the int lexes
                    // as IntLit after the dot). Otherwise a normal named field.
                    if let TokenKind::IntLit(n) = self.peek().clone() {
                        self.advance();
                        expr = Expr::Field(Box::new(expr), n.to_string());
                        continue;
                    }
                    let field = self.expect_ident()?;
                    // Check for method call: expr.method(args)
                    if self.at(&TokenKind::LParen) {
                        self.advance();
                        let args = self.parse_args()?;
                        self.expect(&TokenKind::RParen)?;
                        expr = Expr::MethodCall(Box::new(expr), field, args);
                    } else {
                        expr = Expr::Field(Box::new(expr), field);
                    }
                }
                TokenKind::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&TokenKind::RBracket)?;
                    expr = Expr::Index(Box::new(expr), Box::new(index));
                }
                TokenKind::LParen => {
                    self.advance();
                    let args = self.parse_args()?;
                    self.expect(&TokenKind::RParen)?;
                    expr = Expr::Call(Box::new(expr), args);
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_primary_expr(&mut self) -> PResult<Expr> {
        self.skip_newlines();
        match self.peek().clone() {
            TokenKind::IntLit(n) => { self.advance(); Ok(Expr::IntLit(n)) }
            TokenKind::FloatLit(n) => { self.advance(); Ok(Expr::FloatLit(n)) }
            TokenKind::StringLit(s) => { let s = s.clone(); self.advance(); Ok(Expr::StringLit(s)) }
            TokenKind::BoolLit(b) => { self.advance(); Ok(Expr::BoolLit(b)) }
            TokenKind::Null => { self.advance(); Ok(Expr::Null) }

            TokenKind::SelfKw => {
                self.advance();
                self.expect(&TokenKind::Dot)?;
                let field = self.expect_ident()?;
                Ok(Expr::SelfAccess(field))
            }

            TokenKind::Deploy => {
                self.advance();
                let name = self.expect_ident()?;
                self.expect(&TokenKind::LParen)?;
                let args = self.parse_args()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Expr::Deploy(name, args))
            }

            TokenKind::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                loop {
                    if self.at(&TokenKind::RBracket) { break; }
                    elems.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma) { break; }
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(Expr::ArrayLit(elems))
            }

            TokenKind::LBrace => {
                self.advance();
                let mut pairs = Vec::new();
                loop {
                    self.skip_newlines();
                    if self.at(&TokenKind::RBrace) { break; }
                    let key = self.parse_expr()?;
                    self.expect(&TokenKind::Colon)?;
                    let val = self.parse_expr()?;
                    self.eat(&TokenKind::Comma);
                    pairs.push((key, val));
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(Expr::MapLit(pairs))
            }

            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(inner)
            }

            TokenKind::Ident(_) => {
                let name = self.expect_ident()?;
                // Check for struct literal: Name { field: val }
                if self.at(&TokenKind::LBrace) && name.chars().next().map_or(false, |c| c.is_uppercase()) {
                    self.advance();
                    let mut fields = Vec::new();
                    loop {
                        self.skip_newlines();
                        if self.at(&TokenKind::RBrace) { break; }
                        let fname = self.expect_ident()?;
                        self.expect(&TokenKind::Colon)?;
                        let val = self.parse_expr()?;
                        self.eat(&TokenKind::Comma);
                        fields.push((fname, val));
                    }
                    self.expect(&TokenKind::RBrace)?;
                    Ok(Expr::StructLit(name, fields))
                } else {
                    Ok(Expr::Ident(name))
                }
            }

            // Log/debug/warn/error as function calls
            TokenKind::Log | TokenKind::Debug | TokenKind::Warn | TokenKind::Error => {
                let name = match self.peek() {
                    TokenKind::Log => "log",
                    TokenKind::Debug => "debug",
                    TokenKind::Warn => "warn",
                    TokenKind::Error => "error",
                    _ => unreachable!(),
                }.to_string();
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let args = self.parse_args()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Expr::Call(Box::new(Expr::Ident(name)), args))
            }

            // Opcode keywords (and other contextual keywords) used as a value
            // in expression position — treat as an identifier reference, then
            // allow normal postfix (.field / call / index) via the caller.
            tok if self.kind_is_ident_like(&tok) => {
                let name = self.expect_ident()?;
                Ok(Expr::Ident(name))
            }

            _ => Err(self.err(format!("unexpected token in expression: {:?}", self.peek()))),
        }
    }

    fn parse_args(&mut self) -> PResult<Vec<Expr>> {
        let mut args = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RParen) { break; }
            args.push(self.parse_expr()?);
            if !self.eat(&TokenKind::Comma) { break; }
        }
        Ok(args)
    }

    // ── Utility parsers ─────────────────────────────────────────────────

    fn parse_ident_list(&mut self) -> PResult<Vec<String>> {
        let mut names = vec![self.expect_ident()?];
        while self.eat(&TokenKind::Comma) {
            names.push(self.expect_ident()?);
        }
        Ok(names)
    }

    fn parse_config_fields(&mut self) -> PResult<Vec<ConfigField>> {
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let value = self.parse_expr()?;
            self.eat(&TokenKind::Semicolon);
            fields.push(ConfigField { name, value, span: self.span() });
        }
        Ok(fields)
    }

    fn parse_bindings(&mut self) -> PResult<Vec<(String, Expr, Span)>> {
        let mut bindings = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&TokenKind::RBrace) { break; }
            let span = self.span();
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let expr = self.parse_expr()?;
            self.eat(&TokenKind::Semicolon);
            bindings.push((name, expr, span));
        }
        Ok(bindings)
    }

    /// Skip a brace-delimited block without parsing its contents.
    fn skip_block(&mut self) -> PResult<()> {
        let mut depth = 1;
        loop {
            if self.pos >= self.tokens.len() {
                return Err(self.err("unexpected EOF in block".into()));
            }
            match &self.tokens[self.pos].kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos += 1;
                        return Ok(());
                    }
                }
                TokenKind::Eof => return Err(self.err("unexpected EOF in block".into())),
                _ => {}
            }
            self.pos += 1;
        }
    }
}

/// Render a TypeExpr to its canonical name string (for opcode type operands,
/// which are stored as String and hashed at compile time).
fn type_expr_to_string(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Void => "void".into(),
        TypeExpr::Array(inner) => format!("array<{}>", type_expr_to_string(inner)),
        TypeExpr::Set(inner) => format!("set<{}>", type_expr_to_string(inner)),
        TypeExpr::Promise(inner) => format!("promise<{}>", type_expr_to_string(inner)),
        TypeExpr::Channel(inner) => format!("channel<{}>", type_expr_to_string(inner)),
        TypeExpr::Nullable(inner) => format!("{}?", type_expr_to_string(inner)),
        TypeExpr::Map(k, v) => format!("map<{},{}>", type_expr_to_string(k), type_expr_to_string(v)),
        TypeExpr::Tuple(ts) => {
            let parts: Vec<String> = ts.iter().map(type_expr_to_string).collect();
            format!("tuple<{}>", parts.join(","))
        }
        TypeExpr::Union(ts) => {
            let parts: Vec<String> = ts.iter().map(type_expr_to_string).collect();
            parts.join("|")
        }
        TypeExpr::Fn(args, ret) => {
            let parts: Vec<String> = args.iter().map(type_expr_to_string).collect();
            format!("({})->{}", parts.join(","), type_expr_to_string(ret))
        }
    }
}

/// True for an ALL-CAPS-with-underscores bareword (`SUBJECT`, `VERB_PHRASE`)
/// — this codebase's unbroken convention for a VSA role symbol (every
/// `bind`/`bind_role` example), distinct from a register name (always
/// lower/mixed-case, e.g. `frame`) or a PascalCase type/program name (e.g.
/// `DecisionMin`). Requires at least one alphabetic character so a token
/// with none (digits/underscores only) falls back to `Named` rather than
/// matching vacuously. Used by `Parser::parse_asm_operand` to classify a
/// bareword `asm` operand as `Role` vs `Named`.
fn is_role_bareword(name: &str) -> bool {
    let mut has_alpha = false;
    for c in name.chars() {
        if c.is_ascii_alphabetic() {
            has_alpha = true;
            if !c.is_ascii_uppercase() {
                return false;
            }
        }
    }
    has_alpha
}

// ── Public parse function ───────────────────────────────────────────────────

pub fn parse(source: &str) -> PResult<SourceFile> {
    let tokens = crate::lexer::Lexer::new(source).tokenize()?;
    let mut parser = Parser::new(tokens);
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        let ast = parse("").unwrap();
        assert!(ast.items.is_empty());
    }

    #[test]
    fn test_parse_interface() {
        let ast = parse(r#"
            interface ISolver {
                type Input;
                type Output;
                abstract public function parse(raw: str): Input;
                abstract public function solve(input: Input): Output;
            }
        "#).unwrap();
        assert_eq!(ast.items.len(), 1);
        if let TopLevel::Interface(iface) = &ast.items[0] {
            assert_eq!(iface.name, "ISolver");
            assert_eq!(iface.members.len(), 4); // 2 types + 2 functions
        } else {
            panic!("expected interface");
        }
    }

    #[test]
    fn test_parse_struct() {
        let ast = parse(r#"
            struct ArithStep {
                label: str;
                lhs: f64;
                result: f64;
            }
        "#).unwrap();
        if let TopLevel::Struct(s) = &ast.items[0] {
            assert_eq!(s.name, "ArithStep");
            assert_eq!(s.fields.len(), 3);
            assert_eq!(s.fields[0].name, "label");
        } else {
            panic!("expected struct");
        }
    }

    #[test]
    fn test_parse_enum() {
        let ast = parse(r#"
            enum MathOp {
                Add = 0x03,
                Sub = 0x04,
                Mul = 0x05,
                Div = 0x06,
            }
        "#).unwrap();
        if let TopLevel::Enum(e) = &ast.items[0] {
            assert_eq!(e.name, "MathOp");
            assert_eq!(e.variants.len(), 4);
            assert_eq!(e.variants[0].name, "Add");
        } else {
            panic!("expected enum");
        }
    }

    #[test]
    fn test_parse_program_minimal() {
        let ast = parse(r#"
            program GSM8K implements ISolver {
                type Input = Problem;
                type Output = Solution;

                @system @once
                public function constructor() {
                    self.patterns = {};
                }

                @external
                public function solve(input: Input): Output {
                    create x : number;
                    assign x = 16;
                    sub x, 3;
                    sum x;
                    query x;
                    remember x;
                    return Output { result: 13 };
                }
            }
        "#).unwrap();
        if let TopLevel::Program(p) = &ast.items[0] {
            assert_eq!(p.name, "GSM8K");
            assert_eq!(p.implements, vec!["ISolver"]);
            // 2 type aliases + constructor + solve = 4 items
            assert!(p.body.len() >= 4);
        } else {
            panic!("expected program");
        }
    }

    #[test]
    fn test_parse_use_declaration() {
        // Task 3: `use <name>;` is a TOP-LEVEL decl, a sibling of `program`,
        // not something nested inside a program body.
        let ast = parse(r#"
            use demo;

            program UsesDemo {
                public function run(): void {}
            }
        "#).unwrap();
        assert_eq!(ast.items.len(), 2);
        match &ast.items[0] {
            TopLevel::Use(name) => assert_eq!(name, "demo"),
            other => panic!("expected TopLevel::Use, got {:?}", other),
        }
        assert!(matches!(&ast.items[1], TopLevel::Program(_)));
    }

    #[test]
    fn test_parse_import_declaration() {
        // Task 5: `import "path";` is a TOP-LEVEL decl, a sibling of
        // `program`/`use` — not the old in-body ES-module `Stmt::Import`
        // deleted in Task 1. This test only covers the parser's shape; the
        // loader that actually resolves/merges the path lives in
        // `crate::loader` and is exercised by `tests/file_import.rs`.
        let ast = parse(r#"
            import "lib.cube";

            program UsesLib {
                public function run(): void {}
            }
        "#).unwrap();
        assert_eq!(ast.items.len(), 2);
        match &ast.items[0] {
            TopLevel::Import(path) => assert_eq!(path, "lib.cube"),
            other => panic!("expected TopLevel::Import, got {:?}", other),
        }
        assert!(matches!(&ast.items[1], TopLevel::Program(_)));
    }

    #[test]
    fn test_parse_import_without_trailing_semicolon() {
        // Mirrors `parse_use`'s tolerance: the trailing `;` is eaten if
        // present, not required (`self.eat`, not `self.expect`).
        let ast = parse(r#"import "lib.cube""#).unwrap();
        assert_eq!(ast.items.len(), 1);
        assert!(matches!(&ast.items[0], TopLevel::Import(p) if p == "lib.cube"));
    }

    #[test]
    fn test_parse_function_override_marker() {
        // Task 3: a POSTFIX `override` after the signature (params / return
        // type), distinct from the pre-existing PREFIX `override` modifier.
        let ast = parse(r#"
            program UsesDemo {
                public function greet() override {
                    return 1;
                }
                public function plain(): void {}
            }
        "#).unwrap();
        if let TopLevel::Program(p) = &ast.items[0] {
            let greet = p.body.iter().find_map(|item| match item {
                ProgramItem::Function(f) if f.sig.name == "greet" => Some(f),
                _ => None,
            }).expect("greet");
            assert!(greet.sig.is_override, "postfix `override` must set is_override");

            let plain = p.body.iter().find_map(|item| match item {
                ProgramItem::Function(f) if f.sig.name == "plain" => Some(f),
                _ => None,
            }).expect("plain");
            assert!(!plain.sig.is_override, "a function without the marker must not be flagged");
        } else {
            panic!("expected program");
        }
    }

    #[test]
    fn test_parse_opcodes() {
        let ast = parse(r#"
            program Test implements ISolver {
                @external
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
                    store x, "key";
                    bind x, AGENT, "alice";
                    unify a, b;
                }
            }
        "#).unwrap();
        if let TopLevel::Program(p) = &ast.items[0] {
            if let ProgramItem::Function(f) = &p.body[0] {
                // Count opcode statements
                let opcode_count = f.body.iter().filter(|s| matches!(s, Stmt::Opcode(_))).count();
                assert_eq!(opcode_count, 14);
            }
        }
    }

    #[test]
    fn test_parse_atomic_try_assert() {
        let ast = parse(r#"
            program Test implements ISolver {
                public function run(): void {
                    atomic {
                        assign x = 12;
                        sub x, 4;
                        commit;
                    }
                    assert result == 8, "wrong";
                    try {
                        div x, 0;
                    } catch (e) {
                        rollback;
                    }
                }
            }
        "#).unwrap();
        if let TopLevel::Program(p) = &ast.items[0] {
            if let ProgramItem::Function(f) = &p.body[0] {
                assert!(f.body.iter().any(|s| matches!(s, Stmt::Atomic(_))));
                assert!(f.body.iter().any(|s| matches!(s, Stmt::Assert(_))));
                assert!(f.body.iter().any(|s| matches!(s, Stmt::TryCatch(_))));
            }
        }
    }

    #[test]
    fn test_parse_if_for_match() {
        let ast = parse(r#"
            program Test implements ISolver {
                public function run(): void {
                    if (x > 0) {
                        log("positive");
                    } else {
                        log("non-positive");
                    }
                    for (let item of items) {
                        add total, item;
                    }
                    match (op) {
                        "+" => add x, n;
                        "-" => sub x, n;
                    }
                }
            }
        "#).unwrap();
        if let TopLevel::Program(p) = &ast.items[0] {
            if let ProgramItem::Function(f) = &p.body[0] {
                assert!(f.body.iter().any(|s| matches!(s, Stmt::If(_))));
                assert!(f.body.iter().any(|s| matches!(s, Stmt::For(_))));
                assert!(f.body.iter().any(|s| matches!(s, Stmt::Match(_))));
            }
        }
    }

    #[test]
    fn test_parse_container() {
        let ast = parse(r#"
            container MathLab implements IOrchestrator {
                config {
                    name: "MathLab";
                    version: "0.1.0";
                }

                programs {
                    solver: deploy GSM8K();
                }

                @system @once
                public function constructor() {
                    log("MathLab started");
                }
            }
        "#).unwrap();
        if let TopLevel::Container(c) = &ast.items[0] {
            assert_eq!(c.name, "MathLab");
            assert_eq!(c.kind, ContainerKind::Container);
            assert_eq!(c.implements, vec!["IOrchestrator"]);
        } else {
            panic!("expected container");
        }
    }

    #[test]
    fn test_parse_world_extends() {
        let ast = parse(r#"
            world Classroom extends Container implements IDeployable {
                config {
                    name: "Classroom";
                }

                @system @once
                public function constructor() {
                    log("world started");
                }
            }
        "#).unwrap();
        if let TopLevel::Container(c) = &ast.items[0] {
            assert_eq!(c.kind, ContainerKind::World);
            assert_eq!(c.extends.as_deref(), Some("Container"));
        } else {
            panic!("expected world container");
        }
    }

    #[test]
    fn test_parse_storage_block() {
        let ast = parse(r#"
            program Test implements ISolver {
                storage {
                    patterns: mutable map<str, function>;
                    count: mutable u64 = 0;
                    config: immutable str;
                }

                public function run(): void {
                    log("test");
                }
            }
        "#).unwrap();
        if let TopLevel::Program(p) = &ast.items[0] {
            let has_storage = p.body.iter().any(|item| matches!(item, ProgramItem::Storage(_)));
            assert!(has_storage, "program should have a storage block");
        }
    }

    #[test]
    fn test_parse_emit() {
        let ast = parse(r#"
            program Test implements ISolver {
                public function run(): void {
                    emit PatternLearned {
                        program: "GSM8K",
                        pattern: "percentage",
                        total: 42,
                    };
                }
            }
        "#).unwrap();
        if let TopLevel::Program(p) = &ast.items[0] {
            if let ProgramItem::Function(f) = &p.body[0] {
                assert!(f.body.iter().any(|s| matches!(s, Stmt::Emit(_))));
            }
        }
    }

    #[test]
    fn test_parse_expressions() {
        let ast = parse(r#"
            program Test implements ISolver {
                public function run(): void {
                    let x: f64 = 3.14;
                    let y = x + 2 * 3;
                    let z = !true && false || x > 0;
                    let arr = [1, 2, 3];
                    let s = "hello";
                    self.count += 1;
                }
            }
        "#).unwrap();
        if let TopLevel::Program(p) = &ast.items[0] {
            if let ProgramItem::Function(f) = &p.body[0] {
                assert!(f.body.len() >= 5);
            }
        }
    }
}
