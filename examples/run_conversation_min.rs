// examples/run_conversation_min.rs

use std::fs;
use std::path::Path;

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

fn main() {
    let source_path = Path::new("conversation_agent/conversation_min.cube");
    let source = fs::read_to_string(source_path)
        .expect("failed to read conversation_agent/conversation_min.cube");

    let programs = compiler::compile(&source)
        .expect("failed to compile conversation_min.cube");

    if programs.is_empty() {
        panic!("no programs compiled");
    }

    let program = programs[0].clone();
    let program_name = program.name.clone();

    println!("compiled program: {}", program_name);
    for func in &program.functions {
        println!("  fn {} -> {} bytes", func.name, func.bytecode.len());
        println!("  hex: {}", func.hex());
    }

    let mut vm = VM::new();
    vm.load(program);
    vm.trace_enabled = true;

    match vm.call(&program_name, "constructor", Vec::new()) {
        ExecResult::Ok(v) | ExecResult::Return(v) => {
            println!("constructor ok: {}", v);
        }
        ExecResult::Error(e) => {
            eprintln!("constructor error: {}", e);
            std::process::exit(1);
        }
    }

    let input = Value::Str("how do i build opcode rules for new schemas".to_string());

    let result = vm.call(&program_name, "solve", vec![input]);

    match result {
        ExecResult::Ok(v) | ExecResult::Return(v) => {
            println!("solve ok: {}", v);
        }
        ExecResult::Error(e) => {
            eprintln!("solve error: {}", e);
            std::process::exit(1);
        }
    }

    println!("\ntrace:");
    for line in &vm.trace {
        println!("  {}", line);
    }

    println!("\nregisters:");
    for (k, v) in &vm.registers {
        println!("  {} = {}", k, v);
    }

    println!("\nstorage:");
    for (k, v) in &vm.storage {
        println!("  {} = {}", k, v);
    }
}