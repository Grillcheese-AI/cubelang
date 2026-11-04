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

mod store_cli;
mod seal_cli;

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
        "run-proto"      => cmd_run_proto(&args[2..]),
        "tick"           => cmd_tick(&args[2..]),
        "keygen"         => cmd_keygen(&args[2..]),
        "keyring"        => cmd_keyring(&args[2..]),
        "sign"           => cmd_sign(&args[2..]),
        "knowledge"      => store_cli::cmd_knowledge(&args[2..]),
        "history"        => store_cli::cmd_history(&args[2..]),
        "tls-pin"        => cmd_tls_pin(&args[2..]),
        "seal"           => seal_cli::cmd_seal(&args[2..]),
        "open"           => seal_cli::cmd_open(&args[2..]),
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
                   (always strict -- no --loose over this transport).
                   --keyring ring.json: verify signed callers (ML-DSA-87 +
                   Ed25519) and refuse unsigned programs that are not
                   deployed; --pin-root FINGERPRINT: the roots to trust;
                   --contracts DIR: the deployed .cube contracts;
                   --history-key KEY.json: histories are sealed to it.
                   Over a network (TLS 1.3, SecP384r1MLKEM1024, AES-256-GCM,
                   both sides pinned): --listen ADDR --tls-key KEY.json
                   --client-pin PIN...; --remote ADDR --tls-key KEY.json
                   --server-pin PIN relays stdin/stdout to such a host
    tls-pin        tls-pin --key KEY.json: the pin of the identity's TLS key
    tick           Run the @cron functions of a program as they come due
                   (--every 1s, --ticks N; 0 = until stopped)
    keygen         Make an identity: keygen NAME --kind user|agent|model|
                   registrar --out key.json [--model-sha256 HEX]; the seed
                   file is sealed (Argon2id + AES-256-GCM) with the passphrase
                   in CUBELANG_KEY_PASSPHRASE or --passphrase-file; prints
                   the public entry (never the seed)
    keyring        check RING | add-root RING ENTRY.json | register RING
                   --by REGISTRAR.key.json [--days N] ENTRY.json | revoke
                   RING --by KEY.json NAME [--reason TEXT]
    sign           Sign one call for run-proto: sign --key key.json
                   --program FILE --fn NAME [--arg V]... [--answer J]...
                   [--knowledge PATH] [--history DIR] [--tick]; prints the
                   Caller as JSON
    knowledge      build FACTS.jsonl -o OUT.cubeseg (freeze facts into a
                   shared, memory-light segment) | info FILE | lookup FILE
                   KEY | fuzzy FILE KEY [--edits N] [--limit N] | keys FILE
                   PREFIX
    history        append DIR [--speaker S] TEXT | search DIR WORDS
                   [--prefix|--fuzzy] [--limit N] | recent DIR [N] | forget
                   DIR ID | merge DIR | gc DIR | info DIR | seal DIR (a
                   sealed history takes --key KEY.json on every command)
    seal           Seal a file to one identity: seal --to NAME --keyring
                   RING.json (or --to ENTRY.json) [--from KEY.json] [--aad T]
                   IN OUT (ML-KEM-1024 + X25519, AES-256-GCM; --from signs)
    open           Open a sealed file: open --key KEY.json [--keyring
                   RING.json] [--signer NAME] [--aad T] IN OUT|-
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
                0x07 => { // Field: base hash, path length, then (len, utf-8 key) per key
                    if pos + 2 < bc.len() {
                        let base = format!("{:02x}{:02x}", bc[pos], bc[pos + 1]);
                        let n = bc[pos + 2] as usize;
                        pos += 3;
                        let mut keys = Vec::with_capacity(n);
                        for _ in 0..n {
                            let Some(&len) = bc.get(pos) else { break };
                            let end = (pos + 1 + len as usize).min(bc.len());
                            keys.push(String::from_utf8_lossy(&bc[pos + 1..end]).into_owned());
                            pos = end;
                        }
                        operands.push(format!("field(named({}).{})", base, keys.join(".")));
                    }
                }
                0x08 => operands.push("null".into()),
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
    let mut history_path: Option<String> = None;
    let mut history_key: Option<String> = None;
    let mut call_args: Vec<Value> = Vec::new();
    let mut caller: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--fn" | "-f" => { if i + 1 < args.len() { target_fn = args[i + 1].clone(); i += 1; } }
            // The caller `@restricted` checks, asserted by the host itself: the local CLI IS the
            // host (it could edit the source too), so no signature. Over run-proto a caller is
            // honoured only with signatures.
            "--as" => { if i + 1 < args.len() { caller = Some(args[i + 1].clone()); i += 1; } }
            "--trace" | "-t" => trace = true,
            "--json" | "-j" => json_out = true,
            "--strict" => strict = true,
            "--loose" => strict = false,
            // Facts for QUERY. Without this the VM has no knowledge and
            // abstains on every query -- which is correct, but not useful.
            "--knowledge" | "-k" => {
                if i + 1 < args.len() { knowledge_path = Some(args[i + 1].clone()); i += 1; }
            }
            // The conversation so far, for `use history;` (2026-10-24).
            "--history" => {
                if i + 1 < args.len() { history_path = Some(args[i + 1].clone()); i += 1; }
            }
            // The key a sealed history opens with (`history seal`); its passphrase as for keygen.
            "--history-key" => {
                if i + 1 < args.len() { history_key = Some(args[i + 1].clone()); i += 1; }
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
    vm.caller = caller;

    let history_vault = history_key.as_deref()
        .map(|k| cubelang::seal::SealedVault::own(open_identity(k, None)));
    if let Some(hp) = &history_path {
        let vault = history_vault.as_ref().map(|v| v as &dyn cubelang::vm::segment::Vault);
        match cubelang::vm::history::History::open_dir_in(Path::new(hp), vault) {
            Ok(h) => {
                if !json_out {
                    println!("  history: {} turns from {}", h.len(), hp);
                }
                vm.history = h;
            }
            Err(e) => { eprintln!("error: {}", e); process::exit(1); }
        }
    }
    if let Some(kp) = knowledge_path.as_ref().filter(|k| k.ends_with(".cubeseg") || Path::new(k.as_str()).is_dir()) {
        match store_cli::open_knowledge(kp) {
            Ok(store) => {
                if !json_out {
                    println!("  knowledge: {} keys from {} (a frozen segment)", store.len(), kp);
                }
                vm.knowledge = store;
            }
            Err(e) => { eprintln!("error: knowledge {}", e); process::exit(1); }
        }
    } else if let Some(kp) = &knowledge_path {
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

    let prog_name = load_input(&mut vm, input, strict);

    // Run the constructor first if the program defines one (sets up
    // storage), then the target function.
    let result = call_with_constructor(&mut vm, &prog_name, &target_fn, call_args);
    report_run(&mut vm, &prog_name, &target_fn, result, trace, json_out);
}

/// Compile a `.cube` in memory (strict unless `--loose`) or load a `.cubebin`, into `vm`; the first
/// program's name. Exits on any error (the CLI commands that share it have nothing to do without it).
fn load_input(vm: &mut VM, input: &str, strict: bool) -> String {
    if input.ends_with(".cubebin") {
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
        load_programs(vm, programs)
    }
}

/// Print one `run`'s outcome (and drive the debug ASK harness), exiting non-zero on an error.
fn report_run(vm: &mut VM, prog_name: &str, target_fn: &str, result: ExecResult, trace: bool, json_out: bool) {
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
                    "runner_up": vm.last_recover_runner_up,
                    "recover_reason": vm.last_recover_reason.map(|r| r.as_str()),
                    "recovers": vm.recovers.iter().map(|r| serde_json::json!({
                        "reg": r.reg, "role": r.role, "winner": r.winner,
                        "similarity": r.similarity, "runner_up": r.runner_up,
                        "margin": r.margin, "pool": r.pool, "reason": r.reason.as_str(),
                    })).collect::<Vec<_>>(),
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
fn cmd_run_proto(args: &[String]) {
    let mut keyring: Option<String> = None;
    let mut contracts: Option<String> = None;
    let mut pins: Vec<String> = Vec::new();
    let mut history_key: Option<String> = None;
    let mut pass_file: Option<String> = None;
    let (mut listen, mut remote, mut tls_key, mut server_pin): (Option<String>, Option<String>, Option<String>, Option<String>) =
        (None, None, None, None);
    let mut client_pins: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--keyring" if i + 1 < args.len() => { keyring = Some(args[i + 1].clone()); i += 1; }
            "--contracts" if i + 1 < args.len() => { contracts = Some(args[i + 1].clone()); i += 1; }
            "--pin-root" if i + 1 < args.len() => { pins.push(args[i + 1].trim().to_ascii_lowercase()); i += 1; }
            "--history-key" if i + 1 < args.len() => { history_key = Some(args[i + 1].clone()); i += 1; }
            "--passphrase-file" if i + 1 < args.len() => { pass_file = Some(args[i + 1].clone()); i += 1; }
            "--listen" if i + 1 < args.len() => { listen = Some(args[i + 1].clone()); i += 1; }
            "--remote" if i + 1 < args.len() => { remote = Some(args[i + 1].clone()); i += 1; }
            "--tls-key" if i + 1 < args.len() => { tls_key = Some(args[i + 1].clone()); i += 1; }
            "--client-pin" if i + 1 < args.len() => { client_pins.push(args[i + 1].clone()); i += 1; }
            "--server-pin" if i + 1 < args.len() => { server_pin = Some(args[i + 1].clone()); i += 1; }
            other => { eprintln!("error: run-proto: unknown option {}", other); process::exit(1); }
        }
        i += 1;
    }
    fn die(msg: String) -> ! {
        eprintln!("error: run-proto: {}", msg);
        process::exit(1)
    }
    fn pin_of(hex: &str) -> [u8; 48] {
        cubelang::tls::parse_pin(hex).unwrap_or_else(|e| die(e))
    }
    // --remote: this process is a relay to a remote run-proto; the checks are the remote host's.
    if let Some(addr) = &remote {
        if keyring.is_some() || contracts.is_some() || history_key.is_some() || listen.is_some() || !client_pins.is_empty() {
            die("--remote relays to a host that has its own options; pass only --tls-key and --server-pin".into());
        }
        let (Some(k), Some(p)) = (&tls_key, &server_pin) else {
            die("--remote needs --tls-key KEY.json (this side's identity) and --server-pin PIN (the host's)".into())
        };
        let me = open_identity(k, pass_file.as_deref());
        let cfg = cubelang::tls::client_config(&me, pin_of(p)).unwrap_or_else(|e| die(e));
        relay_tls(addr, cfg);
        return;
    }
    if server_pin.is_some() {
        die("--server-pin goes with --remote".into());
    }
    let tls_server = match (&listen, &tls_key) {
        (Some(_), Some(k)) => {
            let me = open_identity(k, pass_file.as_deref());
            let pins: Vec<[u8; 48]> = client_pins.iter().map(|p| pin_of(p)).collect();
            let my_pin = cubelang::tls::pin(&me).unwrap_or_else(|e| die(e));
            Some((cubelang::tls::server_config(&me, &pins).unwrap_or_else(|e| die(e)), my_pin))
        }
        (Some(_), None) => die("--listen needs --tls-key KEY.json: a network leg is TLS 1.3 or nothing".into()),
        (None, Some(_)) => die("--tls-key goes with --listen or --remote".into()),
        (None, None) if !client_pins.is_empty() => die("--client-pin goes with --listen".into()),
        (None, None) => None,
    };
    // --history-key: every history this process opens is sealed to this identity (`history seal`),
    // and a plain one is refused.
    if let Some(k) = &history_key {
        let vault = cubelang::seal::SealedVault::own(open_identity(k, pass_file.as_deref()));
        HISTORY_VAULT.with(|v| *v.borrow_mut() = Some(std::rc::Rc::new(vault)));
    }
    if keyring.is_some() && pins.is_empty() {
        eprintln!("warning: the keyring's roots are not pinned (--pin-root FINGERPRINT): whoever can write \
                   the keyring file can add a root. `cubelang keyring check` prints the fingerprints.");
    }
    // Fail closed: a keyring or contract set that does not load stops the process, rather than
    // serving with checks the host asked for silently missing.
    let mut host = match Host::open(keyring.as_deref(), contracts.as_deref(), pins) {
        Ok(h) => h,
        Err(e) => { eprintln!("error: {}", e); process::exit(1); }
    };
    if let (Some((cfg, my_pin)), Some(addr)) = (tls_server, &listen) {
        serve_tls(addr, cfg, &mut host, &my_pin);
        return;
    }
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
        // A frame's length is the sender's word: cap it before allocating, so a bad length cannot
        // make the process reserve gigabytes. The stream cannot resynchronise after one, so stop.
        if len > MAX_FRAME_BYTES {
            eprintln!("error: a RunRequest of {} bytes is over the {}-byte limit", len, MAX_FRAME_BYTES);
            process::exit(1);
        }

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

        let result = run_request_result(req, &mut host);

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

/// The largest RunRequest frame `run-proto` reads (programs travel as source; facts travel by path).
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

// ── run-proto over the network (2026-10-24): TLS 1.3, `cubelang::tls` ─────────

/// Connections a listening run-proto holds at once; one more is closed at once.
const MAX_CONNECTIONS: usize = 64;
/// A handshake (and each write) gets this long; a peer that stalls is dropped.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// A connection that sends nothing for this long is closed.
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// One frame (4-byte big-endian length, then the bytes), or `None` at a clean end of stream.
fn read_frame<R: Read>(input: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut len_buf = [0u8; 4];
    match input.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(format!("cannot read a frame length: {}", e)),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(format!("a frame of {} bytes is over the {}-byte limit", len, MAX_FRAME_BYTES));
    }
    let mut buf = vec![0u8; len];
    input.read_exact(&mut buf).map_err(|e| format!("cannot read a frame body: {}", e))?;
    Ok(Some(buf))
}

fn write_frame<W: Write>(out: &mut W, bytes: &[u8]) -> std::io::Result<()> {
    out.write_all(&(bytes.len() as u32).to_be_bytes())?;
    out.write_all(bytes)?;
    out.flush()
}

/// A request handed from a connection's thread to the one thread that runs requests (it owns the
/// Host, the Governor and the caches), with where its result goes.
type Job = (RunRequest, std::sync::mpsc::Sender<Vec<u8>>);

/// `run-proto --listen ADDR --tls-key KEY.json --client-pin PIN...`: serve RunRequests over TLS 1.3
/// to the pinned clients only. Connections are read on their own threads; requests run one at a time
/// on this one, against the same Host as the pipe would.
fn serve_tls(addr: &str, cfg: std::sync::Arc<rustls::ServerConfig>, host: &mut Host, my_pin: &str) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = std::net::TcpListener::bind(addr)
        .unwrap_or_else(|e| { eprintln!("error: run-proto --listen {}: {}", addr, e); process::exit(1) });
    let local = listener.local_addr().map(|a| a.to_string()).unwrap_or_else(|_| addr.to_string());
    eprintln!("run-proto: listening on {} (TLS 1.3, SecP384r1MLKEM1024, AES-256-GCM, pinned clients only); \
               this host's pin: {}", local, my_pin);
    let (jobs, queue) = std::sync::mpsc::channel::<Job>();
    let active = std::sync::Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                continue; // dropped: closed at once
            }
            active.fetch_add(1, Ordering::SeqCst);
            let (cfg, jobs, active) = (cfg.clone(), jobs.clone(), active.clone());
            std::thread::spawn(move || {
                serve_connection(stream, cfg, jobs);
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    for (req, reply) in queue {
        let result = run_request_result(req, host);
        let mut buf = Vec::new();
        result.encode(&mut buf).expect("encoding RunResult into a Vec cannot fail");
        let _ = reply.send(buf);
    }
}

/// One client: the handshake (bounded in time), then frames until it closes, stalls or misbehaves.
/// A bad frame closes this connection only; the process serves on.
fn serve_connection(stream: std::net::TcpStream, cfg: std::sync::Arc<rustls::ServerConfig>,
                    jobs: std::sync::mpsc::Sender<Job>) {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_else(|_| "?".into());
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let conn = match rustls::ServerConnection::new(cfg) {
        Ok(c) => c,
        Err(e) => { eprintln!("run-proto: {}: {}", peer, e); return; }
    };
    let mut tls = rustls::StreamOwned::new(conn, stream);
    while tls.conn.is_handshaking() {
        if let Err(e) = tls.conn.complete_io(&mut tls.sock) {
            eprintln!("run-proto: {} refused: {}", peer, e);
            return;
        }
    }
    let who = cubelang::tls::peer_pin(tls.conn.peer_certificates()).unwrap_or_default();
    eprintln!("run-proto: {} connected (client {})", peer, &who[..who.len().min(16)]);
    let _ = tls.sock.set_read_timeout(Some(IDLE_TIMEOUT));
    let _ = tls.sock.set_write_timeout(Some(IDLE_TIMEOUT));
    loop {
        let frame = match read_frame(&mut tls) {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => { eprintln!("run-proto: {}: {}", peer, e); break; }
        };
        let req = match RunRequest::decode(&frame[..]) {
            Ok(r) => r,
            Err(e) => { eprintln!("run-proto: {}: cannot decode a RunRequest: {}", peer, e); break; }
        };
        let (reply, result) = std::sync::mpsc::channel();
        if jobs.send((req, reply)).is_err() {
            break;
        }
        let Ok(out) = result.recv() else { break };
        if let Err(e) = write_frame(&mut tls, &out) {
            eprintln!("run-proto: {}: {}", peer, e);
            break;
        }
    }
    tls.conn.send_close_notify();
    let _ = tls.conn.complete_io(&mut tls.sock);
}

/// `run-proto --remote ADDR --tls-key KEY.json --server-pin PIN`: relay stdin's RunRequests to a
/// remote run-proto over TLS 1.3 and its RunResults back to stdout, so a client that spawns
/// `run-proto` (CubbyLLM) reaches a remote host without a TLS stack of its own.
fn relay_tls(addr: &str, cfg: std::sync::Arc<rustls::ClientConfig>) {
    fn fail(addr: &str, what: impl std::fmt::Display) -> ! {
        eprintln!("error: run-proto --remote {}: {}", addr, what);
        process::exit(1)
    }
    // TLS 1.3 sends the client's key after the server's: a host that does not pin it says so
    // only when this side next reads
    const NOT_PINNED: &str = "(is this side's key pinned there? `cubelang tls-pin --key` prints it)";
    let stream = std::net::TcpStream::connect(addr).unwrap_or_else(|e| fail(addr, e));
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let conn = rustls::ClientConnection::new(cfg, cubelang::tls::server_name()).unwrap_or_else(|e| fail(addr, e));
    let mut tls = rustls::StreamOwned::new(conn, stream);
    while tls.conn.is_handshaking() {
        if let Err(e) = tls.conn.complete_io(&mut tls.sock) {
            fail(addr, format!("the TLS handshake failed: {}", e));
        }
    }
    // a request may run for as long as its budget allows
    let _ = tls.sock.set_read_timeout(None);
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    loop {
        let frame = match read_frame(&mut input) {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => fail(addr, format!("stdin: {}", e)),
        };
        if let Err(e) = write_frame(&mut tls, &frame) {
            fail(addr, format!("{} {}", e, NOT_PINNED));
        }
        match read_frame(&mut tls) {
            Ok(Some(reply)) => {
                if write_frame(&mut out, &reply).is_err() {
                    break;
                }
            }
            Ok(None) => fail(addr, format!("the host closed the connection {}", NOT_PINNED)),
            Err(e) => fail(addr, format!("{} {}", e, NOT_PINNED)),
        }
    }
    tls.conn.send_close_notify();
    let _ = tls.conn.complete_io(&mut tls.sock);
}

/// `cubelang tls-pin --key KEY.json [--passphrase-file P]`: the pin (SHA-384 of the public key) of
/// the identity's TLS key, what the other side of a remote run-proto passes as `--server-pin` or
/// `--client-pin`. The key comes from the identity's seed: there is no TLS key file.
fn cmd_tls_pin(args: &[String]) {
    let (mut key, mut pass_file) = (None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--key" => { key = Some(flag_value(args, i, "--key")); i += 1; }
            "--passphrase-file" => { pass_file = Some(flag_value(args, i, "--passphrase-file")); i += 1; }
            other => { eprintln!("error: tls-pin: unknown option {}", other); process::exit(1); }
        }
        i += 1;
    }
    let Some(key) = key else { eprintln!("usage: cubelang tls-pin --key KEY.json [--passphrase-file P]"); process::exit(1) };
    let me = open_identity(&key, pass_file.as_deref());
    match cubelang::tls::pin(&me) {
        Ok(p) => println!("{}", p),
        Err(e) => { eprintln!("error: {}", e); process::exit(1); }
    }
}

/// The budget a locked process imposes on a request that is not signed by a model or an agent: the
/// request may ask for less, never more, and an unset field (zero, "unlimited") gets the cap.
const UNTRUSTED_BUDGET: cubelang::reasoning::Budget = cubelang::reasoning::Budget {
    max_jumps: 1_000_000,
    max_ops: 5_000_000,
    max_queries: 1_000,
    max_wall_ms: 5_000,
};

fn clamp_budget(budget: &mut Option<cubelang::reasoning::Budget>) {
    let cap = UNTRUSTED_BUDGET;
    let b = budget.get_or_insert(cubelang::reasoning::Budget { max_jumps: 0, max_ops: 0, max_queries: 0, max_wall_ms: 0 });
    let lower = |asked: u64, cap: u64| if asked == 0 { cap } else { asked.min(cap) };
    b.max_jumps = lower(b.max_jumps as u64, cap.max_jumps as u64) as _;
    b.max_ops = lower(b.max_ops as u64, cap.max_ops as u64) as _;
    b.max_queries = lower(b.max_queries as u64, cap.max_queries as u64) as _;
    b.max_wall_ms = lower(b.max_wall_ms as u64, cap.max_wall_ms as u64) as _;
}

/// What the resident `run-proto` keeps across requests (2026-10-24). Each request still runs on a
/// fresh VM; these are the parts that would mean nothing if they reset per request:
/// - the Governor: the `@ratelimit` windows and the `@cron` clock;
/// - with `--keyring`: the verifier (public keys, pinned roots, the nonces seen), reloaded when the
///   file's mtime moves, so a `cubelang keyring register` takes effect on the next request;
/// - with `--contracts`: the SHA-384 of each deployed contract's source, and the directory itself.
///
/// Either option LOCKS the process. A request that is not signed by a model or an agent may run only
/// a deployed contract, may name a knowledge file only inside the contracts directory, and runs
/// under `UNTRUSTED_BUDGET`. Without either, the process serves its parent as before (a local pipe:
/// only the host can write to it).
struct Host {
    governor: cubelang::vm::governor::Governor,
    locked: bool,
    keyring_path: Option<String>,
    keyring_mtime: Option<std::time::SystemTime>,
    verifier: Option<cubelang::auth::Verifier>,
    contracts_dir: Option<PathBuf>,
    deployed: std::collections::HashSet<[u8; 48]>,
}

fn file_mtime(path: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

impl Host {
    fn open(keyring: Option<&str>, contracts: Option<&str>, pins: Vec<String>) -> Result<Host, String> {
        let mut h = Host {
            governor: cubelang::vm::governor::Governor::new(),
            locked: keyring.is_some() || contracts.is_some(),
            keyring_path: None,
            keyring_mtime: None,
            verifier: None,
            contracts_dir: None,
            deployed: std::collections::HashSet::new(),
        };
        if let Some(p) = keyring {
            let text = std::fs::read_to_string(p).map_err(|e| format!("--keyring {}: {}", p, e))?;
            let ring = cubelang::auth::Keyring::from_json(&text).map_err(|e| format!("--keyring {}: {}", p, e))?;
            h.verifier = Some(cubelang::auth::Verifier::new(ring, pins).map_err(|e| format!("--keyring {}: {}", p, e))?);
            h.keyring_mtime = file_mtime(p);
            h.keyring_path = Some(p.to_string());
        }
        if let Some(dir) = contracts {
            let canon = std::fs::canonicalize(dir).map_err(|e| format!("--contracts {}: {}", dir, e))?;
            let entries = std::fs::read_dir(&canon).map_err(|e| format!("--contracts {}: {}", dir, e))?;
            for entry in entries {
                let path = entry.map_err(|e| format!("--contracts {}: {}", dir, e))?.path();
                if path.extension().is_some_and(|x| x == "cube") {
                    let src = std::fs::read_to_string(&path)
                        .map_err(|e| format!("--contracts {}: {}", path.display(), e))?;
                    // a contract that does not compile strictly is not deployed: refuse to start
                    compiler::compile_strict(&src).map_err(|e| format!("--contracts {}: {}", path.display(), e))?;
                    h.deployed.insert(cubelang::auth::program_digest(&src));
                }
            }
            h.contracts_dir = Some(canon);
        }
        Ok(h)
    }

    /// Reload the keyring when its file changed. A reload that fails its chain keeps the previous
    /// keyring (requests stay checked against keys that verified) and says so once on stderr.
    fn refresh(&mut self) {
        let (Some(p), Some(v)) = (&self.keyring_path, self.verifier.as_mut()) else { return };
        let m = file_mtime(p);
        if m == self.keyring_mtime {
            return;
        }
        self.keyring_mtime = m;
        let loaded = std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|t| cubelang::auth::Keyring::from_json(&t))
            .and_then(|r| v.replace_keyring(r));
        if let Err(e) = loaded {
            eprintln!("warning: keyring {} changed but was refused (keeping the previous one): {}", p, e);
        }
    }

    /// Who the request speaks for (`None`: anonymous) and whether it is trusted (an unlocked process,
    /// or a signed model or agent), or why it is refused before anything runs.
    fn admit_request(&mut self, req: &RunRequest) -> Result<(Option<String>, bool), String> {
        use cubelang::auth::{CallClaim, HybridSignature, Kind, TICK};
        let caller = match req.caller.as_ref().filter(|c| !c.name.is_empty()) {
            None => None,
            Some(c) => {
                let Some(v) = self.verifier.as_mut() else {
                    return Err(format!("the request claims to be {} but this process loaded no keyring \
                                        (run-proto --keyring)", c.name));
                };
                let claim = CallClaim {
                    program_source: &req.program,
                    function: if req.tick { TICK } else { req.fn_name.as_str() },
                    args: &req.args,
                    answers: &req.answers,
                    knowledge_path: &req.knowledge_path,
                    caller: &c.name,
                    timestamp_ms: c.timestamp_ms,
                    nonce: c.nonce,
                    history_path: &req.history_path,
                };
                let sig = HybridSignature { ml_dsa: c.ml_dsa.clone(), ed25519: c.ed25519.clone() };
                Some(v.check(&c.name, &sig, c.timestamp_ms, c.nonce, &claim.message(), cubelang::auth::now_ms())?)
            }
        };
        if !self.locked {
            return Ok((caller, true));
        }
        let kind = caller.as_deref().and_then(|n| self.verifier.as_ref().and_then(|v| v.kind_of(n)));
        if matches!(kind, Some(Kind::Model | Kind::Agent)) {
            return Ok((caller, true));
        }
        let who = match (&caller, kind) {
            (Some(n), Some(k)) => format!("{} (a {})", n, k.as_str()),
            _ => "an anonymous request".to_string(),
        };
        if !self.deployed.contains(&cubelang::auth::program_digest(&req.program)) {
            return Err(format!("{} may run only a deployed contract (run-proto --contracts); this program \
                                is not one -- only a signed model or agent may submit a new program", who));
        }
        if !req.knowledge_path.is_empty() {
            let inside = self.contracts_dir.as_ref().is_some_and(|dir| {
                std::fs::canonicalize(&req.knowledge_path).is_ok_and(|p| p.starts_with(dir))
            });
            if !inside {
                return Err(format!("{} may name a knowledge file only inside the contracts directory", who));
            }
        }
        // A history is someone's conversation: no unsigned or user request reads one by naming it.
        if !req.history_path.is_empty() {
            return Err(format!("{} may not name a history; only a signed model or agent may", who));
        }
        Ok((caller, false))
    }
}

// The resident process's knowledge cache: path -> (mtime, store).
// A request naming a `knowledge_path` gets a CLONE of the store on its fresh
// VM; the file is re-read only when its mtime moves. Keyed by the path
// string as sent, so two spellings of one file parse twice -- harmless, and
// simpler than canonicalizing on a platform where that can fail.
//
// 2026-10-24: the cached store is frozen into one shared segment (a
// `.cubeseg` file already is one), so the per-request clone -- and every
// branch's -- copies a pointer, not the store.
thread_local! {
    static KNOWLEDGE_CACHE: std::cell::RefCell<
        std::collections::HashMap<String, (Option<std::time::SystemTime>, cubelang::vm::knowledge::KnowledgeStore)>
    > = std::cell::RefCell::new(std::collections::HashMap::new());
    /// The same for history directories, keyed by the manifest's mtime.
    static HISTORY_CACHE: std::cell::RefCell<
        std::collections::HashMap<String, (Option<std::time::SystemTime>, cubelang::vm::history::History)>
    > = std::cell::RefCell::new(std::collections::HashMap::new());
    /// `run-proto --history-key`: the identity every history is sealed to.
    static HISTORY_VAULT: std::cell::RefCell<Option<std::rc::Rc<cubelang::seal::SealedVault>>> =
        const { std::cell::RefCell::new(None) };
    /// The highest generation each sealed history directory has been served at: an older one put
    /// back in its place is refused for the life of the process.
    static HISTORY_GENERATION: std::cell::RefCell<std::collections::HashMap<String, u64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
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
            let fresh = store_cli::open_knowledge(path).map_err(|e| format!("knowledge_path {}", e))?;
            KNOWLEDGE_CACHE.with(|c| { c.borrow_mut().insert(path.to_string(), (mtime, fresh.clone())); });
            fresh
        }
    };
    vm.knowledge = store;
    Ok(())
}

/// Load the history directory `path` into `vm.history` (through the cache; re-opened when its
/// manifest changes). A directory that cannot be opened is an error, not an empty history.
fn load_history_cached(vm: &mut VM, path: &str) -> Result<(), String> {
    if !Path::new(path).is_dir() {
        return Err(format!("history_path {}: not a directory", path));
    }
    let vault = HISTORY_VAULT.with(|v| v.borrow().clone());
    let manifest = Path::new(path).join(if vault.is_some() { "MANIFEST.cubeseal" } else { "MANIFEST.json" });
    let mtime = std::fs::metadata(&manifest).ok().and_then(|m| m.modified().ok());
    let hit = HISTORY_CACHE.with(|c| {
        c.borrow().get(path).and_then(|(m, h)| if *m == mtime { Some(h.clone()) } else { None })
    });
    let history = match hit {
        Some(h) => h,
        None => {
            let v = vault.as_deref().map(|v| v as &dyn cubelang::vm::segment::Vault);
            let fresh = cubelang::vm::history::History::open_dir_in(Path::new(path), v)
                .map_err(|e| format!("history_path {}: {}", path, e))?;
            if vault.is_some() {
                let seen = HISTORY_GENERATION.with(|g| g.borrow().get(path).copied().unwrap_or(0));
                if fresh.generation() < seen {
                    return Err(format!("history_path {}: generation {} is older than generation {} this process \
                                        already served -- an older copy was put back in its place; refusing it",
                                       path, fresh.generation(), seen));
                }
                HISTORY_GENERATION.with(|g| { g.borrow_mut().insert(path.to_string(), fresh.generation()); });
            }
            HISTORY_CACHE.with(|c| { c.borrow_mut().insert(path.to_string(), (mtime, fresh.clone())); });
            fresh
        }
    };
    vm.history = history;
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
/// The branch report: what the VM did, read off its counters. On every
/// result, ok or not -- a refused or erroring branch still cost what it cost.
fn branch_report(vm: &VM, branch_id: &str, excluded: &[String], assumed: u32, wall_ms: u32) -> cubelang::reasoning::BranchReport {
    cubelang::reasoning::BranchReport {
        branch_id: branch_id.to_string(),
        ops: vm.ops_executed,
        jumps: vm.jumps_taken,
        queried: vm.queried.iter().map(|(k, n)| cubelang::reasoning::Queried { key: k.clone(), hits: *n as u32 }).collect(),
        excluded: excluded.len() as u32,
        assumed,
        excluded_keys: excluded.to_vec(),
        max_jumps: vm.jump_budget,
        budget: Some(cubelang::reasoning::Budget {
            max_jumps: vm.jump_budget,
            max_ops: vm.op_budget,
            max_queries: vm.query_budget as u32,
            max_wall_ms: vm.wall_budget_ms,
        }),
        wall_ms,
        recovers: vm.recovers.iter().map(|r| cubelang::reasoning::Recover {
            reg: r.reg.clone(),
            role: r.role.clone(),
            winner: r.winner.clone().unwrap_or_default(),
            similarity: r.similarity,
            runner_up: r.runner_up,
            margin: r.margin,
            pool: r.pool as u32,
            reason: recover_reason_wire(r.reason) as i32,
        }).collect(),
    }
}

/// The VM's closed reason enum onto the wire's.
fn recover_reason_wire(r: cubelang::vm::engine::RecoverReason) -> cubelang::reasoning::RecoverReason {
    use cubelang::reasoning::RecoverReason as W;
    use cubelang::vm::engine::RecoverReason as R;
    match r {
        R::Recovered => W::Recovered,
        R::AbsentRole => W::AbsentRole,
        R::NoFrame => W::NoFrame,
        R::EmptyPool => W::EmptyPool,
    }
}

/// Apply `req.budget` to the VM (VM step 0): every field is "unlimited"
/// when zero. `Budget.max_jumps` supersedes the older `Branch.max_jumps`.
fn apply_budget(vm: &mut VM, budget: &Option<cubelang::reasoning::Budget>) {
    let Some(b) = budget else { return };
    if b.max_jumps > 0 { vm.jump_budget = b.max_jumps; }
    vm.op_budget = b.max_ops;
    vm.query_budget = b.max_queries as u64;
    vm.wall_budget_ms = b.max_wall_ms;
    if b.max_wall_ms > 0 {
        vm.deadline = Some(std::time::Instant::now() + std::time::Duration::from_millis(b.max_wall_ms as u64));
    }
}

/// Apply `req.branch` to the VM's (cloned) store: exclusions first, then
/// assumptions, so an assumed fact under an excluded key survives (the branch
/// said both, and the later word wins). Returns (excluded keys, assumed facts).
fn apply_branch(vm: &mut VM, branch: &Option<cubelang::reasoning::Branch>) -> Result<(Vec<String>, u32), String> {
    let Some(b) = branch else { return Ok((Vec::new(), 0)) };
    if b.max_jumps > 0 {
        vm.jump_budget = b.max_jumps;
    }
    // echoed back as applied: the normalized key once for itself and once per alias it took with it,
    // so `excluded.len()` keeps counting aliases and a key that matched nothing is visibly absent
    let mut excluded: Vec<String> = Vec::new();
    for key in &b.exclude {
        let n = vm.knowledge.remove(key);
        for _ in 0..n {
            excluded.push(cubelang::vm::knowledge::normalize(key));
        }
    }
    let mut assumed = 0u32;
    if !b.assume.is_empty() {
        let data = b.assume.join("\n");
        let (facts, aliases) = vm.knowledge.load_jsonl(&data)
            .map_err(|e| format!("branch {}: assume: {}", b.id, e))?;
        assumed = (facts + aliases) as u32;
    }
    Ok((excluded, assumed))
}

/// A non-ok `RunResult`: the error, the report, and every side-channel
/// unset (`similarity`, `runner_up`, `recover_reason` -- a number that
/// described an earlier recover must not sit beside `ok:false`).
fn error_result(e: String, report: Option<cubelang::reasoning::BranchReport>) -> RunResult {
    RunResult { ok: false, result: Some(run_result::Result::Error(e)), report, ..Default::default() }
}

/// One request against the resident host: who is calling first (refused before anything compiles),
/// then a fresh VM that borrows the host's Governor for the run and hands it back on every path.
fn run_request_result(mut req: RunRequest, host: &mut Host) -> RunResult {
    host.refresh();
    let branch_id = req.branch.as_ref().map(|b| b.id.clone()).unwrap_or_default();
    let caller = match host.admit_request(&req) {
        Ok((c, trusted)) => {
            if !trusted {
                clamp_budget(&mut req.budget);
            }
            c
        }
        Err(e) => return error_result(e, Some(branch_report(&VM::new(), &branch_id, &[], 0, 0))),
    };
    let mut vm = VM::new();
    vm.caller = caller;
    vm.governor = std::mem::take(&mut host.governor);
    let result = run_request_on(&mut vm, req, &branch_id);
    host.governor = std::mem::take(&mut vm.governor);
    result
}

fn run_request_on(vm: &mut VM, req: RunRequest, branch_id: &str) -> RunResult {
    let branch_id = branch_id.to_string();
    let programs = match compiler::compile_strict(&req.program) {
        Ok(p) => p,
        Err(e) => return error_result(e.to_string(), Some(branch_report(vm, &branch_id, &[], 0, 0))),
    };
    if programs.is_empty() {
        return error_result("no programs in request".to_string(),
                            Some(branch_report(vm, &branch_id, &[], 0, 0)));
    }

    if !req.knowledge_path.is_empty() {
        if let Err(e) = load_knowledge_cached(vm, &req.knowledge_path) {
            return error_result(e, Some(branch_report(vm, &branch_id, &[], 0, 0)));
        }
    }
    if !req.history_path.is_empty() {
        if let Err(e) = load_history_cached(vm, &req.history_path) {
            return error_result(e, Some(branch_report(vm, &branch_id, &[], 0, 0)));
        }
    }
    let (excluded, assumed) = match apply_branch(vm, &req.branch) {
        Ok(n) => n,
        Err(e) => return error_result(e, Some(branch_report(vm, &branch_id, &[], 0, 0))),
    };
    // The budget clock starts here: the store is loaded and the branch
    // applied; what follows is the program's own work.
    let started = std::time::Instant::now();
    apply_budget(vm, &req.budget);
    let report = |vm: &VM| Some(branch_report(vm, &branch_id, &excluded, assumed,
                                              started.elapsed().as_millis().min(u32::MAX as u128) as u32));
    let prog_name = load_programs(vm, programs);

    // A scheduler tick (2026-10-24): the runtime runs the constructor, then every `@cron` function
    // of the program that is due by the host's clock (the Governor this request borrowed).
    if req.tick {
        if let ExecResult::Error(e) = vm.boot(&prog_name) {
            return error_result(e, report(vm));
        }
        let crons = vm.tick(&prog_name, std::time::Instant::now())
            .into_iter()
            .map(|(function, r)| cron_run(function, r))
            .collect();
        return RunResult { ok: true, crons, report: report(vm), ..Default::default() };
    }

    let call_args: Vec<Value> = req.args.into_iter().map(Value::Str).collect();

    let mut exec_result = call_with_constructor(vm, &prog_name, &req.fn_name, call_args);

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
                return RunResult { ok: true, result: Some(run_result::Result::Suspended(suspended)),
                                   report: report(vm), ..Default::default() };
            }
            Some(raw) => {
                let parsed = match serde_json::from_str::<serde_json::Value>(&raw) {
                    Ok(j) => j,
                    Err(e) => return error_result(
                        format!("resume: answer is not JSON ({}): {}", raw, e), report(vm)),
                };
                let answer = match json_to_value(&parsed) {
                    Ok(v) => v,
                    Err(e) => return error_result(
                        format!("resume: bad answer {}: {}", raw, e), report(vm)),
                };
                exec_result = vm.resume(susp, answer);
            }
        }
    }
    // Task 7 (Feature A, similarity-surfacing): read after the call, since
    // it's VM state the call itself populates. "The last recover in this
    // run" is well-defined here -- this function builds a fresh `VM::new()`
    // per request (above) and the reasoning program does exactly one
    // `recover` per function. VM step 0 adds the runner-up and the reason
    // beside it; `report.recovers` has every hop.
    let similarity = vm.last_recover_similarity;
    let runner_up = vm.last_recover_runner_up;
    let recover_reason = vm.last_recover_reason.map(|r| recover_reason_wire(r) as i32);

    match exec_result {
        ExecResult::Ok(val) | ExecResult::Return(val) => {
            RunResult { ok: true, result: value_to_run_result(&val), similarity, runner_up, recover_reason,
                        report: report(vm), crons: Vec::new() }
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
            error_result("suspended after the answers loop (internal)".to_string(), report(vm))
        }
        ExecResult::Error(e) => error_result(e, report(vm)),
    }
}

/// One `@cron` run onto the wire. A cron that ASKs has no one to answer it: an error, never a guess.
fn cron_run(function: String, r: ExecResult) -> cubelang::reasoning::CronRun {
    use cubelang::reasoning::CronRun;
    match r {
        ExecResult::Ok(v) | ExecResult::Return(v) => {
            let value = match value_to_run_result(&v) {
                Some(run_result::Result::Symbol(s)) => s,
                _ => String::new(),
            };
            CronRun { function, ok: true, value, error: String::new() }
        }
        ExecResult::Suspend(s) => CronRun {
            function,
            ok: false,
            value: String::new(),
            error: format!("asked {} with no one to answer: a @cron function runs unattended",
                           value_to_json(&s.question)),
        },
        ExecResult::Error(e) => CronRun { function, ok: false, value: String::new(), error: e },
    }
}

// ── tick / keygen / keyring / sign ─────────────────────────────────────────

/// `cubelang tick FILE [--every 1s] [--ticks N] [--loose]`: one VM, its constructor, then a tick every
/// interval, printing each `@cron` run. `--ticks 0` runs until stopped. Storage persists across ticks
/// here (one VM), unlike `run-proto`, whose requests each get a fresh VM.
fn cmd_tick(args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: cubelang tick <file.cube|file.cubebin> [--every 1s] [--ticks N] [--loose]");
        process::exit(1);
    }
    let mut every = std::time::Duration::from_secs(1);
    let mut ticks: u64 = 1;
    let mut strict = true;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--every" if i + 1 < args.len() => {
                every = cubelang::vm::governor::parse_period(&args[i + 1]).unwrap_or_else(|e| {
                    eprintln!("error: --every: {}", e);
                    process::exit(1)
                });
                i += 1;
            }
            "--ticks" if i + 1 < args.len() => {
                ticks = args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("error: --ticks takes a whole number (0 = until stopped)");
                    process::exit(1)
                });
                i += 1;
            }
            "--loose" => strict = false,
            "--strict" => strict = true,
            other => { eprintln!("warning: ignoring unknown option {}", other); }
        }
        i += 1;
    }
    let mut vm = VM::new();
    let prog = load_input(&mut vm, &args[0], strict);
    if vm.due_crons(&prog, std::time::Instant::now()).is_empty() {
        eprintln!("error: {} has no @cron function: nothing to tick", prog);
        process::exit(1);
    }
    if let ExecResult::Error(e) = vm.boot(&prog) {
        eprintln!("error: {}", e);
        process::exit(1);
    }
    let mut n = 0u64;
    loop {
        for (name, r) in vm.tick(&prog, std::time::Instant::now()) {
            let run = cron_run(name, r);
            if run.ok {
                println!("  {}.{}() -> {}", prog, run.function, run.value);
            } else {
                eprintln!("  {}.{}() error: {}", prog, run.function, run.error);
            }
        }
        n += 1;
        if ticks != 0 && n >= ticks {
            break;
        }
        std::thread::sleep(every);
    }
}

/// The value after a flag, or exit naming the flag.
fn flag_value(args: &[String], i: usize, flag: &str) -> String {
    match args.get(i + 1) {
        Some(v) => v.clone(),
        None => { eprintln!("error: {} needs a value", flag); process::exit(1); }
    }
}

/// Write a new file, never over an existing one; owner-only on Unix (a private key).
fn write_new_private(path: &str, text: &str) -> Result<(), String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(|e| format!("{}: {}", path, e))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("{}: {}", path, e))
}

/// Replace a file by writing a sibling and renaming it over (a crash leaves the old file whole).
fn write_replace(path: &str, text: &str) -> Result<(), String> {
    let tmp = format!("{}.tmp", path);
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {}", tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {}", path, e))
}

/// The passphrase that seals or opens a key file: `--passphrase-file PATH` (its first line), else the
/// `CUBELANG_KEY_PASSPHRASE` environment variable. Never a command-line argument (it would sit in the
/// process list and the shell history).
fn passphrase(file: Option<&str>) -> Option<zeroize::Zeroizing<String>> {
    if let Some(p) = file {
        let text = zeroize::Zeroizing::new(std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("error: --passphrase-file {}: {}", p, e);
            process::exit(1)
        }));
        return Some(zeroize::Zeroizing::new(text.lines().next().unwrap_or("").to_string()));
    }
    std::env::var("CUBELANG_KEY_PASSPHRASE").ok().map(zeroize::Zeroizing::new)
}

/// Open a key file, with its passphrase when it is sealed.
fn open_identity(path: &str, pass_file: Option<&str>) -> cubelang::auth::Identity {
    let file: cubelang::auth::PrivateKeyFile = read_json(path);
    let pass = passphrase(pass_file);
    cubelang::auth::Identity::load(&file, pass.as_deref().map(|s| s.as_str()))
        .unwrap_or_else(|e| { eprintln!("error: {}: {}", path, e); process::exit(1) })
}

/// `cubelang keygen NAME --kind KIND --out key.json [--model-sha256 HEX] [--passphrase-file P |
/// --plaintext]`: a fresh identity. The seed file is sealed with the passphrase (Argon2id +
/// AES-256-GCM) unless `--plaintext` says otherwise by name; it is never written over an existing
/// file, and the seed is never printed. Prints the public entry to register.
fn cmd_keygen(args: &[String]) {
    use cubelang::auth::{keygen, Kind, Protect};
    let Some(name) = args.first() else {
        eprintln!("usage: cubelang keygen NAME --kind user|agent|model|registrar --out key.json \
                   [--model-sha256 HEX] [--passphrase-file P | --plaintext]");
        process::exit(1);
    };
    let (mut kind, mut out, mut model, mut pass_file, mut plaintext) = (None, None, None, None, false);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--kind" => { kind = Some(flag_value(args, i, "--kind")); i += 1; }
            "--out" => { out = Some(flag_value(args, i, "--out")); i += 1; }
            "--model-sha256" => { model = Some(flag_value(args, i, "--model-sha256")); i += 1; }
            "--passphrase-file" => { pass_file = Some(flag_value(args, i, "--passphrase-file")); i += 1; }
            "--plaintext" => plaintext = true,
            other => { eprintln!("error: keygen: unknown option {}", other); process::exit(1); }
        }
        i += 1;
    }
    let (Some(kind), Some(out)) = (kind, out) else {
        eprintln!("error: keygen needs --kind and --out");
        process::exit(1);
    };
    let kind = Kind::parse(&kind).unwrap_or_else(|e| { eprintln!("error: {}", e); process::exit(1) });
    let pass = if plaintext { None } else {
        Some(passphrase(pass_file.as_deref()).unwrap_or_else(|| {
            eprintln!("error: keygen seals the key file: set CUBELANG_KEY_PASSPHRASE or pass --passphrase-file \
                       (or --plaintext, for a throwaway test identity only)");
            process::exit(1)
        }))
    };
    let protect = match &pass { Some(p) => Protect::Passphrase(p.as_str()), None => Protect::Plaintext };
    let (private, public) = keygen(name, kind, model, protect).unwrap_or_else(|e| { eprintln!("error: {}", e); process::exit(1) });
    let text = serde_json::to_string_pretty(&private).expect("a key file serializes");
    if let Err(e) = write_new_private(&out, &text) {
        eprintln!("error: cannot write the key file (it is never overwritten): {}", e);
        process::exit(1);
    }
    eprintln!("  wrote {} (keep it private: never commit it, never put it in a .cube file or a request)", out);
    println!("{}", serde_json::to_string_pretty(&public).expect("an entry serializes"));
}

fn read_keyring_or_empty(path: &str) -> cubelang::auth::Keyring {
    match std::fs::read_to_string(path) {
        Ok(t) => cubelang::auth::Keyring::from_json(&t).unwrap_or_else(|e| { eprintln!("error: {}: {}", path, e); process::exit(1) }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => cubelang::auth::Keyring::default(),
        Err(e) => { eprintln!("error: {}: {}", path, e); process::exit(1); }
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &str) -> T {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| { eprintln!("error: {}: {}", path, e); process::exit(1) });
    serde_json::from_str(&text).unwrap_or_else(|e| { eprintln!("error: {}: {}", path, e); process::exit(1) })
}

/// `cubelang keyring check RING` | `keyring add-root RING ENTRY.json` |
/// `keyring register RING --by REGISTRAR.key.json [--days N] ENTRY.json` |
/// `keyring revoke RING --by KEY.json NAME [--reason TEXT]`. Every change is checked exactly as a
/// load checks it, then written by replacing the file whole. Sealed registrar keys take their
/// passphrase from `--passphrase-file` or `CUBELANG_KEY_PASSPHRASE`.
fn cmd_keyring(args: &[String]) {
    use cubelang::auth::{fingerprint, register, revoke, KeyEntry};
    let usage = "usage: cubelang keyring check RING | keyring add-root RING ENTRY.json | \
                 keyring register RING --by REGISTRAR.key.json [--days N] ENTRY.json | \
                 keyring revoke RING --by KEY.json NAME [--reason TEXT] | \
                 keyring enable-sealing RING --key KEY.json   (sealed keys: --passphrase-file P)";
    let (Some(sub), Some(ring_path)) = (args.first(), args.get(1)) else { eprintln!("{}", usage); process::exit(1) };
    let (mut by, mut days, mut reason, mut pass_file, mut positional) = (None, None, String::new(), None, None);
    let mut key: Option<String> = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--by" => { by = Some(flag_value(args, i, "--by")); i += 1; }
            "--key" => { key = Some(flag_value(args, i, "--key")); i += 1; }
            "--days" => {
                let d: u64 = flag_value(args, i, "--days").parse().unwrap_or_else(|_| { eprintln!("error: --days takes a whole number"); process::exit(1) });
                days = Some(d);
                i += 1;
            }
            "--reason" => { reason = flag_value(args, i, "--reason"); i += 1; }
            "--passphrase-file" => { pass_file = Some(flag_value(args, i, "--passphrase-file")); i += 1; }
            other if positional.is_none() => positional = Some(other.to_string()),
            other => { eprintln!("error: keyring {}: unexpected {}", sub, other); process::exit(1); }
        }
        i += 1;
    }
    let save = |ring: &cubelang::auth::Keyring| {
        if let Err(e) = write_replace(ring_path, &ring.to_json()) { eprintln!("error: {}", e); process::exit(1); }
    };
    match sub.as_str() {
        "check" => {
            let text = std::fs::read_to_string(ring_path).unwrap_or_else(|e| { eprintln!("error: {}: {}", ring_path, e); process::exit(1) });
            match cubelang::auth::Keyring::from_json(&text) {
                Ok(ring) => {
                    let admitted = ring.admitted_names().unwrap_or_default();
                    println!("  {}: the chain verifies ({} root(s), {} registered, {} revocation(s))",
                             ring_path, ring.roots.len(), ring.keys.len(), ring.revocations.len());
                    for e in ring.roots.iter().chain(ring.keys.iter()) {
                        let by = e.registration.as_ref().map(|r| format!(" (registered by {})", r.by)).unwrap_or_else(|| " (root)".into());
                        let state = if admitted.iter().any(|(n, _)| n == &e.name) { "" } else { "  REVOKED" };
                        let sealing = if e.kem.is_some() { "" } else { "\n      no encryption keys: `keyring enable-sealing` with its key" };
                        println!("    {} -- {}{}{}\n      fingerprint {}{}", e.name, e.kind.as_str(), by, state, fingerprint(e), sealing);
                    }
                }
                Err(e) => { eprintln!("error: {}", e); process::exit(1); }
            }
        }
        "revoke" => {
            let (Some(by), Some(name)) = (by, positional) else { eprintln!("{}", usage); process::exit(1) };
            let mut ring = read_keyring_or_empty(ring_path);
            let signer = open_identity(&by, pass_file.as_deref());
            let target: KeyEntry = ring.keys.iter().find(|k| k.name == name).cloned()
                .unwrap_or_else(|| { eprintln!("error: {} is not a registered entry of {}", name, ring_path); process::exit(1) });
            let r = revoke(&signer, &target, cubelang::auth::now_ms(), &reason).unwrap_or_else(|e| { eprintln!("error: {}", e); process::exit(1) });
            if let Err(e) = ring.add_revocation(r) { eprintln!("error: {}", e); process::exit(1); }
            save(&ring);
            println!("  {}: {} revoked by {}", ring_path, name, signer.name);
        }
        "add-root" => {
            let Some(entry_path) = positional else { eprintln!("{}", usage); process::exit(1) };
            let mut ring = read_keyring_or_empty(ring_path);
            let entry: KeyEntry = read_json(&entry_path);
            let fp = fingerprint(&entry);
            if let Err(e) = ring.add_root(entry) { eprintln!("error: {}", e); process::exit(1); }
            save(&ring);
            println!("  {}: root added ({} root(s)); pin it: --pin-root {}", ring_path, ring.roots.len(), fp);
        }
        "register" => {
            let (Some(by), Some(entry_path)) = (by, positional) else { eprintln!("{}", usage); process::exit(1) };
            let mut ring = read_keyring_or_empty(ring_path);
            let registrar = open_identity(&by, pass_file.as_deref());
            let entry: KeyEntry = read_json(&entry_path);
            let name = entry.name.clone();
            let now = cubelang::auth::now_ms();
            let until = days.map(|d| now + d * 86_400_000);
            let signed = register(&registrar, entry, now, until)
                .unwrap_or_else(|e| { eprintln!("error: {}", e); process::exit(1) });
            if let Err(e) = ring.add(signed) { eprintln!("error: {}", e); process::exit(1); }
            save(&ring);
            println!("  {}: {} registered by {}{}", ring_path, name, registrar.name,
                     days.map(|d| format!(" for {} day(s)", d)).unwrap_or_default());
        }
        // An entry made before 2026-10-24 has no encryption keys; its owner adds them.
        "enable-sealing" => {
            let Some(key) = key else { eprintln!("{}", usage); process::exit(1) };
            let mut ring = read_keyring_or_empty(ring_path);
            let me = open_identity(&key, pass_file.as_deref());
            if let Err(e) = ring.enable_sealing(&me) { eprintln!("error: {}", e); process::exit(1); }
            save(&ring);
            println!("  {}: {} can now be sealed to (encryption keys signed by its own keys)", ring_path, me.name);
        }
        _ => { eprintln!("{}", usage); process::exit(1); }
    }
}

/// `cubelang sign --key key.json --program FILE --fn NAME [--arg V]... [--answer J]...
/// [--knowledge PATH] [--tick]`: the `Caller` for one run-proto request, as JSON (signatures hex).
/// Sign the program text exactly as the request will carry it.
fn cmd_sign(args: &[String]) {
    use cubelang::auth::{CallClaim, Purpose, TICK};
    use rand::RngCore;
    let (mut key, mut program, mut function, mut knowledge, mut tick) = (None, None, None, String::new(), false);
    let mut history = String::new();
    let mut pass_file: Option<String> = None;
    let (mut call_args, mut answers) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--key" => { key = Some(flag_value(args, i, "--key")); i += 1; }
            "--program" => { program = Some(flag_value(args, i, "--program")); i += 1; }
            "--fn" => { function = Some(flag_value(args, i, "--fn")); i += 1; }
            "--arg" => { call_args.push(flag_value(args, i, "--arg")); i += 1; }
            "--answer" => { answers.push(flag_value(args, i, "--answer")); i += 1; }
            "--knowledge" => { knowledge = flag_value(args, i, "--knowledge"); i += 1; }
            "--history" => { history = flag_value(args, i, "--history"); i += 1; }
            "--passphrase-file" => { pass_file = Some(flag_value(args, i, "--passphrase-file")); i += 1; }
            "--tick" => tick = true,
            other => { eprintln!("error: sign: unknown option {}", other); process::exit(1); }
        }
        i += 1;
    }
    let (Some(key), Some(program)) = (key, program) else {
        eprintln!("usage: cubelang sign --key key.json --program FILE (--fn NAME | --tick) [--arg V]... [--answer J]... [--knowledge PATH]");
        process::exit(1);
    };
    let function = if tick { TICK.to_string() } else {
        function.unwrap_or_else(|| { eprintln!("error: sign needs --fn NAME or --tick"); process::exit(1) })
    };
    let identity = open_identity(&key, pass_file.as_deref());
    let source = std::fs::read_to_string(&program).unwrap_or_else(|e| { eprintln!("error: {}: {}", program, e); process::exit(1) });
    let timestamp_ms = cubelang::auth::now_ms();
    let nonce = rand::rngs::OsRng.next_u64();
    let claim = CallClaim {
        program_source: &source,
        function: &function,
        args: &call_args,
        answers: &answers,
        knowledge_path: &knowledge,
        caller: &identity.name,
        timestamp_ms,
        nonce,
        history_path: &history,
    };
    let sig = identity.sign(Purpose::Call, &claim.message()).unwrap_or_else(|e| { eprintln!("error: {}", e); process::exit(1) });
    println!("{}", serde_json::json!({
        "name": identity.name,
        "timestamp_ms": timestamp_ms,
        "nonce": nonce,
        "ml_dsa": cubelang::auth::to_hex(&sig.ml_dsa),
        "ed25519": cubelang::auth::to_hex(&sig.ed25519),
    }));
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

/// Load every freshly-compiled program into `vm`, returning the name of the
/// one to run -- the "the first compiled program is the default one to run"
/// convention (see `loader` module doc) that `cmd_run`'s in-memory
/// `.cube`-compile branch and `run-proto` both rely on identically, except
/// that a file with a container runs its container (2026-10-24: the
/// container is the deployment unit, its programs what it runs). Callers
/// check `programs.is_empty()` themselves first (the two call sites report
/// that case with different, path-specific messages).
fn load_programs(vm: &mut VM, programs: Vec<CompiledProgram>) -> String {
    let name = programs.iter().find(|p| p.container.is_some())
        .unwrap_or(&programs[0]).name.clone();
    for p in programs { vm.load(p); }
    name
}

/// Boot the program as the runtime does (its constructor; for a container,
/// its bound programs' constructors, its own, then `on_start`), then call
/// `function` -- the exact sequence `cmd_run` and `run-proto` both need,
/// factored out so it can't drift between the two transports.
fn call_with_constructor(vm: &mut VM, program: &str, function: &str, args: Vec<Value>) -> ExecResult {
    // The runtime runs the constructor (`@system @once` is written for it);
    // the target is the HOST's entry, so its access attribute is enforced
    // (VM step 0: `@internal` / `@system` refused at the door). 2026-10-24:
    // a constructor that fails stops the run (its error used to be dropped,
    // and the entry ran on half-built storage).
    if let ExecResult::Error(e) = vm.boot(program) {
        return ExecResult::Error(e);
    }
    vm.call_entry(program, function, args)
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
