# VM step 0 (2026-09-29): what the branch runner needs before it trusts a program

Agreed by seven or eight of the round-3 panel (CubbyLLM's
`docs/research/2026-09-28-panel-round3-synthesis.md`). Four changes, each pinned by
`tests/vm_step0.rs` and `tests/recover_scope.rs`.

## 1. An absent role is Null, structurally

A frame register records the ROLES bound into it as well as the fillers (`VsaFrame {roles, fillers}`).
`recover(frame, ROLE)` / `UNBIND` on a role that is not among the frame's roles returns Null with reason
`absent_role` **before any unbind runs**: no similarity is computed, so there is no number to threshold.
The tau thresholds now guard only crosstalk among roles that are present.

Before: the absent-role control rested on noise staying below tau -- a statistical guard doing a structural
job (WO-1.2 had narrowed the pool to the frame's own fillers; the wrong answer was still one of them at
~0.02). Measured now: absent-role probe **500/500 Null**, present-role recovery **500/500** right, the genuine
3-role recovery unchanged at 0.27001953125 (the number tau_vm(3) = 0.22021484375 is calibrated against).

The raw `UNBIND` opcode leaves `Value::Null` in the register on an absent role. A register with no recorded
frame keeps the old fallback (the global name table as the pool, a similarity the caller must read).

## 2. Attributes the VM does not honour are strict errors; the three it does are enforced

- `@hook(...)`, `@before(...)`, `@after(...)`, `@cron(...)`: strict compile errors (`attribute ... (parsed, not
  enforced by the VM)`). The VM has no event bus, middleware or scheduler; a program that compiled with them
  advertised behaviour it would never get. `--loose` still accepts them, as before. The 24 trace-only
  mnemonics were already strict errors; `run-proto` compiles strict.
- `@internal`: refused as the host's entry (`VM::call_entry`, which `run-proto` and `cubelang run` use for the
  target function). `@system`: refused as the host's entry too; the runtime's own constructor call (`VM::call`
  / `init`) is the runtime. An unmarked function is callable, as it always was (`Access::External`).
- `@once`: a second run of the function in the same VM is an error (`is @once and has already run`). A
  resume after ASK re-enters the same run and is not a second call.

`CompiledFunction` carries `access` and `once`; like `is_override` they are not in `.cubebin` yet.

## 3. Budgets on the request

```proto
message Budget {
  uint64 max_jumps = 1;    // per function entry (the loop guard); 0 = the VM default (1,000,000)
  uint64 max_ops = 2;      // opcodes for the whole run, constructor included; 0 = unlimited
  uint32 max_queries = 3;  // QUERYs for the whole run; 0 = unlimited
  uint32 max_wall_ms = 4;  // wall clock for the whole run; 0 = unlimited
}
```

`RunRequest.budget = 7`. `Budget.max_jumps` supersedes `Branch.max_jumps`. Every budget is checked before the
step that would cross it (the opcode, the store lookup, the jump), and exceeding one is an error naming it
(`op budget exhausted (500 opcodes)`, `query budget exhausted (2 queries)`, `wall budget exhausted`, `jump budget
exhausted`) -- never a truncated answer. The wall clock starts after the store is loaded and the branch applied,
and is checked every 64 opcodes. The report echoes the budget as applied (`BranchReport.budget`) and the wall
clock spent (`wall_ms`).

## 4. The report carries every recover, with its runner-up and a closed reason

```proto
enum RecoverReason { RECOVERED = 0; ABSENT_ROLE = 1; NO_FRAME = 2; EMPTY_POOL = 3; }

message Recover {
  string reg = 1;                // the frame register (its declared name where the name table knows it, else r_xxxx)
  string role = 2;
  string winner = 3;             // empty unless RECOVERED
  optional double similarity = 4;
  optional double runner_up = 5; // unset when the pool had one candidate
  optional double margin = 6;    // similarity - runner_up
  uint32 pool = 7;               // candidates considered
  RecoverReason reason = 8;
}
```

`BranchReport.recovers = 11`, one per UNBIND / `recover()` in order. Beside `RunResult.similarity`, the result
now carries `runner_up = 7` and `recover_reason = 8` for the last recover (the one-recover-per-function case).
The margin is what tells a clean recovery from a near-tie the threshold alone would let through; the host is
expected to refuse on a small margin whatever tau says. `cubelang run --json` prints the same fields.

## Not in step 0

Call frames (six of eight: not now). ASK inside branch programs (prohibited by the host for v1). The read set
tagged base / assumed / contingent and `assumption_used` (the branch runner's closure-aware overlays, next).
Array-size budget (no array growth path worth guarding yet).
