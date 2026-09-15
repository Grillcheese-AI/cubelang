//! CubeLang CLI — compile, inspect, and run .cube programs.
//!
//! Usage:
//!   cubelang compile <file.cube>              → file.cubebin (strict by
//!                                                default; --loose to opt out)
//!   cubelang compile <file.cube> -o out.cubebin
//!   cubelang run     <file.cube|file.cubebin> → execute (strict by default;
//!                                                --loose to opt out)
//!   cubelang inspect <file.cubebin>           → print program info
//!   cubelang disasm  <file.cubebin>           → disassemble bytecode
//!   cubelang check   <file.cube>              → full strict compile, NOT
//!                                                parse-only (--loose to opt out)
//!   cubelang version

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process;

use cubelang::compiler::{self, CompiledProgram, op};
use cubelang::loader;
use cubelang::reasoning::{RunRequest, RunResult, run_result};
use cubelang::vm::{VM, ExecResult, Value};
use prost::Message;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "compile"  | "c" => cmd_compile(&args[2..]),
        "inspect"  | "i" => cmd_inspect(&args[2..]),
        "disasm"   | "d" => cmd_disasm(&args[2..]),
        "check"    | "k" => cmd_check(&args[2..]),
        "validate" | "vd" => cmd_validate(&args[2..]),
        "run"      | "r" => cmd_run(&args[2..]),
        "run-proto"      => cmd_run_proto(),
        "version"  | "v" | "--version" | "-v" => cmd_version(),
        "help" | "--help" | "-h" => print_usage(),
        _ => {
            eprintln!("unknown command: {}", args[1]);
            print_usage();
            process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!(r#"CubeLang v0.1.0 — Unified AI Instruction Language

USAGE:
    cubelang <command> [options] <file>

COMMANDS:
    compile,  c    Compile .cube to .cubebin (strict by default; --loose to
                   accept non-executing constructs as silent no-ops instead
                   of a compile error)
    run,      r    Compile (or load .cubebin) and execute a program (strict
                   by default; --loose to opt out, same as compile)
    run-proto      Read a length-prefixed RunRequest from stdin, run it from
                   source, write a length-prefixed RunResult to stdout
                   (always strict -- no --loose over this transport)
    inspect,  i    Show .cubebin program info
    disasm,   d    Disassemble .cubebin to readable opcodes
    check,    k    Fully compile .cube in strict mode (NOT parse-only --
                   this is the same verification `compile`/`run` do by
                   default; pass --loose for a looser compile instead)
    validate, vd   Report executing vs trace-only opcodes per function
    version,  v    Print version

NOTE: `compile`/`run`/`check` reject every non-executing construct (trace-
only opcodes, `match`, `unify`, an unsupported assign/assert/expr-statement
shape, `throw`, ...) as a hard compile error by default -- this is what
"strict" used to mean and now just means; --strict is still accepted
everywhere as an explicit no-op alias for the default. Pass --loose to get
the old, permissive behavior back for THOSE constructs (silently compiling
them to a no-op instead of erroring). --loose does NOT bring back a
non-encodable expression operand (a `BinOp`/`UnaryOp`/... used where only a
literal, a bare identifier, or `self.field` can be encoded) -- that is
always a hard error, in every mode, because it is the one silent-garbage
path strict mode itself could never catch (see docs/DRIFT.md §C11).

EXAMPLES:
    cubelang compile examples/gsm8k.cube
    cubelang compile --loose examples/qc_decision.cube
    cubelang compile examples/gsm8k.cube -o build/gsm8k.cubebin
    cubelang inspect build/gsm8k.cubebin
    cubelang disasm build/gsm8k.cubebin
    cubelang check examples/gsm8k.cube
    cubelang run examples/fibonacci.cube --trace
"#);
}

fn cmd_version() {
    println!("cubelang 0.1.0");
}

// ── compile ─────────────────────────────────────────────────────────────────

fn cmd_compile(args: &[String]) {
    if args.is_empty() {
        eprintln!("error: missing input file");
        eprintln!("usage: cubelang compile [--loose] <file.cube> [-o output.cubebin]");
        process::exit(1);
    }

    // Parse flags and positional args. Strict verification (P0-1: rejects
    // non-executing constructs -- trace-only opcodes, `match`, `unify`, ...
    // NOT `for`, which always lowers to real loop scaffolding; see
    // `compiler::compile_strict`'s doc) is now the DEFAULT (see docs/DRIFT.md
    // "the language's accepted surface should equal its executing surface").
    // `--strict` is kept as an accepted, explicit no-op for anyone still
    // typing it out of habit or in an existing script; `--loose` is the new
    // opt-out.
    let mut strict = true;
    let mut positional: Vec<&String> = Vec::new();
    for arg in args {
        if arg == "--strict" { strict = true; }
        else if arg == "--loose" { strict = false; }
        else { positional.push(arg); }
    }

    if positional.is_empty() {
        eprintln!("error: missing input file");
        eprintln!("usage: cubelang compile [--loose] <file.cube> [-o output.cubebin]");
        process::exit(1);
    }

    let input = positional[0];
    let output = if positional.len() >= 3 && positional[1] == "-o" {
        PathBuf::from(positional[2])
    } else {
        Path::new(input).with_extension("cubebin")
    };

    // Load `input` and everything it (transitively) `import`s into one
    // merged AST (Task 5) -- replaces the old bare read_to_string+parse,
    // since a single source string can't carry more than one file.
    let merged = match loader::load(Path::new(input)) {
        Ok(ast) => ast,
        Err(e) => {
            eprintln!("error: {}", e);
            process::exit(1);
        }
    };

    // Compile (--strict surfaces non-executing constructs as hard errors).
    // Imported code goes through the exact same check as the entry file's
    // own -- `import` is modularity, never a verification bypass.
    let programs = if strict {
        match compiler::compile_ast_strict(&merged) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error (strict compile -- pass --loose to accept non-executing \
                           constructs instead): {}", e);
                process::exit(1);
            }
        }
    } else {
        match compiler::compile_ast(&merged) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: {}", e);
                process::exit(1);
            }
        }
    };

    if programs.is_empty() {
        eprintln!("warning: no programs found in {}", input);
        process::exit(0);
    }

    // Save each program
    for prog in &programs {
        let path = if programs.len() == 1 {
            output.clone()
        } else {
            output.with_file_name(format!("{}.cubebin", prog.name.to_lowercase()))
        };

        // Ensure output directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        match prog.save(&path) {
            Ok(()) => {
                let bin = prog.to_cubebin();
                let total_bc: usize = prog.functions.iter().map(|f| f.bytecode.len()).sum();
                println!("  compiled {} → {} ({} bytes, {} functions, {} bytes bytecode)",
                    input, path.display(), bin.len(), prog.functions.len(), total_bc);
            }
            Err(e) => {
                eprintln!("error: cannot write {}: {}", path.display(), e);
                process::exit(1);
            }
        }
    }
}

// ── inspect ─────────────────────────────────────────────────────────────────

fn cmd_inspect(args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: cubelang inspect <file.cubebin>");
        process::exit(1);
    }

    let prog = load_cubebin(&args[0]);

    println!("╔═══════════════════════════════════════════════╗");
    println!("║  .cubebin: {}",  args[0]);
    println!("╚═══════════════════════════════════════════════╝");
    println!();
    println!("  program:    {}", prog.name);
    println!("  implements: {}", if prog.implements.is_empty() { "(none)".into() } else { prog.implements.join(", ") });
    println!("  storage:    {} fields", prog.storage_fields.len());
    for f in &prog.storage_fields {
        println!("    - {}", f);
    }
    println!("  functions:  {}", prog.functions.len());
    let mut total_bc = 0;
    for f in &prog.functions {
        println!("    {}(): {} bytes", f.name, f.bytecode.len());
        total_bc += f.bytecode.len();
    }
    println!();
    println!("  total bytecode: {} bytes", total_bc);
    println!("  file size:      {} bytes", std::fs::metadata(&args[0]).map(|m| m.len()).unwrap_or(0));
}

// ── disasm ──────────────────────────────────────────────────────────────────

fn cmd_disasm(args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: cubelang disasm <file.cubebin>");
        process::exit(1);
    }

    let prog = load_cubebin(&args[0]);

    println!("# Disassembly: {} ({})", prog.name, args[0]);
    println!("# implements: {}", prog.implements.join(", "));
    println!();

    for func in &prog.functions {
        println!("{}():", func.name);
        disasm_bytecode(&func.bytecode);
        println!();
    }
}

fn disasm_bytecode(bc: &[u8]) {
    let mut pos = 0;
    while pos < bc.len() {
        let offset = pos;
        let opcode = bc[pos]; pos += 1;

        let mnemonic = opcode_name(opcode);

        if opcode == op::SKIP {
            println!("  0x{:04x}: {:02x}           {}", offset, opcode, mnemonic);
            continue;
        }

        if pos >= bc.len() {
            println!("  0x{:04x}: {:02x}           {}", offset, opcode, mnemonic);
            break;
        }

        let n_ops = bc[pos] as usize; pos += 1;
        let mut operands = Vec::new();

        for _ in 0..n_ops {
            if pos >= bc.len() { break; }
            let op_type = bc[pos]; pos += 1;
            match op_type {
                0x01 => { // Named
                    if pos + 1 < bc.len() {
                        operands.push(format!("named({:02x}{:02x})", bc[pos], bc[pos+1]));
                        pos += 2;
                    }
                }
                0x02 => { // Imm
                    if pos + 7 < bc.len() {
                        let val = i64::from_le_bytes([
                            bc[pos], bc[pos+1], bc[pos+2], bc[pos+3],
                            bc[pos+4], bc[pos+5], bc[pos+6], bc[pos+7],
                        ]);
                        operands.push(format!("{}", val));
                        pos += 8;
                    }
                }
                0x03 => { // Type
                    if pos + 1 < bc.len() {
                        operands.push(format!("type({:02x}{:02x})", bc[pos], bc[pos+1]));
                        pos += 2;
                    }
                }
                0x04 => { // Role
                    if pos + 1 < bc.len() {
                        operands.push(format!("role({:02x}{:02x})", bc[pos], bc[pos+1]));
                        pos += 2;
                    }
                }
                0x05 => { // Global
                    if pos + 1 < bc.len() {
                        operands.push(format!("global({:02x}{:02x})", bc[pos], bc[pos+1]));
                        pos += 2;
                    }
                }
                0x00 => { // None
                    if pos + 1 < bc.len() {
                        pos += 2;
                        operands.push("?".into());
                    }
                }
                _ => {
                    operands.push(format!("?0x{:02x}", op_type));
                }
            }
        }

        println!("  0x{:04x}: {:02x} {:>2}  {:<14} {}",
            offset, opcode, n_ops, mnemonic, operands.join(", "));
    }
}

fn opcode_name(code: u8) -> &'static str {
    match code {
        0x00 => "CREATE",     0x01 => "DESTROY",    0x02 => "ASSIGN",
        0x03 => "ADD",        0x04 => "SUB",        0x05 => "MUL",
        0x06 => "DIV",        0x07 => "TRANSFER",   0x08 => "COPY",
        0x09 => "PUSH",       0x0A => "POP",        0x0B => "COMPARE",
        0x0C => "QUERY",      0x0D => "STORE",      0x0E => "RECALL",
        0x0F => "BIND_ROLE",  0x10 => "UNBIND",     0x11 => "COND",
        0x12 => "LOOP",       0x13 => "CALL",       0x14 => "JMP",
        0x15 => "LABEL",      0x16 => "SEQ",        0x17 => "UNSEQ",
        0x21 => "REMEMBER",   0x34 => "SUM",        0x35 => "UNIFY",
        0x36 => "INST",       0x37 => "GEN",        0x38 => "NEWVAR",
        0x39 => "RETURN",
        0x3A => "MAKE_ARRAY", 0x3B => "LEN",       0x3C => "INDEX",
        0xFF => "SKIP",
        _ => "???",
    }
}

// ── check ───────────────────────────────────────────────────────────────────

fn cmd_check(args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: cubelang check [--loose] <file.cube>");
        process::exit(1);
    }

    // Fix 4 (docs/DRIFT.md §A4/B7): `check` was never parse-only -- it has
    // always fully compiled (see the comment on the compile call below) --
    // and this usage/help text no longer claims otherwise (see also
    // `print_usage`'s COMMANDS entry for `check`). Strict-by-default (this
    // is the same flip `compile`/`run` got): `--loose` opts `check` out of
    // strict verification into a plain compile; `--strict` is accepted as
    // an explicit no-op alias for the default, same as the other commands.
    let mut strict = true;
    let mut positional: Vec<&String> = Vec::new();
    for arg in args {
        if arg == "--strict" { strict = true; }
        else if arg == "--loose" { strict = false; }
        else { positional.push(arg); }
    }
    if positional.is_empty() {
        eprintln!("usage: cubelang check [--loose] <file.cube>");
        process::exit(1);
    }
    let input = positional[0];

    // Load + merge (Task 5: `import`) instead of a bare parse -- `check`
    // should see the whole compilation unit, imports included.
    let ast = match loader::load(Path::new(input)) {
        Ok(ast) => ast,
        Err(e) => {
            eprintln!("  {} error: {}", input, e);
            process::exit(1);
        }
    };

    // Task 8 (Feature B, verify-before-execute): `check` used to be
    // parse-only -- a program that parses but fails real compilation (a
    // missing `implements`, an unknown `asm` mnemonic, or a strict-mode
    // violation like a trace-only ext op) silently reported "ok". Actually
    // compile it -- in strict mode by default, so `check` is an honest
    // "will this run safely" preflight rather than just a syntax check, the
    // same strict verifier `compile`/`run` now use by default too; --loose
    // falls back to a plain (non-strict) compile for anyone who wants the
    // looser check.
    let compile_result = if strict {
        compiler::compile_ast_strict(&ast).map(|_| ())
    } else {
        compiler::compile_ast(&ast).map(|_| ()).map_err(|e| e.to_string())
    };
    if let Err(e) = compile_result {
        eprintln!("  {} error: {}", input, e);
        process::exit(1);
    }

    let n_items = ast.items.len();
    let mut n_programs = 0;
    let mut n_interfaces = 0;
    let mut n_structs = 0;
    let mut n_enums = 0;

    for item in &ast.items {
        match item {
            cubelang::ast::TopLevel::Interface(_) => n_interfaces += 1,
            cubelang::ast::TopLevel::Program(_) => n_programs += 1,
            cubelang::ast::TopLevel::Struct(_) => n_structs += 1,
            cubelang::ast::TopLevel::Enum(_) => n_enums += 1,
            _ => {}
        }
    }

    println!("  {} ok ({} items: {} programs, {} interfaces, {} structs, {} enums)",
        input, n_items, n_programs, n_interfaces, n_structs, n_enums);
}

// ── validate ────────────────────────────────────────────────────────────────

fn cmd_validate(args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: cubelang validate <file.cube>");
        process::exit(1);
    }

    let source = match std::fs::read_to_string(&args[0]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", args[0], e);
            process::exit(1);
        }
    };

    let report = match cubelang::validate::validate(&source) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {}", e);
            process::exit(1);
        }
    };

    print!("{}", report);
    if !report.is_clean() {
        eprintln!("\n{} trace-only / stub construct(s) found", report.finding_count());
        process::exit(1);
    }
}

// ── run ─────────────────────────────────────────────────────────────────────

fn cmd_run(args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: cubelang run <file.cube|file.cubebin> \
                   [--loose] [--fn name] [--trace] [--json] [--arg VALUE]... \
                   [--knowledge facts.jsonl]");
        process::exit(1);
    }

    let input = &args[0];
    let mut target_fn = "solve".to_string();
    let mut trace = false;
    let mut json_out = false;
    // Task 8 (Feature B, verify-before-execute) / strict-by-default: rejects
    // non-executing constructs (trace-only opcodes, `match`, ...; see
    // `compile_strict`'s doc for the exact list) for the `.cube`-from-source
    // path, same as `compile`. Strict is now the DEFAULT here too (see
    // docs/DRIFT.md: "the language's accepted surface should equal its
    // executing surface, so a model trained on it learns one consistent
    // thing"). `--strict` remains an accepted explicit no-op; `--loose`
    // opts out.
    let mut strict = true;
    let mut knowledge_path: Option<String> = None;
    let mut call_args: Vec<Value> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--fn" | "-f" => { if i + 1 < args.len() { target_fn = args[i + 1].clone(); i += 1; } }
            "--trace" | "-t" => trace = true,
            "--json" | "-j" => json_out = true,
            "--strict" => strict = true,
            "--loose" => strict = false,
            // Facts for QUERY. Without this the VM has no knowledge and
            // abstains on every query -- which is correct, but not useful.
            "--knowledge" | "-k" => {
                if i + 1 < args.len() { knowledge_path = Some(args[i + 1].clone()); i += 1; }
            }
            // Arguments bound to the function's DECLARED parameter names.
            "--arg" | "-a" => {
                if i + 1 < args.len() { call_args.push(Value::Str(args[i + 1].clone())); i += 1; }
            }
            other => { eprintln!("warning: ignoring unknown option {}", other); }
        }
        i += 1;
    }

    // Load: compile .cube in-memory, or load a .cubebin directly.
    let mut vm = VM::new();
    vm.trace_enabled = trace;

    if let Some(kp) = &knowledge_path {
        match std::fs::read_to_string(kp) {
            Ok(data) => match vm.knowledge.load_jsonl(&data) {
                Ok((facts, aliases)) => {
                    if !json_out {
                        println!("  knowledge: {} facts, {} aliases from {}",
                                 facts, aliases, kp);
                    }
                }
                // A malformed fact file must fail loudly. Silently grounding
                // against a half-loaded store is worse than not grounding.
                Err(e) => { eprintln!("error: {}", e); process::exit(1); }
            },
            Err(e) => { eprintln!("error: cannot read {}: {}", kp, e); process::exit(1); }
        }
    }

    let prog_name: String = if input.ends_with(".cubebin") {
        match vm.load_file(Path::new(input)) {
            Ok(name) => name,
            Err(e) => { eprintln!("error: cannot load {}: {}", input, e); process::exit(1); }
        }
    } else {
        let merged = match loader::load(Path::new(input)) {
            Ok(ast) => ast,
            Err(e) => { eprintln!("error: {}", e); process::exit(1); }
        };
        // Task 8 / strict-by-default: same strict/loose split as `cmd_compile`
        // above -- strict (the default) surfaces non-executing constructs as
        // hard errors instead of silently compiling them to no-ops; --loose
        // opts out.
        let programs = if strict {
            match compiler::compile_ast_strict(&merged) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error (strict compile -- pass --loose to accept non-executing \
                               constructs instead): {}", e);
                    process::exit(1);
                }
            }
        } else {
            match compiler::compile_ast(&merged) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: {}", e);
                    process::exit(1);
                }
            }
        };
        if programs.is_empty() {
            eprintln!("error: no programs in {}", input);
            process::exit(1);
        }
        load_programs(&mut vm, programs)
    };

    // Run the constructor first if the program defines one (sets up
    // storage), then the target function.
    let result = call_with_constructor(&mut vm, &prog_name, &target_fn, call_args);

    if trace && !json_out {
        println!("# trace ({} ops):", vm.trace.len());
        for line in &vm.trace { println!("  {}", line); }
        println!();
    }

    match result {
        ExecResult::Ok(val) | ExecResult::Return(val) => {
            if json_out {
                println!("{}", serde_json::json!({
                    "ok": true,
                    "program": prog_name,
                    "function": target_fn,
                    "result": value_to_json(&val),
                    "similarity": vm.last_recover_similarity,
                }));
            } else {
                println!("  {}.{}() -> {}", prog_name, target_fn, val);
            }
        }
        ExecResult::Suspend(susp) => {
            // The program asked. It is neither finished nor broken: it grounded
            // several candidates and only the caller can choose.
            //
            // THE REAL CALLER IS THE MODEL, NOT A HUMAN. The trunk renders the
            // question + candidates into language, the user replies in language,
            // and the trunk maps that reply back to one of the candidates before
            // calling `vm.resume`. `--json` is that boundary.
            if json_out {
                println!("{}", serde_json::json!({
                    "ok": true,
                    "suspended": true,
                    "program": susp.program,
                    "function": susp.function,
                    "question": value_to_json(&susp.question),
                    "candidates": susp.candidates.iter().map(value_to_json).collect::<Vec<_>>(),
                }));
            } else {
                // DEBUG HARNESS ONLY. A person never picks an index in the real
                // system; this exists so `cubelang run` can exercise resume from
                // a terminal. Do not mistake it for the interface.
                eprintln!("  [debug harness: the model, not a person, answers ASK]");
                println!("  ? {}", susp.question);
                for (i, c) in susp.candidates.iter().enumerate() {
                    // A QUERY chunk is a Map; showing "{3 entries}" defeats the
                    // point of a harness meant to exercise the contract by hand.
                    match c {
                        Value::Map(m) => {
                            let text = m.get("text").map(|v| v.to_string()).unwrap_or_default();
                            let src = m.get("source").map(|v| v.to_string()).unwrap_or_default();
                            println!("      [{}] {}", i, text);
                            if !src.is_empty() {
                                println!("          {}", src);
                            }
                        }
                        other => println!("      [{}] {}", i, other),
                    }
                }
                print!("  choose [0-{}]: ", susp.candidates.len().saturating_sub(1));
                use std::io::Write;
                let _ = std::io::stdout().flush();

                let mut line = String::new();
                let answer = match std::io::stdin().read_line(&mut line) {
                    Ok(_) => match line.trim().parse::<usize>() {
                        Ok(i) if i < susp.candidates.len() => susp.candidates[i].clone(),
                        _ => Value::Null,
                    },
                    Err(_) => Value::Null,
                };

                match vm.resume(susp, answer) {
                    ExecResult::Ok(val) | ExecResult::Return(val) => {
                        println!("  {}.{}() -> {}", prog_name, target_fn, val);
                    }
                    ExecResult::Suspend(s2) => {
                        println!("  (suspended again: {})", s2.question);
                    }
                    ExecResult::Error(e) => {
                        eprintln!("  error after resume: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }
        ExecResult::Error(e) => {
            if json_out {
                println!("{}", serde_json::json!({"ok": false, "error": e}));
            } else {
                eprintln!("  {}.{}() runtime error: {}", prog_name, target_fn, e);
            }
            process::exit(1);
        }
    }
}

// ── run-proto ───────────────────────────────────────────────────────────────
//
// `run --json`'s wire-transport sibling for the reasoning bridge (M2): same
// compile-from-source + call path, same value mapping, protobuf over
// stdin/stdout instead of a JSON line on a subprocess's stdout. Framing in
// both directions is a plain `u32` big-endian byte length followed by the
// encoded message. Rust's stdio is binary-safe (no CRLF translation), so
// reads/writes below move raw bytes with no text-mode handling.

/// Read one length-prefixed `RunRequest` from stdin, compile+run it, write
/// one length-prefixed `RunResult` to stdout. Never touches `cmd_run` or its
/// `--json` output -- this is a fully separate entry point that happens to
/// reuse the same program-loading/call helpers (`load_programs`,
/// `call_with_constructor`) `cmd_run` itself now uses.
/// `run-proto`: length-prefixed `RunRequest`s on stdin, length-prefixed
/// `RunResult`s on stdout, ONE PER REQUEST, UNTIL STDIN CLOSES.
///
/// Resident since 2026-09-11. Before, the process answered exactly one
/// request and exited, so a client that needed N answers spawned N
/// processes -- and on CubbyLLM's plan-disposer harvest the 181 spawns for
/// 181 membership questions cost more wall time than the 161 walks they
/// prevented (exp_m3 lookup_vp: 59.4 -> 65.7 ms/question). Now the client
/// keeps the pipe open and streams requests; each still runs on a FRESH
/// `VM::new()` (see `run_request_result`), so nothing about determinism or
/// verify-before-execute changes -- only the process boundary is amortized.
///
/// The one-shot shape still works unchanged: a client that writes one
/// request and closes stdin (`subprocess.run(input=...)`, the Rust tests'
/// `drop(stdin)`) gets one result, then the loop reads EOF and exits 0. A
/// TRUNCATED frame (EOF inside a length or body) is still an error: it
/// means the client died mid-write, not that it finished.
fn cmd_run_proto() {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    loop {
        let mut len_buf = [0u8; 4];
        match input.read_exact(&mut len_buf) {
            Ok(()) => {}
            // clean EOF at a frame boundary: the client is done
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                eprintln!("error: cannot read RunRequest length from stdin: {}", e);
                process::exit(1);
            }
        }
        let len = u32::from_be_bytes(len_buf) as usize;

        let mut req_buf = vec![0u8; len];
        if let Err(e) = input.read_exact(&mut req_buf) {
            eprintln!("error: cannot read RunRequest body from stdin: {}", e);
            process::exit(1);
        }

        let req = match RunRequest::decode(&req_buf[..]) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: cannot decode RunRequest: {}", e);
                process::exit(1);
            }
        };

        let result = run_request_result(req);

        let mut out_buf = Vec::new();
        // `Message::encode` into a `Vec<u8>` only fails on a writer error; a
        // `Vec` never fails to grow, so this can't actually happen in practice --
        // the `expect` documents that instead of threading a dead error path
        // through the rest of the function.
        result.encode(&mut out_buf).expect("encoding RunResult into a Vec cannot fail");

        let _ = out.write_all(&(out_buf.len() as u32).to_be_bytes());
        let _ = out.write_all(&out_buf);
        let _ = out.flush();
    }
}

/// The resident process's knowledge cache: path -> (mtime, parsed store).
/// A request naming a `knowledge_path` gets a CLONE of the parsed store on
/// its fresh VM; the file is re-read only when its mtime moves. Keyed by
/// the path string as sent, so two spellings of one file parse twice --
/// harmless, and simpler than canonicalizing on a platform where that can
/// fail.
thread_local! {
    static KNOWLEDGE_CACHE: std::cell::RefCell<
        std::collections::HashMap<String, (Option<std::time::SystemTime>, cubelang::vm::knowledge::KnowledgeStore)>
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Load `path` into `vm.knowledge` (through the cache). A missing or
/// malformed file is an ERROR, never an empty store: silently grounding
/// against nothing is the confabulation-shaped failure `knowledge.rs` exists
/// to prevent, and the client asked for this file by name.
fn load_knowledge_cached(vm: &mut VM, path: &str) -> Result<(), String> {
    let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
    let hit = KNOWLEDGE_CACHE.with(|c| {
        c.borrow().get(path).and_then(|(m, store)| if *m == mtime { Some(store.clone()) } else { None })
    });
    let store = match hit {
        Some(s) => s,
        None => {
            let data = std::fs::read_to_string(path)
                .map_err(|e| format!("knowledge_path {}: cannot read: {}", path, e))?;
            let mut fresh = cubelang::vm::knowledge::KnowledgeStore::new();
            fresh.load_jsonl(&data).map_err(|e| format!("knowledge_path {}: {}", path, e))?;
            KNOWLEDGE_CACHE.with(|c| { c.borrow_mut().insert(path.to_string(), (mtime, fresh.clone())); });
            fresh
        }
    };
    vm.knowledge = store;
    Ok(())
}

/// Compile `req.program` FROM SOURCE (`compiler::compile_strict`, the
/// loader-free sibling of the `loader::load` + `compiler::compile_ast_strict`
/// pair `cmd_run --strict` uses for a `.cube` file -- there is no filesystem
/// path here to resolve `import`s against, and a `use`-ing program can't
/// serialize to `.cubebin` anyway) and run `req.fn_name` with `req.args`
/// bound as `Value::Str`, mapping the outcome onto `RunResult` with the SAME
/// value shape `cmd_run`'s `--json` path uses (`value_to_json`) so the two
/// transports agree on meaning byte-for-byte. See `value_to_run_result` for
/// the mapping detail that matters most: `Value::Null` leaves the oneof
/// UNSET rather than becoming `Symbol("null")`.
///
/// Task 8 (Feature B, verify-before-execute): this used to call the loose
/// `compiler::compile`. A `use`-ing reasoning program can't serialize to
/// `.cubebin`, so `run-proto` was its ONLY from-source entry point that
/// never ran the strict verifier `compile --strict` already had for `.cube`
/// files compiled to disk -- every protobuf request now gets the same
/// verify-before-execute guarantee.
fn run_request_result(req: RunRequest) -> RunResult {
    let programs = match compiler::compile_strict(&req.program) {
        Ok(p) => p,
        Err(e) => return RunResult { ok: false, result: Some(run_result::Result::Error(e.to_string())), similarity: None },
    };
    if programs.is_empty() {
        return RunResult {
            ok: false,
            result: Some(run_result::Result::Error("no programs in request".to_string())),
            similarity: None,
        };
    }

    let mut vm = VM::new();
    if !req.knowledge_path.is_empty() {
        if let Err(e) = load_knowledge_cached(&mut vm, &req.knowledge_path) {
            return RunResult { ok: false, result: Some(run_result::Result::Error(e)), similarity: None };
        }
    }
    let prog_name = load_programs(&mut vm, programs);
    let call_args: Vec<Value> = req.args.into_iter().map(Value::Str).collect();

    let mut exec_result = call_with_constructor(&mut vm, &prog_name, &req.fn_name, call_args);

    // ASK/resume over the one-shot transport (2026-08-30): the request may
    // carry answers to the program's ASKs, in order. Each is a JSON-encoded
    // Value; `VM::resume` keeps its guarantee (the answer must be one of the
    // candidates that ASK offered) -- an invented answer is an error here
    // exactly as it is in-process. Out of answers with the program still
    // asking => report the suspension, never a fake Symbol.
    let mut answers = req.answers.into_iter();
    while let ExecResult::Suspend(susp) = exec_result {
        match answers.next() {
            None => {
                let suspended = cubelang::reasoning::Suspended {
                    question: value_to_json(&susp.question).to_string(),
                    candidates: susp.candidates.iter().map(|c| value_to_json(c).to_string()).collect(),
                    program: susp.program.clone(),
                    function: susp.function.clone(),
                };
                return RunResult { ok: true, result: Some(run_result::Result::Suspended(suspended)), similarity: None };
            }
            Some(raw) => {
                let parsed = match serde_json::from_str::<serde_json::Value>(&raw) {
                    Ok(j) => j,
                    Err(e) => return RunResult {
                        ok: false,
                        result: Some(run_result::Result::Error(format!("resume: answer is not JSON ({}): {}", raw, e))),
                        similarity: None,
                    },
                };
                let answer = match json_to_value(&parsed) {
                    Ok(v) => v,
                    Err(e) => return RunResult {
                        ok: false,
                        result: Some(run_result::Result::Error(format!("resume: bad answer {}: {}", raw, e))),
                        similarity: None,
                    },
                };
                exec_result = vm.resume(susp, answer);
            }
        }
    }
    // Task 7 (Feature A, similarity-surfacing): read after the call, since
    // it's VM state the call itself populates. "The last recover in this
    // run" is well-defined here -- this function builds a fresh `VM::new()`
    // per request (above) and the reasoning program does exactly one
    // `recover` per function.
    let similarity = vm.last_recover_similarity;

    match exec_result {
        ExecResult::Ok(val) | ExecResult::Return(val) => {
            RunResult { ok: true, result: value_to_run_result(&val), similarity }
        }
        // Task 8 (folded-in Minor from Task 7's review): both non-ok arms
        // below now hardcode `similarity: None` instead of threading through
        // `similarity`. A run that recovers cleanly and THEN suspends/errors
        // would otherwise carry a real-looking, stale similarity next to
        // `ok:false` -- misleading, since that number described an earlier
        // recover(), not the outcome being reported. The compile-error early
        // returns above already hardcode `similarity: None`; this makes
        // every non-ok `RunResult` agree, protecting whatever future
        // non-Python wire consumer reads this next.
        ExecResult::Suspend(_) => {
            // Unreachable: the answers loop above returns `Suspended` (or
            // resumes) for every Suspend before this match runs. Kept as an
            // honest arm rather than an `unreachable!()` so a future refactor
            // that bypasses the loop still reports rather than panics.
            RunResult {
                ok: false,
                result: Some(run_result::Result::Error("suspended after the answers loop (internal)".to_string())),
                similarity: None,
            }
        }
        ExecResult::Error(e) => RunResult { ok: false, result: Some(run_result::Result::Error(e)), similarity: None },
    }
}

/// The value mapping `run-proto` and `run --json` (`value_to_json`) must
/// agree on: `Value::Str` passes through verbatim as `Symbol`; every other
/// non-null variant reuses `value_to_json`'s own JSON shape, stringified, so
/// a bool/int/array/map result stays inspectable through one formatting
/// rule instead of two divergent ones. `Value::Null` is the load-bearing
/// case -- it leaves the oneof UNSET (`None`), mirroring
/// `value_to_json`'s `Value::Null -> serde_json::Value::Null` (how an
/// unbound `recover()` already renders as JSON `null`), so the Python side
/// decodes `None` rather than the string `"null"`.
fn value_to_run_result(val: &Value) -> Option<run_result::Result> {
    match val {
        Value::Null => None,
        Value::Str(s) => Some(run_result::Result::Symbol(s.clone())),
        other => Some(run_result::Result::Symbol(value_to_json(other).to_string())),
    }
}

/// Serialize a VM result Value to JSON for the `run --json` bridge output
/// (cubbyverse parses this structured result). `run-proto`'s
/// `value_to_run_result` reuses this same shape, stringified, for every
/// non-Str/non-Null value so the JSON and protobuf transports agree.
fn value_to_json(v: &cubelang::vm::Value) -> serde_json::Value {
    use cubelang::vm::Value;
    match v {
        Value::Int(n) => serde_json::json!(n),
        Value::Float(f) => serde_json::json!(f),
        Value::Str(s) => serde_json::json!(s),
        Value::Bool(b) => serde_json::json!(b),
        Value::Null => serde_json::Value::Null,
        Value::Array(a) => serde_json::Value::Array(a.iter().map(value_to_json).collect()),
        Value::Map(m) => serde_json::Value::Object(
            m.iter().map(|(k, val)| (k.clone(), value_to_json(val))).collect(),
        ),
        Value::Hvec(h) => serde_json::json!({ "hvec_dim": h.dim() }),
    }
}

/// The inverse of `value_to_json` for ASK answers arriving over the wire
/// (`RunRequest.answers`). Ints stay ints (a `1969` must equal the
/// `Value::Int(1969)` candidate under `resume`'s structural check), floats
/// stay floats, and objects become `Value::Map` so a QUERY chunk handed back
/// verbatim still matches. Hypervectors have no JSON form and cannot be a
/// selection anyway.
fn json_to_value(j: &serde_json::Value) -> Result<Value, String> {
    Ok(match j {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() { Value::Int(i) }
            else if let Some(f) = n.as_f64() { Value::Float(f) }
            else { return Err(format!("unrepresentable number {}", n)); }
        }
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Array(a) => Value::Array(a.iter().map(json_to_value).collect::<Result<Vec<_>, _>>()?),
        serde_json::Value::Object(m) => {
            if m.contains_key("hvec_dim") { return Err("a hypervector is not a selection".to_string()); }
            let mut out = std::collections::HashMap::new();
            for (k, v) in m { out.insert(k.clone(), json_to_value(v)?); }
            Value::Map(out)
        }
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Load every freshly-compiled program into `vm`, returning the first
/// program's name -- the "the first compiled program is the default one to
/// run" convention (see `loader` module doc) that `cmd_run`'s in-memory
/// `.cube`-compile branch and `run-proto` both rely on identically. Callers
/// check `programs.is_empty()` themselves first (the two call sites report
/// that case with different, path-specific messages).
fn load_programs(vm: &mut VM, programs: Vec<CompiledProgram>) -> String {
    let name = programs[0].name.clone();
    for p in programs { vm.load(p); }
    name
}

/// Run a program's `constructor` if it has one (sets up storage; best-effort,
/// like `cmd_run` always treated it), then call `function` -- the exact
/// two-call sequence `cmd_run` and `run-proto` both need, factored out so it
/// can't drift between the two transports.
fn call_with_constructor(vm: &mut VM, program: &str, function: &str, args: Vec<Value>) -> ExecResult {
    let _ = vm.call(program, "constructor", Vec::new());
    vm.call(program, function, args)
}

fn load_cubebin(path: &str) -> CompiledProgram {
    match CompiledProgram::load(Path::new(path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot load {}: {}", path, e);
            process::exit(1);
        }
    }
}
