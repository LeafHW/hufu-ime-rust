$ErrorActionPreference = 'Stop'
$path = 'E:\DSH-KF\TigerClaw\sentence\Models\sentence-qwen-q8.gguf'
$fs = [System.IO.File]::OpenRead($path)
$br = New-Object System.IO.BinaryReader($fs)

function Read-Str {
  $len = $br.ReadUInt64()
  if ($len -gt 1MB -or $len -gt $fs.Length) { throw "字符串长度异常: $len @ pos $($fs.Position)" }
  $b = $br.ReadBytes([int]$len)
  [System.Text.Encoding]::UTF8.GetString($b)
}
function Read-Value([uint32]$t) {
  switch ($t) {
    0 { $br.ReadByte() }
    1 { $br.ReadSByte() }
    2 { $br.ReadUInt16() }
    3 { $br.ReadInt16() }
    4 { $br.ReadUInt32() }
    5 { $br.ReadInt32() }
    6 { $br.ReadSingle() }
    7 { $br.ReadByte() }
    8 { Read-Str }
    9 {
      $et = $br.ReadUInt32()
      $n = $br.ReadUInt64()
      if ($n -gt 1MB) { throw "数组长度异常: $n @ pos $($fs.Position)" }
      $arr = @()
      for ($i = 0; $i -lt $n; $i++) { $arr += ,(Read-Value $et) }
      ,$arr
    }
    10 { $br.ReadUInt64() }
    11 { $br.ReadInt64() }
    12 { $br.ReadDouble() }
    default { throw "未知类型 $t @ pos $($fs.Position)" }
  }
}

$null = $br.ReadBytes(4)   # magic
$ver = $br.ReadUInt32()
$tc = $br.ReadUInt64()
$kc = $br.ReadUInt64()
Write-Host "version=$ver tensors=$tc kv=$kc"
for ($i = 0; $i -lt $kc; $i++) {
  $key = Read-Str
  $t = $br.ReadUInt32()
  $v = Read-Value $t
  $s = "$v"
  if ($s.Length -gt 60) { $s = $s.Substring(0, 60) }
  Write-Host ("  [{0}] {1} ({2}) = {3}" -f $i, $key, $t, $s)
}
$fs.Close()
exit 0
