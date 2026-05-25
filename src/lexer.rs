//! CubeLang lexer — tokenizes `.cube` source into a token stream.

use crate::token::{Token, TokenKind, Span};

pub struct Lexer<'a> {
    source: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { source: source.as_bytes(), pos: 0, line: 1, col: 1 }
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof { break; }
        }
        tokens
    }

    fn span(&self) -> Span {
        Span { line: self.line, col: self.col, offset: self.pos as u32 }
    }

    fn peek(&self) -> u8 {
        if self.pos < self.source.len() { self.source[self.pos] } else { 0 }
    }

    fn peek2(&self) -> u8 {
        if self.pos + 1 < self.source.len() { self.source[self.pos + 1] } else { 0 }
    }

    fn advance(&mut self) -> u8 {
        let ch = self.peek();
        if ch == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        self.pos += 1;
        ch
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.source.len() {
            match self.peek() {
                b' ' | b'\t' | b'\r' => { self.advance(); }
                _ => break,
            }
        }
    }

    fn skip_comment(&mut self) {
        // # single-line comment
        if self.peek() == b'#' {
            while self.pos < self.source.len() && self.peek() != b'\n' {
                self.advance();
            }
        }
    }

    fn next_token(&mut self) -> Token {
        loop {
            self.skip_whitespace();
            if self.peek() == b'#' {
                self.skip_comment();
                continue;
            }
            break;
        }

        let span = self.span();

        if self.pos >= self.source.len() {
            return Token { kind: TokenKind::Eof, span };
        }

        let ch = self.peek();

        // Newline
        if ch == b'\n' {
            self.advance();
            return Token { kind: TokenKind::Newline, span };
        }

        // String literal
        if ch == b'"' {
            return self.lex_string(span);
        }

        // Number literal
        if ch.is_ascii_digit() {
            return self.lex_number(span);
        }

        // @ annotations
        if ch == b'@' {
            return self.lex_annotation(span);
        }

        // Identifier or keyword
        if ch.is_ascii_alphabetic() || ch == b'_' {
            return self.lex_ident(span);
        }

        // Punctuation and operators
        self.lex_punct(span)
    }

    fn lex_string(&mut self, span: Span) -> Token {
        self.advance(); // skip opening "
        let mut s = String::new();
        loop {
            if self.pos >= self.source.len() { break; }
            let ch = self.advance();
            if ch == b'"' { break; }
            if ch == b'\\' {
                let esc = self.advance();
                match esc {
                    b'n' => s.push('\n'),
                    b't' => s.push('\t'),
                    b'\\' => s.push('\\'),
                    b'"' => s.push('"'),
                    b'$' => s.push('$'),
                    _ => { s.push('\\'); s.push(esc as char); }
                }
            } else {
                s.push(ch as char);
            }
        }
        Token { kind: TokenKind::StringLit(s), span }
    }

    fn lex_number(&mut self, span: Span) -> Token {
        let start = self.pos;

        // Hex literal: 0x...
        if self.peek() == b'0' && self.pos + 1 < self.source.len()
            && (self.source[self.pos + 1] == b'x' || self.source[self.pos + 1] == b'X')
        {
            self.advance(); // 0
            self.advance(); // x
            while self.pos < self.source.len() && (self.peek().is_ascii_hexdigit() || self.peek() == b'_') {
                self.advance();
            }
            let text: String = self.source[start + 2..self.pos].iter()
                .filter(|&&b| b != b'_')
                .map(|&b| b as char)
                .collect();
            let val = i64::from_str_radix(&text, 16).unwrap_or(0);
            return Token { kind: TokenKind::IntLit(val), span };
        }

        let mut is_float = false;
        while self.pos < self.source.len() && (self.peek().is_ascii_digit() || self.peek() == b'.' || self.peek() == b'_') {
            if self.peek() == b'.' {
                if is_float { break; }
                if self.peek2().is_ascii_digit() {
                    is_float = true;
                } else {
                    break;
                }
            }
            self.advance();
        }

        let text: String = self.source[start..self.pos].iter()
            .filter(|&&b| b != b'_')
            .map(|&b| b as char)
            .collect();

        if is_float {
            let val = text.parse::<f64>().unwrap_or(0.0);
            Token { kind: TokenKind::FloatLit(val), span }
        } else {
            let val = text.parse::<i64>().unwrap_or(0);
            Token { kind: TokenKind::IntLit(val), span }
        }
    }

    fn lex_annotation(&mut self, span: Span) -> Token {
        self.advance(); // skip @
        let start = self.pos;
        while self.pos < self.source.len() && (self.peek().is_ascii_alphanumeric() || self.peek() == b'_') {
            self.advance();
        }
        let word: String = self.source[start..self.pos].iter().map(|&b| b as char).collect();

        match word.as_str() {
            "external"   => Token { kind: TokenKind::AtExternal, span },
            "internal"   => Token { kind: TokenKind::AtInternal, span },
            "system"     => Token { kind: TokenKind::AtSystem, span },
            "hook"       => Token { kind: TokenKind::AtHook, span },
            "before"     => Token { kind: TokenKind::AtBefore, span },
            "after"      => Token { kind: TokenKind::AtAfter, span },
            "cron"       => Token { kind: TokenKind::AtCron, span },
            "once"       => Token { kind: TokenKind::AtOnce, span },
            "restricted" => Token { kind: TokenKind::AtRestricted, span },
            "ratelimit"  => Token { kind: TokenKind::AtRatelimit, span },
            _            => Token { kind: TokenKind::At, span }, // bare @
        }
    }

    fn lex_ident(&mut self, span: Span) -> Token {
        let start = self.pos;
        while self.pos < self.source.len() && (self.peek().is_ascii_alphanumeric() || self.peek() == b'_') {
            self.advance();
        }
        let word: String = self.source[start..self.pos].iter().map(|&b| b as char).collect();

        let kind = match word.as_str() {
            // Declarations
            "interface"    => TokenKind::Interface,
            "program"      => TokenKind::Program,
            "container"    => TokenKind::Container,
            "world"        => TokenKind::WorldKw,
            "modal"        => TokenKind::Modal,
            "robot"        => TokenKind::Robot,
            "extends"      => TokenKind::Extends,
            "implements"   => TokenKind::Implements,
            "struct"       => TokenKind::Struct,
            "enum"         => TokenKind::Enum,
            "type"         => TokenKind::Type,
            "event"        => TokenKind::Event,
            "extend"       => TokenKind::Extend,
            "deploy"       => TokenKind::Deploy,
            "proxy"        => TokenKind::Proxy,
            "grant"        => TokenKind::Grant,
            "revoke"       => TokenKind::Revoke,

            // Blocks
            "config"       => TokenKind::Config,
            "io"           => TokenKind::Io,
            "inputs"       => TokenKind::Inputs,
            "outputs"      => TokenKind::Outputs,
            "formats"      => TokenKind::Formats,
            "storage"      => TokenKind::Storage,
            "programs"     => TokenKind::Programs,
            "agents"       => TokenKind::Agents,
            "modalities"   => TokenKind::Modalities,
            "fusion"       => TokenKind::Fusion,
            "sensors"      => TokenKind::Sensors,
            "actuators"    => TokenKind::Actuators,
            "safety"       => TokenKind::Safety,
            "permissions"  => TokenKind::Permissions,

            // Functions
            "function"     => TokenKind::Function,
            "return"       => TokenKind::Return,
            "let"          => TokenKind::Let,
            "const"        => TokenKind::Const,
            "if"           => TokenKind::If,
            "else"         => TokenKind::Else,
            "for"          => TokenKind::For,
            "while"        => TokenKind::While,
            "match"        => TokenKind::Match,
            "in"           => TokenKind::In,
            "of"           => TokenKind::Of,
            "as"           => TokenKind::As,
            "null"         => TokenKind::Null,
            "self"         => TokenKind::SelfKw,
            "super"        => TokenKind::Super,
            "throw"        => TokenKind::Throw,
            "await"        => TokenKind::Await,
            "emit"         => TokenKind::Emit,
            "on"           => TokenKind::On,
            "from"         => TokenKind::From,
            "to"           => TokenKind::To,
            "override"     => TokenKind::Override,
            "constructor"  => TokenKind::Constructor,

            // Error handling
            "try"          => TokenKind::Try,
            "catch"        => TokenKind::Catch,
            "finally"      => TokenKind::Finally,
            "assert"       => TokenKind::Assert,

            // Transactions / ACID
            "atomic"       => TokenKind::Atomic,
            "rollback"     => TokenKind::Rollback,
            "commit"       => TokenKind::Commit,
            "gate"         => TokenKind::Gate,

            // Bytecode / low-level
            "bytecode"     => TokenKind::BytecodeKw,
            "import"       => TokenKind::Import,
            "export"       => TokenKind::Export,
            "exec"         => TokenKind::Exec,
            "codebook"     => TokenKind::Codebook,

            // Encryption / security
            "encrypt"      => TokenKind::Encrypt,
            "decrypt"      => TokenKind::Decrypt,
            "sealed"       => TokenKind::Sealed,
            "hash"         => TokenKind::CryptoHash,
            "sign"         => TokenKind::CryptoSign,
            "verify"       => TokenKind::CryptoVerify,

            // Debug / logging
            "log"          => TokenKind::Log,
            "debug"        => TokenKind::Debug,
            "warn"         => TokenKind::Warn,
            "error"        => TokenKind::Error,

            // Modifiers
            "public"       => TokenKind::Public,
            "private"      => TokenKind::Private,
            "abstract"     => TokenKind::Abstract,
            "global"       => TokenKind::Global,
            "mutable"      => TokenKind::Mutable,
            "immutable"    => TokenKind::Immutable,
            "async"        => TokenKind::Async,
            "sequential"   => TokenKind::Sequential,
            "parallel"     => TokenKind::Parallel,
            "joined"       => TokenKind::Joined,
            "pure"         => TokenKind::Pure,
            "optional"     => TokenKind::Optional,
            "singleton"    => TokenKind::Singleton,

            // Booleans
            "true"         => TokenKind::BoolLit(true),
            "false"        => TokenKind::BoolLit(false),

            // Opcodes
            "create"       => TokenKind::OpCreate,
            "assign"       => TokenKind::OpAssign,
            "add"          => TokenKind::OpAdd,
            "sub"          => TokenKind::OpSub,
            "mul"          => TokenKind::OpMul,
            "div"          => TokenKind::OpDiv,
            "sum"          => TokenKind::OpSum,
            "push"         => TokenKind::OpPush,
            "pop"          => TokenKind::OpPop,
            "query"        => TokenKind::OpQuery,
            "remember"     => TokenKind::OpRemember,
            "store"        => TokenKind::OpStore,
            "recall"       => TokenKind::OpRecall,
            "bind"         => TokenKind::OpBind,
            "unify"        => TokenKind::OpUnify,
            "bind_role"    => TokenKind::OpBindRole,
            "transfer"     => TokenKind::OpTransfer,
            "compare"      => TokenKind::OpCompare,

            // Built-in types
            "u8"               => TokenKind::TyU8,
            "u16"              => TokenKind::TyU16,
            "u32"              => TokenKind::TyU32,
            "u64"              => TokenKind::TyU64,
            "i8"               => TokenKind::TyI8,
            "i16"              => TokenKind::TyI16,
            "i32"              => TokenKind::TyI32,
            "i64"              => TokenKind::TyI64,
            "f32"              => TokenKind::TyF32,
            "f64"              => TokenKind::TyF64,
            "bool"             => TokenKind::TyBool,
            "str"              => TokenKind::TyStr,
            "byte"             => TokenKind::TyByte,
            "void"             => TokenKind::TyVoid,
            "vec"              => TokenKind::TyVec,
            "emb"              => TokenKind::TyEmb,
            "role"             => TokenKind::TyRole,
            "opcode"           => TokenKind::TyOpcode,
            "ctx"              => TokenKind::TyCtx,
            "rag"              => TokenKind::TyRag,
            "module"           => TokenKind::TyModule,
            "mdx"              => TokenKind::TyMdx,
            "agent"            => TokenKind::TyAgent,
            "cloned_agent"     => TokenKind::TyClonedAgent,
            "file"             => TokenKind::TyFile,
            "url"              => TokenKind::TyUrl,
            "dataset"          => TokenKind::TyDataset,
            "external_program" => TokenKind::TyExternalProgram,
            "request"          => TokenKind::TyRequest,
            "response"         => TokenKind::TyResponse,
            "array"            => TokenKind::TyArray,
            "map"              => TokenKind::TyMap,
            "set"              => TokenKind::TySet,
            "promise"          => TokenKind::TyPromise,
            "channel"          => TokenKind::TyChannel,
            "tuple"            => TokenKind::TyTuple,

            // Plain identifier
            _ => TokenKind::Ident(word),
        };

        Token { kind, span }
    }

    fn lex_punct(&mut self, span: Span) -> Token {
        let ch = self.advance();
        let kind = match ch {
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b';' => TokenKind::Semicolon,
            b':' => TokenKind::Colon,
            b',' => TokenKind::Comma,
            b'?' => TokenKind::Question,
            b'$' => TokenKind::Dollar,
            b'%' => TokenKind::Percent,
            b'.' => {
                if self.peek() == b'.' {
                    self.advance();
                    if self.peek() == b'.' {
                        self.advance();
                        TokenKind::Ellipsis
                    } else {
                        TokenKind::DotDot
                    }
                } else {
                    TokenKind::Dot
                }
            }
            b'-' => {
                if self.peek() == b'>' {
                    self.advance();
                    TokenKind::Arrow
                } else if self.peek() == b'=' {
                    self.advance();
                    TokenKind::MinusEq
                } else {
                    TokenKind::Minus
                }
            }
            b'=' => {
                if self.peek() == b'=' {
                    self.advance();
                    TokenKind::EqEq
                } else if self.peek() == b'>' {
                    self.advance();
                    TokenKind::FatArrow
                } else {
                    TokenKind::Eq
                }
            }
            b'!' => {
                if self.peek() == b'=' {
                    self.advance();
                    TokenKind::NotEq
                } else {
                    TokenKind::Exclamation
                }
            }
            b'<' => {
                if self.peek() == b'=' {
                    self.advance();
                    TokenKind::LtEq
                } else {
                    TokenKind::LAngle
                }
            }
            b'>' => {
                if self.peek() == b'=' {
                    self.advance();
                    TokenKind::GtEq
                } else {
                    TokenKind::RAngle
                }
            }
            b'+' => {
                if self.peek() == b'=' {
                    self.advance();
                    TokenKind::PlusEq
                } else {
                    TokenKind::Plus
                }
            }
            b'*' => {
                if self.peek() == b'=' {
                    self.advance();
                    TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }
            b'/' => {
                if self.peek() == b'=' {
                    self.advance();
                    TokenKind::SlashEq
                } else {
                    TokenKind::Slash
                }
            }
            b'&' => {
                if self.peek() == b'&' {
                    self.advance();
                    TokenKind::AmpAmp
                } else {
                    TokenKind::Ampersand
                }
            }
            b'|' => {
                if self.peek() == b'|' {
                    self.advance();
                    TokenKind::PipePipe
                } else {
                    TokenKind::Pipe
                }
            }
            _ => TokenKind::Ident(format!("<unknown:{}>", ch as char)),
        };

        Token { kind, span }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lex_empty() {
        let tokens = Lexer::new("").tokenize();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Eof);
    }

    #[test]
    fn test_lex_keywords() {
        let tokens = Lexer::new("interface program container function").tokenize();
        assert_eq!(tokens[0].kind, TokenKind::Interface);
        assert_eq!(tokens[1].kind, TokenKind::Program);
        assert_eq!(tokens[2].kind, TokenKind::Container);
        assert_eq!(tokens[3].kind, TokenKind::Function);
    }

    #[test]
    fn test_lex_modifiers() {
        let tokens = Lexer::new("public async mutable function").tokenize();
        assert!(tokens[0].kind.is_modifier());
        assert!(tokens[1].kind.is_modifier());
        assert!(tokens[2].kind.is_modifier());
        assert_eq!(tokens[3].kind, TokenKind::Function);
    }

    #[test]
    fn test_lex_permissions() {
        let tokens = Lexer::new("@external @internal @system @once").tokenize();
        assert!(tokens[0].kind.is_permission());
        assert!(tokens[1].kind.is_permission());
        assert!(tokens[2].kind.is_permission());
        assert!(tokens[3].kind.is_permission());
    }

    #[test]
    fn test_lex_opcodes() {
        let tokens = Lexer::new("create assign add sub mul div sum push pop query remember").tokenize();
        for tok in &tokens[..11] {
            assert!(tok.kind.is_opcode(), "expected opcode, got {:?}", tok.kind);
        }
    }

    #[test]
    fn test_lex_types() {
        let tokens = Lexer::new("u8 u64 f32 str vec emb ctx rag mdx agent").tokenize();
        assert_eq!(tokens[0].kind, TokenKind::TyU8);
        assert_eq!(tokens[1].kind, TokenKind::TyU64);
        assert_eq!(tokens[2].kind, TokenKind::TyF32);
        assert_eq!(tokens[3].kind, TokenKind::TyStr);
        assert_eq!(tokens[4].kind, TokenKind::TyVec);
        assert_eq!(tokens[5].kind, TokenKind::TyEmb);
        assert_eq!(tokens[6].kind, TokenKind::TyCtx);
        assert_eq!(tokens[7].kind, TokenKind::TyRag);
        assert_eq!(tokens[8].kind, TokenKind::TyMdx);
        assert_eq!(tokens[9].kind, TokenKind::TyAgent);
    }

    #[test]
    fn test_lex_literals() {
        let tokens = Lexer::new(r#"42 3.14 "hello" true false null"#).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::IntLit(42));
        assert_eq!(tokens[1].kind, TokenKind::FloatLit(3.14));
        assert_eq!(tokens[2].kind, TokenKind::StringLit("hello".into()));
        assert_eq!(tokens[3].kind, TokenKind::BoolLit(true));
        assert_eq!(tokens[4].kind, TokenKind::BoolLit(false));
        assert_eq!(tokens[5].kind, TokenKind::Null);
    }

    #[test]
    fn test_lex_operators() {
        let tokens = Lexer::new("= == != -> => <= >= += -=").tokenize();
        assert_eq!(tokens[0].kind, TokenKind::Eq);
        assert_eq!(tokens[1].kind, TokenKind::EqEq);
        assert_eq!(tokens[2].kind, TokenKind::NotEq);
        assert_eq!(tokens[3].kind, TokenKind::Arrow);
        assert_eq!(tokens[4].kind, TokenKind::FatArrow);
        assert_eq!(tokens[5].kind, TokenKind::LtEq);
        assert_eq!(tokens[6].kind, TokenKind::GtEq);
        assert_eq!(tokens[7].kind, TokenKind::PlusEq);
        assert_eq!(tokens[8].kind, TokenKind::MinusEq);
    }

    #[test]
    fn test_lex_comments_skipped() {
        let tokens = Lexer::new("let x # this is a comment\nlet y").tokenize();
        // Should get: let, x, newline, let, y, eof
        assert_eq!(tokens[0].kind, TokenKind::Let);
        assert_eq!(tokens[1].kind, TokenKind::Ident("x".into()));
    }

    #[test]
    fn test_lex_program_header() {
        let src = "program GSM8K implements ISolver, IDeployable {";
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::Program);
        assert_eq!(tokens[1].kind, TokenKind::Ident("GSM8K".into()));
        assert_eq!(tokens[2].kind, TokenKind::Implements);
        assert_eq!(tokens[3].kind, TokenKind::Ident("ISolver".into()));
        assert_eq!(tokens[4].kind, TokenKind::Comma);
        assert_eq!(tokens[5].kind, TokenKind::Ident("IDeployable".into()));
        assert_eq!(tokens[6].kind, TokenKind::LBrace);
    }

    #[test]
    fn test_lex_function_decl() {
        let src = "@external public async function solve(input: Input): Output {";
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::AtExternal);
        assert_eq!(tokens[1].kind, TokenKind::Public);
        assert_eq!(tokens[2].kind, TokenKind::Async);
        assert_eq!(tokens[3].kind, TokenKind::Function);
        assert_eq!(tokens[4].kind, TokenKind::Ident("solve".into()));
        assert_eq!(tokens[5].kind, TokenKind::LParen);
        assert_eq!(tokens[6].kind, TokenKind::Ident("input".into()));
        assert_eq!(tokens[7].kind, TokenKind::Colon);
        assert_eq!(tokens[8].kind, TokenKind::Ident("Input".into()));
        assert_eq!(tokens[9].kind, TokenKind::RParen);
        assert_eq!(tokens[10].kind, TokenKind::Colon);
        assert_eq!(tokens[11].kind, TokenKind::Ident("Output".into()));
        assert_eq!(tokens[12].kind, TokenKind::LBrace);
    }

    #[test]
    fn test_lex_container_declaration() {
        let src = "container MathLab implements IOrchestrator, IDeployable {";
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::Container);
        assert_eq!(tokens[1].kind, TokenKind::Ident("MathLab".into()));
        assert_eq!(tokens[2].kind, TokenKind::Implements);
    }

    #[test]
    fn test_lex_world_extends() {
        let src = "world MathClassroom extends Container implements IDeployable {";
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::WorldKw);
        assert_eq!(tokens[1].kind, TokenKind::Ident("MathClassroom".into()));
        assert_eq!(tokens[2].kind, TokenKind::Extends);
        assert_eq!(tokens[3].kind, TokenKind::Ident("Container".into())); // uppercase = ident, not keyword
        assert_eq!(tokens[4].kind, TokenKind::Implements);
    }

    #[test]
    fn test_lex_opcode_statement() {
        let src = "create x : quantity;\nassign x = 16;\nadd x, 3;";
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::OpCreate);
        assert_eq!(tokens[1].kind, TokenKind::Ident("x".into()));
        assert_eq!(tokens[2].kind, TokenKind::Colon);
        assert_eq!(tokens[3].kind, TokenKind::Ident("quantity".into()));
        assert_eq!(tokens[4].kind, TokenKind::Semicolon);
    }

    #[test]
    fn test_lex_generic_type() {
        let src = "map<str, f64>";
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::TyMap);
        assert_eq!(tokens[1].kind, TokenKind::LAngle);
        assert_eq!(tokens[2].kind, TokenKind::TyStr);
        assert_eq!(tokens[3].kind, TokenKind::Comma);
        assert_eq!(tokens[4].kind, TokenKind::TyF64);
        assert_eq!(tokens[5].kind, TokenKind::RAngle);
    }

    #[test]
    fn test_lex_error_handling() {
        let tokens = Lexer::new("try catch finally assert").tokenize();
        assert_eq!(tokens[0].kind, TokenKind::Try);
        assert_eq!(tokens[1].kind, TokenKind::Catch);
        assert_eq!(tokens[2].kind, TokenKind::Finally);
        assert_eq!(tokens[3].kind, TokenKind::Assert);
    }

    #[test]
    fn test_lex_acid_keywords() {
        let tokens = Lexer::new("atomic rollback commit gate").tokenize();
        assert_eq!(tokens[0].kind, TokenKind::Atomic);
        assert_eq!(tokens[1].kind, TokenKind::Rollback);
        assert_eq!(tokens[2].kind, TokenKind::Commit);
        assert_eq!(tokens[3].kind, TokenKind::Gate);
    }

    #[test]
    fn test_lex_bytecode_keywords() {
        let tokens = Lexer::new("bytecode import export exec codebook").tokenize();
        assert_eq!(tokens[0].kind, TokenKind::BytecodeKw);
        assert_eq!(tokens[1].kind, TokenKind::Import);
        assert_eq!(tokens[2].kind, TokenKind::Export);
        assert_eq!(tokens[3].kind, TokenKind::Exec);
        assert_eq!(tokens[4].kind, TokenKind::Codebook);
    }

    #[test]
    fn test_lex_atomic_block() {
        let src = r#"atomic {
            assign x = 12;
            sub x, 4;
            commit;
        }"#;
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::Atomic);
        assert_eq!(tokens[1].kind, TokenKind::LBrace);
    }

    #[test]
    fn test_lex_try_catch() {
        let src = r#"try { solve(input); } catch (e) { rollback; }"#;
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::Try);
        assert_eq!(tokens[1].kind, TokenKind::LBrace);
    }

    #[test]
    fn test_lex_bytecode_wasm_import() {
        let src = r#"bytecode import "solver.wasm";"#;
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::BytecodeKw);
        assert_eq!(tokens[1].kind, TokenKind::Import);
        assert_eq!(tokens[2].kind, TokenKind::StringLit("solver.wasm".into()));
        assert_eq!(tokens[3].kind, TokenKind::Semicolon);
    }

    #[test]
    fn test_lex_assert() {
        let src = r#"assert output.result == 18, "wrong answer";"#;
        let tokens = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].kind, TokenKind::Assert);
        assert_eq!(tokens[1].kind, TokenKind::Ident("output".into()));
        assert_eq!(tokens[2].kind, TokenKind::Dot);
        assert_eq!(tokens[3].kind, TokenKind::Ident("result".into()));
        assert_eq!(tokens[4].kind, TokenKind::EqEq);
        assert_eq!(tokens[5].kind, TokenKind::IntLit(18));
        assert_eq!(tokens[6].kind, TokenKind::Comma);
        assert_eq!(tokens[7].kind, TokenKind::StringLit("wrong answer".into()));
    }

    #[test]
    fn test_lex_full_program() {
        let src = r#"
            program GSM8K implements ISolver {
                storage {
                    patterns: mutable map<str, function>;
                }

                @external
                public function solve(input: Input): Output {
                    atomic {
                        create x : quantity;
                        assign x = 16;
                        sub x, 3;
                        sum x;
                        commit;
                    }
                    assert eval(x) == 13, "step failed";
                    return Output { result: eval(x) };
                }
            }
        "#;
        let tokens = Lexer::new(src).tokenize();
        // Should tokenize without errors — just check it doesn't panic
        // and has reasonable token count
        let non_newline: Vec<_> = tokens.iter()
            .filter(|t| t.kind != TokenKind::Newline && t.kind != TokenKind::Eof)
            .collect();
        assert!(non_newline.len() > 30, "expected 30+ tokens, got {}", non_newline.len());
        assert_eq!(non_newline[0].kind, TokenKind::Program);
    }
}
