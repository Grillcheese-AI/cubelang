# CubeLang VM — Implementation Plan: RETURN, CALL, LOOP

Grounded in the current code (src/vm/engine.rs, src/compiler.rs) as of this writing.

## Verified starting facts (do not re-derive — confirmed in engine.rs)
- `exec_function` is a flat `while pc < bc.len()` loop; it returns
  `ExecResult::Ok(self.accumulator)` at the end.
- Registers, stack, storage, memory, accumulator are ALL VM-global. There are
  **no call frames** today. This is the central design constraint for CALL.
- `ExecResult::Return(Value)` already exists as a variant but is never produced.
- Labels resolve via `scan_labels` -> `label_map`; JMP/COND set `pc`, guarded by
  a 1,000,000 `jump_budget`. COMPARE sets `flag`, COND jumps when `!flag`.
- `COPY` (0x08, register->register value copy) already works.
- `resolve_value` / `resolve_i64` resolve a Named operand to its register value.
- Free opcode bytes: 0x17, 0x1F, 0x20, 0x29, 0x39, 0x3A+ (0x39 is right after
  NEWVAR=0x38).

## Opcode allocations (new bytes — must sync across 3 repos)
| name        | byte | needed for |
|-------------|------|-----------|
| RETURN      | 0x39 | early return |
| MAKE_ARRAY  | 0x3A | LOOP prerequisite |
| LEN         | 0x3B | LOOP prerequisite |
| INDEX       | 0x3C | LOOP prerequisite |

SYNC REQUIREMENT: every new byte must be added in lockstep to
(1) cubelang `src/compiler.rs` `mod op`, (2) opcode-vsa-rs `src/ir.rs`,
(3) the cubemind Python VM opcode table. Add a cross-repo sync test that fails
if the three tables disagree. CALL (0x13) and LOOP (0x12) already have bytes —
they need handlers, not new bytes.

## Build order (dependencies are real — follow this sequence)
1. **RETURN** — smallest; CALL depends on it to capture a callee's return value.
2. **CALL** — depends on RETURN; reuses POP for result delivery.
3. **Array values** (MAKE_ARRAY / LEN / INDEX) — prerequisite for a real LOOP.
4. **LOOP** (for-each) — lowered to while+index using the array opcodes.

GLOBAL INVARIANT: `cargo test` after each step, all tests stay green; add tests
per step; as each feature genuinely executes, REMOVE it from the P0-1 strict-mode
blocklist in CUBELANG_FIXES.md (do not unblock before it is tested).

---

## 1. RETURN (early exit) — SMALL (~0.5 day)

Problem: `compile_stmt(Stmt::Return)` emits SKIP; the VM only returns at the end
of the bytecode, so an early `return` does nothing and execution falls through.

Design — RETURN carries an optional value operand (0 or 1 operands):
- Compiler `compile_stmt(Stmt::Return(expr))`:
  - if `expr` present: emit `RETURN <operand>` where the operand is compiled via
    the existing `compile_expr_operand` (literal -> Imm/Float, register -> Named).
  - if bare `return;`: emit `RETURN` with 0 operands.
- VM new arm in `exec_function`:
  ```
  op::RETURN => {
      if let Some(o) = operands.get(0) {
          self.accumulator = self.resolve_value(Some(o));
      }
      return ExecResult::Return(self.accumulator.clone());
  }
  ```
- Caller plumbing: top-level `call()` must treat `Return(v)` the same as `Ok(v)`
  (unwrap the value). CALL (task 2) catches `Return(v)` to obtain the result.

Acceptance tests:
- `if (arg0 > 0) { return 1; } add side, 1; return 0;` with arg0=5 -> result 1
  AND register `side` was never incremented (proves fall-through stopped).
- bare `return;` mid-function halts and yields the current accumulator.
- 96 existing tests stay green.

---

## 2. CALL (intra-program call) — MEDIUM (~1.5-2 days)

Problem: CALL (0x13) is trace-only. The hard part is the flat global-register
model: a naive recursive call lets the callee clobber the caller's registers.

Decision: **v1 = recursive exec with register save/restore** (locals isolated,
storage/memory/accumulator stay shared = intended side-effect semantics).
Document **v2 = real call frames** as the eventual refactor (removes native-stack
coupling, enables stack traces) — defer it.

VM changes (engine.rs):
- Add VM context fields: `current_program: Option<String>` (set by `call()` so a
  CALL can find sibling functions) and `call_depth: u32` (guard, cap e.g. 256).
- New CALL arm:
  1. operand[0] = global(fn_name); operands[1..] = arg values (resolve each).
  2. look up callee in `current_program`; error if not found.
  3. guard: `call_depth >= MAX` -> ExecResult::Error("call depth exceeded").
  4. snapshot `self.registers` (clone) = caller locals; build fresh arg scope:
     keep snapshot, then insert `arg0..argN` with the resolved args.
  5. `call_depth += 1`; `let r = self.exec_function(&callee)`; `call_depth -= 1`.
  6. extract value: `Ok(v) | Return(v) => v`, `Error(e) => propagate`.
  7. restore caller registers from snapshot; `self.stack.push(v)` (result via
     stack — reuses POP).

Compiler changes (compiler.rs):
- Emit CALL with arg operands: `compile_expr_discard(Call(name, args))` ->
  `CALL global(name), <arg0>, <arg1>, ...`.
- Result binding: `let r = foo(a,b)` -> `CALL foo, a, b` then `POP r`.
- Bare-statement call -> `CALL foo, args` then `POP __discard` (drop result).
- Method calls `obj.m(args)`: v1 simplification — compile to `CALL m, obj, args`
  (object becomes arg0). Document this; full method dispatch is later.

Acceptance tests:
- `function dbl(): quantity { create r; add r, arg0; add r, arg0; sum r; return r; }`
  + `solve` does `let y = dbl(21); sum y;` -> 42.
- recursion: factorial(5) -> 120 (exercises RETURN + CALL + depth).
- caller-locals-intact: a local set before a call has its value after the call,
  even if the callee uses the same register name.
- depth guard returns a clean Error, not a native stack overflow.

---

## 3. Array values (MAKE_ARRAY / LEN / INDEX) — prerequisite for LOOP (~1.5 days)

Problem: `Value::Array` exists but cannot be constructed (array literals fall to
the opaque OP_NONE path in `compile_expr_operand`) or indexed. LOOP needs both.

New opcodes (pure data, no control flow):
- `MAKE_ARRAY dst, n, e0..e(n-1)`: collect n resolved values into `Value::Array`
  in register `dst`.
- `LEN dst, arr`: `dst = Int(array.len())` (0 for non-array/empty).
- `INDEX dst, arr, i`: `dst = array[i]` (Null if out of range).

Compiler: lower array literal `[a,b,c]` -> `MAKE_ARRAY tmp, 3, a, b, c`.

Acceptance tests:
- build `[10,20,30]`, `LEN n` -> 3, `INDEX x, arr, 1` -> 20, out-of-range -> Null.
- 96 tests green; new sync test passes (all 3 opcode tables agree).

---

## 4. LOOP (for-each) — via lowering (~1.5 days on top of task 3)

Decision: **lower `for x of arr` to an index-based `while`** — reuses the proven
COMPARE/COND/JMP machinery, no new VM control flow. (Native LOOP-frame iteration
was considered and rejected as higher-risk.)

Compiler `compile_for` emits:
```
CREATE __i ; ASSIGN __i = 0
LEN __n, arr
LABEL loop_top
COMPARE __i, __n, kind=2(lt) ; COND -> loop_end
INDEX <binding>, arr, __i
<body>
ADD __i, 1
JMP loop_top
LABEL loop_end
```
Use `fresh_label` for loop_top/loop_end (nested for-loops already get unique
labels). The 1M jump_budget already guards runaway loops. The LOOP opcode (0x12)
becomes an optional structural/no-op marker emitted for trace fidelity only.

Acceptance tests:
- `let xs=[10,20,30]; let total=0; for x of xs { add total, x; } sum total;` -> 60.
- empty array -> 0 iterations, total stays 0.
- nested for-loops over two arrays produce the correct product/sum.

---

## Risks & honest notes
- CALL v1 clones the whole register map per call — correct and fine at
  QC-program scale, NOT for hot inner loops. That's the reason v2 (real frames)
  exists on the roadmap; don't ship CALL inside a tight LOOP expecting speed.
- The bulk of LOOP effort is actually the array-value work (task 3), not the
  loop itself. Don't let "implement for-loops" hide that sub-project.
- Every new byte touches 3 repos. The sync test is not optional — a silent
  divergence between the Rust VM and the Python VM is exactly the class of bug
  that ships wrong behavior without failing locally.
- Sequencing vs CUBELANG_FIXES.md P0-1: unblock each construct in strict mode
  ONLY after its acceptance tests pass. Order: RETURN, then CALL, then for.
