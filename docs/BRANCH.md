# The branch on the wire (2026-09-28)

The counterfactual fork, at the transport rather than inside a program. Built for CubbyLLM's branch runner
(`docs/research/2026-09-28-panel-round3-synthesis.md` there): the host enumerates branches, each is one
`RunRequest` on a fresh VM, the VM reports what it did.

## RunRequest.branch

```proto
message Branch {
  string id = 1;                 // the host's name for the branch; echoed in the report
  repeated string exclude = 2;   // keys to take out of the store (with every alias carrying the same facts)
  repeated string assume = 3;    // facts to add, one knowledge .jsonl line each ({key,text,source} or {alias,of})
  uint64 max_jumps = 4;          // this run's jump budget; 0 = the default (1,000,000)
}
```

Applied to the request's own CLONE of the cached knowledge store (`load_knowledge_cached` hands every request a
clone; the branch is applied after, exclusions first, then assumptions, so an assumed fact under an excluded key
survives). The cached original is never touched: the next request without a branch sees the store as it was.
An assumed fact without a `source` is that branch's error, and the process keeps serving.

## RunResult.report

On every result, ok or not — a refused or erroring branch still cost what it cost:

```proto
message BranchReport {
  string branch_id = 1;
  uint64 ops = 2;                // opcodes executed
  uint64 jumps = 3;              // jumps taken
  repeated Queried queried = 4;  // every QUERY in order: {key, hits}; hits 0 = the store abstained
  uint32 excluded = 5;           // keys removed, aliases counted
  uint32 assumed = 6;            // facts added
  repeated string excluded_keys = 7; // the exclusions that matched, echoed back as applied
  uint64 max_jumps = 8;          // the budget this run had
}
```

`excluded_keys` is the audit: an exclusion the host asked for that matched nothing is visibly absent, and the
report cannot say a branch excluded a key it did not find.

## VM::snapshot / VM::restore

`VM::snapshot()` captures the program-visible state (registers, stack, storage, the associative memory, the
frames, the accumulator) and the knowledge store; `VM::restore(snap)` puts it back and keeps the logs and the
counters (`ops_executed`, `jumps_taken`, `queried`): a discarded branch still ran. These are the two methods the
SPEC's `ctx.snapshot / restore / fork / discard` will sit on when a gate needs within-program backtracking; until
then the host calls them, and no opcode is allocated (the free bytes are 0x3E and 0x3F; every new byte must be
added in lockstep to the three opcode tables, see `CUBELANG_OPCODES_PLAN.md`).

## What this is not

Not general fork semantics: it is isolated evaluation of bounded alternative inputs. Not a fix for counterfactual
coherence: excluding an event leaves its consequences in the store; the host marks the downstream closure
contingent and the report's `queried` list is how it knows a branch read one. Not a budget model: `max_jumps` was
the first of the per-request budgets; the rest (ops, queries, wall) arrived with VM step 0 (2026-09-29) as
`RunRequest.budget`, and the report grew `budget`, `wall_ms` and `recovers` -- see `docs/STEP0.md`.

Pinned by `tests/proto_branch.rs` (and `tests/vm_step0.rs` for the step-0 fields).
