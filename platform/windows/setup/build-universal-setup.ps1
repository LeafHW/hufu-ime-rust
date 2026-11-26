[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Version,
    [Parameter(Mandatory=$true)][string]$PayloadRoot,
    [Parameter(Mandatory=$true)][string]$DataSource,
    [Parameter(Mandatory=$true)][string]$DistDir,
    [string]$Iscc = '',
    [string]$Python = 'python',
    [switch]$StageOnly
)
# Universal source is two validated flat payloads (x64/arm64), not an installed app.
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$packageTool = Join-Path $repo 'tools\windows_package.py'
$DistDir = [IO.Path]::GetFullPath($DistDir)
$work = Join-Path $DistDir ('setup-' + [guid]::NewGuid().ToString('N'))
$stage = Join-Path $work 'staging'
New-Item -ItemType Directory -Path $work -Force | Out-Null
$report = & $Python $packageTool stage --payload $PayloadRoot --data $DataSource --out $stage --version $Version
if ($LASTEXITCODE -ne 0) { throw 'Universal staging failed.' }
[IO.File]::WriteAllText((Join-Path $work 'stage-report.json'), ($report -join "`n") + "`n", (New-Object Text.UTF8Encoding($false)))
foreach ($name in @('staging-files.inc','staging-programs.inc')) {
    Copy-Item (Join-Path $stage $name) (Join-Path $work $name)
}
Copy-Item (Join-Path $PSScriptRoot 'hufu-setup.iss') $work
Copy-Item (Join-Path $PSScriptRoot 'universal-code.iss') $work
Copy-Item (Join-Path $PSScriptRoot 'lang') $work -Recurse
if ($StageOnly) { Write-Output $work; return }
if (!$Iscc) {
    $candidate = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($candidate) { $Iscc = $candidate.Source }
    foreach ($location in @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe")) {
        if (!$Iscc -and (Test-Path $location)) { $Iscc = $location }
    }
}
if (!(Test-Path $Iscc)) { throw 'Pass -Iscc with the Inno Setup 6.7+ compiler path.' }
$parts = @($Version.Split('.')) + @('0','0','0')
$ver4 = $parts[0..3] -join '.'
& $Iscc "/DSetupVersion=$Version" "/DSetupVersion4=$ver4" "/DDistDir=$DistDir" /DRUST_ENGINE=1 /DUNIVERSAL=1 (Join-Path $work 'hufu-setup.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC failed: $LASTEXITCODE" }
$exe = Join-Path $DistDir "HuFuSetup-$Version.exe"
$artifact = & $Python $packageTool check-update $exe
if ($LASTEXITCODE -ne 0) { throw 'Installer exceeds legacy online-update limit. Do not publish.' }
[IO.File]::WriteAllText("$exe.artifact.json", ($artifact -join "`n") + "`n", (New-Object Text.UTF8Encoding($false)))
$sha = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText("$exe.sha256", "$sha  $([IO.Path]::GetFileName($exe))`n", [Text.Encoding]::ASCII)
Write-Output $exe
