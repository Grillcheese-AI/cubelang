---
name: add-or-extend-opcodes-in-parser-and-compiler
description: Workflow command scaffold for add-or-extend-opcodes-in-parser-and-compiler in cubelang.
allowed_tools: ["Bash", "Read", "Write", "Grep", "Glob"]
---

# /add-or-extend-opcodes-in-parser-and-compiler

Use this workflow when working on **add-or-extend-opcodes-in-parser-and-compiler** in `cubelang`.

## Goal

Adds new opcodes or extends existing ones in the language, updating parsing, compilation, and tokenization logic.

## Common Files

- `src/ast.rs`
- `src/compiler.rs`
- `src/lexer.rs`
- `src/parser.rs`
- `src/token.rs`

## Suggested Sequence

1. Understand the current state and failure mode before editing.
2. Make the smallest coherent change that satisfies the workflow goal.
3. Run the most relevant verification for touched files.
4. Summarize what changed and what still needs review.

## Typical Commit Signals

- Edit src/ast.rs to define new AST node(s) or update existing ones.
- Edit src/lexer.rs and src/token.rs to add new tokens or keywords.
- Edit src/parser.rs to parse new syntax or statements.
- Edit src/compiler.rs to lower new AST nodes to bytecode/opcodes.
- Optionally update docs/SPEC.md to document new opcodes.

## Notes

- Treat this as a scaffold, not a hard-coded script.
- Update the command if the workflow evolves materially.