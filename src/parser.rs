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
            TokenKind::Export       => "export".into(),
            TokenKind::Codebook     => "codebook".into(),
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
            TokenKind::Exec         => "exec".into(),
            TokenKind::Assert       => "assert".into(),
            TokenKind::Atomic       => "atomic".into(),
            TokenKind::Gate         => "gate".into(),
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
            TokenKind::BytecodeKw   => "bytecode".into(),
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
            _ => return Err(self.err(format!("expected identifier, got {:?}", self.tokens[self.pos].kind))),
        };
        self.pos += 1;
        Ok(name)
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
                _ => return Err(self.err(format!("unexpected token at top level: {:?}", self.peek()))),
            }
        }
        Ok(SourceFile { items })
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

        Ok(FunctionSig { permissions, modifiers, name, params, return_type, span })
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
            TokenKind::Rollback => { self.advance(); self.eat(&TokenKind::Semicolon); Ok(Stmt::Rollback) }
            TokenKind::Commit => { self.advance(); self.eat(&TokenKind::Semicolon); Ok(Stmt::Commit) }
            TokenKind::Assert => self.parse_assert_stmt(),
            TokenKind::Try => self.parse_try_catch_stmt(),

            // Opcode statements
            tok if TokenKind::OpCreate == tok => self.parse_opcode_stmt(),
            tok if matches!(tok, TokenKind::OpAssign | TokenKind::OpAdd | TokenKind::OpSub |
                TokenKind::OpMul | TokenKind::OpDiv | TokenKind::OpSum | TokenKind::OpPush |
                TokenKind::OpPop | TokenKind::OpQuery | TokenKind::OpRemember | TokenKind::OpStore |
                TokenKind::OpRecall | TokenKind::OpBind | TokenKind::OpUnify) => self.parse_opcode_stmt(),

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

    fn parse_opcode_stmt(&mut self) -> PResult<Stmt> {
        let op = self.peek().clone();
        self.advance();

        let stmt = match op {
            TokenKind::OpCreate => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Colon)?;
                let ty = self.expect_ident()?;
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
            TokenKind::OpPop      => OpcodeStmt::Pop { reg: self.expect_ident()? },
            TokenKind::OpQuery    => OpcodeStmt::Query { reg: self.expect_ident()? },
            TokenKind::OpRemember => OpcodeStmt::Remember { reg: self.expect_ident()? },
            TokenKind::OpStore => {
                let reg = self.expect_ident()?;
                self.expect(&TokenKind::Comma)?;
                let key = self.parse_expr()?;
                OpcodeStmt::Store { reg, key }
            }
            TokenKind::OpRecall => {
                let key = self.parse_expr()?;
                OpcodeStmt::Recall { key }
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
            _ => return Err(self.err(format!("unknown opcode: {:?}", op))),
        };
        self.eat(&TokenKind::Semicolon);
        Ok(Stmt::Opcode(stmt))
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

// ── Public parse function ───────────────────────────────────────────────────

pub fn parse(source: &str) -> PResult<SourceFile> {
    let tokens = crate::lexer::Lexer::new(source).tokenize();
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
                    create x : quantity;
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
    fn test_parse_opcodes() {
        let ast = parse(r#"
            program Test implements ISolver {
                @external
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
