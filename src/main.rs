//! CubeLang CLI — compile, inspect, and run .cube programs.
//!
//! Usage:
//!   cubelang compile <file.cube>              → file.cubebin
//!   cubelang compile <file.cube> -o out.cubebin
//!   cubelang inspect <file.cubebin>           → print program info
//!   cubelang disasm  <file.cubebin>           → disassemble bytecode
//!   cubelang check   <file.cube>              → parse + type check only
//!   cubelang version

use std::path::{Path, PathBuf};
use std::process;

use cubelang::compiler::{self, CompiledProgram, op};
use cubelang::vm::{VM, ExecResult};

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
    compile,  c    Compile .cube to .cubebin (use --strict to reject stubs)
    run,      r    Compile (or load .cubebin) and execute a program
    inspect,  i    Show .cubebin program info
    disasm,   d    Disassemble .cubebin to readable opcodes
    check,    k    Parse and check .cube without compiling
    validate, vd   Report executing vs trace-only opcodes per function
    version,  v    Print version

EXAMPLES:
    cubelang compile examples/gsm8k.cube
    cubelang compile --strict examples/qc_decision.cube
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
        eprintln!("usage: cubelang compile [--strict] <file.cube> [-o output.cubebin]");
        process::exit(1);
    }

    // Parse flags and positional args. `--strict` (P0-1) rejects non-executing
    // constructs (trace-only opcodes, `for`, `match`, intra-program `call`).
    let mut strict = false;
    let mut positional: Vec<&String> = Vec::new();
    for arg in args {
        if arg == "--strict" { strict = true; } else { positional.push(arg); }
    }

    if positional.is_empty() {
        eprintln!("error: missing input file");
        eprintln!("usage: cubelang compile [--strict] <file.cube> [-o output.cubebin]");
        process::exit(1);
    }

    let input = positional[0];
    let output = if positional.len() >= 3 && positional[1] == "-o" {
        PathBuf::from(positional[2])
    } else {
        Path::new(input).with_extension("cubebin")
    };

    // Read source
    let source = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", input, e);
            process::exit(1);
        }
    };

    // Compile (--strict surfaces non-executing constructs as hard errors).
    let programs = if strict {
        match compiler::compile_strict(&source) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error (--strict): {}", e);
                process::exit(1);
            }
        }
    } else {
        match compiler::compile(&source) {
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
        eprintln!("usage: cubelang check <file.cube>");
        process::exit(1);
    }

    let source = match std::fs::read_to_string(&args[0]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", args[0], e);
            process::exit(1);
        }
    };

    // Parse
    match cubelang::parser::parse(&source) {
        Ok(ast) => {
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
                args[0], n_items, n_programs, n_interfaces, n_structs, n_enums);
        }
        Err(e) => {
            eprintln!("  {} error: {}", args[0], e);
            process::exit(1);
        }
    }
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
        eprintln!("usage: cubelang run <file.cube|file.cubebin> [--fn name] [--trace]");
        process::exit(1);
    }

    let input = &args[0];
    let mut target_fn = "solve".to_string();
    let mut trace = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--fn" | "-f" => { if i + 1 < args.len() { target_fn = args[i + 1].clone(); i += 1; } }
            "--trace" | "-t" => trace = true,
            other => { eprintln!("warning: ignoring unknown option {}", other); }
        }
        i += 1;
    }

    // Load: compile .cube in-memory, or load a .cubebin directly.
    let mut vm = VM::new();
    vm.trace_enabled = trace;
    let prog_name: String = if input.ends_with(".cubebin") {
        match vm.load_file(Path::new(input)) {
            Ok(name) => name,
            Err(e) => { eprintln!("error: cannot load {}: {}", input, e); process::exit(1); }
        }
    } else {
        let source = match std::fs::read_to_string(input) {
            Ok(s) => s,
            Err(e) => { eprintln!("error: cannot read {}: {}", input, e); process::exit(1); }
        };
        let programs = match compiler::compile(&source) {
            Ok(p) => p,
            Err(e) => { eprintln!("error: {}", e); process::exit(1); }
        };
        if programs.is_empty() {
            eprintln!("error: no programs in {}", input);
            process::exit(1);
        }
        let name = programs[0].name.clone();
        for p in programs { vm.load(p); }
        name
    };

    // Run the constructor first if the program defines one (sets up storage).
    let _ = vm.call(&prog_name, "constructor", Vec::new());

    // Run the target function.
    let result = vm.call(&prog_name, &target_fn, Vec::new());

    if trace {
        println!("# trace ({} ops):", vm.trace.len());
        for line in &vm.trace { println!("  {}", line); }
        println!();
    }

    match result {
        ExecResult::Ok(val) | ExecResult::Return(val) => {
            println!("  {}.{}() -> {}", prog_name, target_fn, val);
        }
        ExecResult::Error(e) => {
            eprintln!("  {}.{}() runtime error: {}", prog_name, target_fn, e);
            process::exit(1);
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn load_cubebin(path: &str) -> CompiledProgram {
    match CompiledProgram::load(Path::new(path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot load {}: {}", path, e);
            process::exit(1);
        }
    }
}
