//! Plan-5: cross-repo opcode-byte sync.
//!
//! New opcode bytes are added in three places — cubelang's `mod op`,
//! opcode-vsa-rs's `CubeMindOpcode::bytecode()`, and the cubemind Python VM
//! table — and the three MUST agree. A silent divergence (e.g. RETURN=0x39 in
//! Rust, RETURN=0x40 in Python) ships wrong behavior without failing locally.
//!
//! This test:
//!   1. enumerates cubelang's authoritative (name, byte) pairs;
//!   2. parses `../opcode-vsa-rs/src/ir.rs` if accessible at test time;
//!   3. asserts every shared name has the same byte.
//!
//! When the sibling repo isn't present (clean CI checkout of cubelang alone),
//! the test prints a skip note rather than failing — sync is enforceable only
//! when the sibling source is reachable.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use cubelang::compiler::op;

/// (cubelang-name, vsa-rs-snake-name, byte). The first column is what cubelang
/// names the opcode; the second is the snake_case form derived from
/// `CubeMindOpcode::Foo` in opcode-vsa-rs (most match; a handful diverge
/// stylistically — UNBIND vs UNBIND_ROLE, NEWVAR vs NEW_VAR).
fn cubelang_table() -> Vec<(&'static str, &'static str, u8)> {
    vec![
        ("CREATE",         "CREATE",         op::CREATE),
        ("DESTROY",        "DESTROY",        op::DESTROY),
        ("ASSIGN",         "ASSIGN",         op::ASSIGN),
        ("ADD",            "ADD",            op::ADD),
        ("SUB",            "SUB",            op::SUB),
        ("MUL",            "MUL",            op::MUL),
        ("DIV",            "DIV",            op::DIV),
        ("SUM",            "SUM",            op::SUM),
        ("TRANSFER",       "TRANSFER",       op::TRANSFER),
        ("COPY",           "COPY",           op::COPY),
        ("PUSH",           "PUSH",           op::PUSH),
        ("POP",            "POP",            op::POP),
        ("COMPARE",        "COMPARE",        op::COMPARE),
        ("QUERY",          "QUERY",          op::QUERY),
        ("STORE",          "STORE",          op::STORE),
        ("RECALL",         "RECALL",         op::RECALL),
        ("BIND_ROLE",      "BIND_ROLE",      op::BIND_ROLE),
        ("UNBIND",         "UNBIND_ROLE",    op::UNBIND),
        ("COND",           "COND",           op::COND),
        ("LOOP",           "LOOP",           op::LOOP),
        ("CALL",           "CALL",           op::CALL),
        ("JMP",            "JMP",            op::JMP),
        ("LABEL",          "LABEL",          op::LABEL),
        ("SEQ",            "SEQ",            op::SEQ),
        ("DIFF",           "DIFF",           op::DIFF),
        ("DETECT_PATTERN", "DETECT_PATTERN", op::DETECT_PATTERN),
        ("PREDICT",        "PREDICT",        op::PREDICT),
        ("MATCH",          "MATCH",          op::MATCH),
        ("DEBATE",         "DEBATE",         op::DEBATE),
        ("ASK",            "ASK",            op::ASK),
        ("DISCOVER",       "DISCOVER",       op::DISCOVER),
        ("REMEMBER",       "REMEMBER",       op::REMEMBER),
        ("FORGET",         "FORGET",         op::FORGET),
        ("DECODE",         "DECODE",         op::DECODE),
        ("SCORE",          "SCORE",          op::SCORE),
        ("SPECIALIZE",     "SPECIALIZE",     op::SPECIALIZE),
        ("EXPLORE",        "EXPLORE",        op::EXPLORE),
        ("REWARD",         "REWARD",         op::REWARD),
        ("FORGE",          "FORGE",          op::FORGE),
        ("INFER",          "INFER",          op::INFER),
        ("BROADCAST",      "BROADCAST",      op::BROADCAST),
        ("SYNC",           "SYNC",           op::SYNC),
        ("MERGE",          "MERGE",          op::MERGE),
        ("SPLIT",          "SPLIT",          op::SPLIT),
        ("FILTER",         "FILTER",         op::FILTER),
        ("MAP_ROLES",      "MAP_ROLES",      op::MAP_ROLES),
        ("REDUCE",         "REDUCE",         op::REDUCE),
        ("TEMPORAL_BIND",  "TEMPORAL_BIND",  op::TEMPORAL_BIND),
        ("ANALOGY",        "ANALOGY",        op::ANALOGY),
        ("UNIFY",          "UNIFY",          op::UNIFY),
        ("INST",           "INST",           op::INST),
        ("GEN",            "GEN",            op::GEN),
        ("NEWVAR",         "NEW_VAR",        op::NEWVAR),
        ("RETURN",         "RETURN",         op::RETURN),
        ("MAKE_ARRAY",     "MAKE_ARRAY",     op::MAKE_ARRAY),
        ("LEN",            "LEN",            op::LEN),
        ("INDEX",          "INDEX",          op::INDEX),
        ("SKIP",           "SKIP",           op::SKIP),
    ]
}

/// Convert CamelCase (the form vsa-rs uses) to SCREAMING_SNAKE (cubelang).
fn camel_to_snake(camel: &str) -> String {
    let mut out = String::new();
    for (i, c) in camel.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// Parse opcode-vsa-rs ir.rs for `CubeMindOpcode::Name => 0xXX,` lines.
/// Returns Some(map) on success, None when the file is unavailable.
fn read_vsa_rs_table() -> Option<HashMap<String, u8>> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest.parent()?.join("opcode-vsa-rs").join("src").join("ir.rs");
    let src = fs::read_to_string(&path).ok()?;

    let mut map = HashMap::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("CubeMindOpcode::") else { continue };
        let Some(arrow) = rest.find("=>") else { continue };
        let name_camel = rest[..arrow].trim();
        let after = &rest[arrow + 2..];
        let after = after.trim().trim_end_matches(',');
        let after = after.trim();
        let Some(hex) = after.strip_prefix("0x") else { continue };
        let hex = hex.trim_end_matches(',').trim();
        let Ok(byte) = u8::from_str_radix(hex, 16) else { continue };
        map.insert(camel_to_snake(name_camel), byte);
    }
    if map.is_empty() { None } else { Some(map) }
}

#[test]
fn opcode_bytes_match_opcode_vsa_rs() {
    let vsa = match read_vsa_rs_table() {
        Some(t) => t,
        None => {
            eprintln!("[opcode_sync] skipped: cannot read sibling opcode-vsa-rs/src/ir.rs \
                       (running outside a checkout that has both repos side-by-side)");
            return;
        }
    };

    let mut mismatches: Vec<String> = Vec::new();
    let mut missing:    Vec<String> = Vec::new();

    for (cub_name, vsa_name, byte) in cubelang_table() {
        match vsa.get(vsa_name) {
            Some(&vbyte) if vbyte == byte => {}
            Some(&vbyte) => mismatches.push(format!(
                "{}: cubelang=0x{:02X}, opcode-vsa-rs::{}=0x{:02X}",
                cub_name, byte, vsa_name, vbyte)),
            None => missing.push(format!("{}=0x{:02X} (vsa-rs::{})", cub_name, byte, vsa_name)),
        }
    }

    if !mismatches.is_empty() {
        panic!("opcode byte divergence between cubelang and opcode-vsa-rs:\n  {}",
            mismatches.join("\n  "));
    }
    assert!(missing.is_empty(),
        "cubelang opcodes missing from opcode-vsa-rs (sync rule says they must \
         be added in lockstep):\n  {}", missing.join("\n  "));
}
