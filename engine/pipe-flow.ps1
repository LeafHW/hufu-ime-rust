# pipe-flow.ps1：逐键流对比——打印每键 commit/raw/候选（与 Rime 探针同格式）
$ErrorActionPreference = 'Stop'
$enc = [System.Text.Encoding]::UTF8
function PipeCall([string]$json) {
  $pipe = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'hufu-ime', [System.IO.Pipes.PipeDirection]::InOut)
  $pipe.Connect(2000)
  $body = $enc.GetBytes($json)
  $pipe.Write([BitConverter]::GetBytes([UInt32]$body.Length), 0, 4)
  $pipe.Write($body, 0, $body.Length); $pipe.Flush()
  $head = New-Object byte[] 4; [void]$pipe.Read($head, 0, 4)
  $n = [BitConverter]::ToUInt32($head, 0); $buf = New-Object byte[] $n
  $read = 0; while ($read -lt $n) { $read += $pipe.Read($buf, $read, $n - $read) }
  $pipe.Close(); ($enc.GetString($buf) | ConvertFrom-Json)
}

[void](PipeCall '{"op":"reset"}')
foreach ($ch in @('s','y','f','t','u','u','u',';','w',';','j','g','f','d')) {
  $r = PipeCall ('{"op":"key","key":"' + $ch + '"}')
  $commit = if ($r.outcome.commit) { "  COMMIT=[$($r.outcome.commit)]" } else { '' }
  $cands = ($r.state.candidates | ForEach-Object { $_.text } | Select-Object -First 4) -join ' / '
  Write-Host ("key [{0}] raw=[{1}]{2}  menu=[{3}]" -f $ch, $r.state.raw, $commit, $cands)
}
$r = PipeCall '{"op":"key","key":" "}'
Write-Host "SPACE COMMIT=[$($r.outcome.commit)] raw=[$($r.state.raw)]"
exit 0