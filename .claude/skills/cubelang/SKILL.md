```markdown
# cubelang Development Patterns

> Auto-generated skill from repository analysis

## Overview

This skill teaches you how to contribute to the `cubelang` codebase, a Rust-based language and virtual machine project. It covers coding conventions, file organization, and common workflows for extending the language, adding examples, co-designing compiler/VM features, and maintaining test assets. You'll learn how to safely add new opcodes, implement features across the compiler and VM, and keep examples and tests in sync.

## Coding Conventions

- **File Naming:**  
  Use `snake_case` for all Rust source files.  
  _Example:_  
  ```
  src/ast.rs
  src/compiler.rs
  src/vm/engine.rs
  ```

- **Import Style:**  
  Use relative imports within the crate.  
  _Example:_  
  ```rust
  use crate::parser::Parser;
  use super::token::TokenKind;
  ```

- **Export Style:**  
  Use named exports for modules and functions.  
  _Example:_  
  ```rust
  pub struct AstNode { /* ... */ }
  pub fn parse(input: &str) -> AstNode { /* ... */ }
  ```

- **Commit Messages:**  
  - Freeform style, often prefixed with area or feature (e.g., `vm:`, `parser:`, `feat:`, `examples:`).
  - Typical message:  
    ```
    feat: add support for new ASK/QUERY opcode in parser, compiler, and VM
    ```

## Workflows

### Add or Extend Opcodes in Parser and Compiler
**Trigger:** When introducing new language features or reasoning operations  
**Command:** `/add-opcode`

1. Edit `src/ast.rs` to define new AST node(s) or update existing ones.
2. Edit `src/lexer.rs` and `src/token.rs` to add new tokens or keywords.
3. Edit `src/parser.rs` to parse the new syntax or statements.
4. Edit `src/compiler.rs` to lower new AST nodes to bytecode/opcodes.
5. Optionally update `docs/SPEC.md` to document new opcodes.
6. Test new opcodes via examples or regression tests.

_Example: Adding a new `ASK` opcode_
```rust
// src/token.rs
pub enum TokenKind {
    // ...
    Ask,
    // ...
}

// src/lexer.rs
// Add logic to recognize "ask" keyword

// src/ast.rs
pub enum AstNode {
    // ...
    Ask { question: String },
    // ...
}

// src/parser.rs
// Parse "ask" statements into AstNode::Ask

// src/compiler.rs
// Emit bytecode for Ask node
```

### Add Example Cube Program
**Trigger:** When showcasing a new feature, kernel, or reasoning pattern  
**Command:** `/add-example`

1. Create a new `.cube` file in `examples/` with a descriptive name.
2. Optionally add a corresponding `.cubebin` or `.jsonl` file if needed.
3. Optionally add or update a Rust runner (e.g., `run_conversation_min.rs`) for integration or regression testing.
4. Document or describe the example in the commit message or documentation.

_Example:_
```
examples/ask_query.cube
examples/ask_query.cubebin
examples/run_conversation_min.rs
```

### VM and Compiler Feature Codesign
**Trigger:** When adding a new execution feature that requires changes across compiler and VM  
**Command:** `/feature-codesign`

1. Edit `src/compiler.rs` to emit new bytecode or handle the new feature.
2. Edit `src/vm/engine.rs` (and possibly `src/vm/mod.rs` or other VM files) to interpret the new bytecode.
3. Update or add relevant tests in `tests/` or examples to cover the new feature.
4. Optionally update documentation (`docs/` or commit message) to describe semantics.

_Example: Adding new control flow_
```rust
// src/compiler.rs
// Emit JumpIfFalse opcode

// src/vm/engine.rs
// Implement JumpIfFalse execution logic

// tests/control_flow.rs
#[test]
fn test_jump_if_false() {
    // ...
}
```

### Update or Sync Example and Test Assets
**Trigger:** When ensuring all examples and tests are up-to-date with the latest language or VM changes  
**Command:** `/sync-examples`

1. Edit or add multiple `.cube`, `.cubebin`, `.jsonl`, `.ps1`, or test files.
2. Update `.gitignore` or documentation if new files are added.
3. Optionally update source files if test/validation logic changes.
4. Commit with a message indicating asset or test update.

_Example:_
```
examples/new_feature.cube
tests/new_feature.rs
.gitignore
```

## Testing Patterns

- **Framework:** Unknown (Rust's built-in test framework is likely used)
- **Test Files:**  
  - Rust test files: `tests/*.rs`
  - Some TypeScript-style test files detected: `*.test.ts` (may be legacy or for auxiliary tooling)
- **Test Example:**
  ```rust
  // tests/vm.rs
  #[test]
  fn test_opcode_execution() {
      // Arrange: set up VM and bytecode
      // Act: execute
      // Assert: check results
  }
  ```

## Commands

| Command           | Purpose                                                      |
|-------------------|--------------------------------------------------------------|
| /add-opcode       | Add or extend opcodes in parser and compiler                 |
| /add-example      | Create a new example CubeLang program                        |
| /feature-codesign | Implement a new feature across compiler and VM               |
| /sync-examples    | Bulk update or sync example and test assets                  |
```
