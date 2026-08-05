//! CubeLang — Unified AI Instruction Language.
//!
//! Compiles `.cube` source files to CubeMind VM bytecodes (0x00-0xFF).
//!
//! ```text
//! ┌──────────┐    ┌───────┐    ┌─────┐    ┌──────────┐
//! │ .cube    │───▶│ Lexer │───▶│ AST │───▶│ Bytecode │
//! │ source   │    │       │    │     │    │ 0x00..   │
//! └──────────┘    └───────┘    └─────┘    └──────────┘
//! ```

pub mod token;
pub mod lexer;
pub mod ast;
pub mod parser;
pub mod compiler;
pub mod validate;
pub mod vm;
pub mod loader;
