Set-Location 'C:\Users\grill\Documents\GitHub\cubelang'
$exe = '.\target\release\cubelang.exe'

Write-Output '--- for-loop over [10,20,30] (expect 60) ---'
@'
program LoopTest implements ISolver {
  type Input = quantity; type Output = quantity;
  @system @once public function constructor() { create z : quantity; assign z = 0; }
  @external public function parse(raw: str): Input { return raw; }
  @external public function solve(input: Input): Output {
    let xs = [10, 20, 30];
    create total : quantity; assign total = 0;
    for x of xs { add total, x; }
    sum total; return total;
  }
  @external public pure function verify(i: Input, o: Output): bool { return true; }
}
'@ | Out-File -Encoding ascii _t_loop.cube
& $exe run _t_loop.cube 2>&1 | Select-String 'solve|->'

Write-Output '--- CALL double(21) (expect 42) ---'
@'
program CallTest implements ISolver {
  type Input = quantity; type Output = quantity;
  @external public function parse(raw: str): Input { return raw; }
  @external public function dbl(): Output {
    create r : quantity; assign r = 0;
    add r, arg0; add r, arg0;
    sum r; return r;
  }
  @external public function solve(input: Input): Output {
    let y = dbl(21);
    sum y; return y;
  }
  @external public pure function verify(i: Input, o: Output): bool { return true; }
}
'@ | Out-File -Encoding ascii _t_call.cube
& $exe run _t_call.cube 2>&1 | Select-String 'solve|->'

Write-Output '--- strict mode rejects a reasoning opcode (expect error) ---'
@'
program StrictTest implements ISolver {
  type Input = quantity; type Output = quantity;
  @external public function parse(raw: str): Input { return raw; }
  @external public function solve(input: Input): Output {
    create a : quantity; assign a = 1;
    predict a;
    sum a; return a;
  }
  @external public pure function verify(i: Input, o: Output): bool { return true; }
}
'@ | Out-File -Encoding ascii _t_strict.cube
& $exe compile _t_strict.cube --strict 2>&1 | Select-String 'error|predict|strict|compiled'
