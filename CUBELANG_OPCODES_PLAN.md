# The RETURN, CALL and LOOP plan: landed

This file held the August 2026 plan for four things: an early `return`, a call inside a program, array values,
and `for` over an array. All four have landed. What runs is in `docs/REFERENCE.md`: 3.1 (the opcode table), 3.3
(calls) and 8 (`for`).

What the plan left for later is still for later, and is said in `docs/REFERENCE.md` 3.3: real call frames, in
place of copying the registers at each call.

The plan's text was removed on 2026-11-11, so that nothing here describes finished work as still to do. It is in
the history: `git show f9a34e4:CUBELANG_OPCODES_PLAN.md`. Where other files cite this plan's sections, they mean
that version.
