---
name: add-example-cube-program
description: Workflow command scaffold for add-example-cube-program in cubelang.
allowed_tools: ["Bash", "Read", "Write", "Grep", "Glob"]
---

# /add-example-cube-program

Use this workflow when working on **add-example-cube-program** in `cubelang`.

## Goal

Creates a new example CubeLang program to demonstrate or test a language feature or reasoning pattern.

## Common Files

- `examples/*.cube`
- `examples/*.cubebin`
- `examples/*.jsonl`
- `examples/*.rs`

## Suggested Sequence

1. Understand the current state and failure mode before editing.
2. Make the smallest coherent change that satisfies the workflow goal.
3. Run the most relevant verification for touched files.
4. Summarize what changed and what still needs review.

## Typical Commit Signals

- Create a new .cube file in examples/ with a descriptive name.
- Optionally add a corresponding .cubebin or .jsonl file if needed.
- Optionally add or update a Rust runner (e.g., run_conversation_min.rs) for integration or regression testing.
- Document or describe the example in commit message or documentation.

## Notes

- Treat this as a scaffold, not a hard-coded script.
- Update the command if the workflow evolves materially.