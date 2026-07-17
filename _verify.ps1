Set-Location 'C:\Users\grill\Documents\GitHub\cubelang'

# --- full suite, aggregate count ---
cargo test --release *>&1 | Out-File -Encoding utf8 _testout.txt
$txt = Get-Content _testout.txt -Raw
$p = 0; $f = 0
foreach ($m in [regex]::Matches($txt, '(\d+) passed; (\d+) failed')) {
  $p += [int]$m.Groups[1].Value
  $f += [int]$m.Groups[2].Value
}
Write-Output ("SUITE: passed=$p failed=$f")
if ($txt -match 'opcode_bytes_match') { Write-Output 'opcode_sync test: PRESENT' }

# --- feature checks via the CLI ---
$exe = '.\target\release\cubelang.exe'

Write-Output '--- RETURN early-exit (expect 1, side NOT incremented) ---'
@'
program RetTest implements ISolver {
  type Input = quantity; type Output = quantity;
  storage { side: mutable u64 = 0; }
  @system @once public function constructor() { assign side = 0; }
  @external public function parse(raw: str): Input { return raw; }
  @external public function solve(input: Input): Output {
    create d : quantity; assign d = 0;
    assign d = 1;
    return d;
    add side, 99;
    assign d = 7;
    sum d; return d;
  }
  @external public pure function verify(i: Input, o: Output): bool { return true; }
}
'@ | Out-File -Encoding ascii _t_return.cube
& $exe run _t_return.cube 2>&1 | Select-String 'solve|->|side'

Write-Output '--- for-loop (expect total = 60) ---'
& $exe check examples\qc_decision.cube 2>&1 | Select-String 'ok|error'
