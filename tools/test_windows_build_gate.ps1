[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Payload,
    [Parameter(Mandatory=$true)][string]$Output
)
# Exercise the production launch wrapper without running a build or installation.
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$source = Join-Path $repo 'platform\windows\setup\build-universal.ps1'
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($source, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'Build script failed to parse.' }
$definition = $ast.Find({ param($node)
    $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Invoke-Checked'
}, $true)
if (!$definition) { throw 'Production Invoke-Checked not found.' }
. ([ScriptBlock]::Create($definition.Extent.Text))
$Payload = [IO.Path]::GetFullPath($Payload)
$Output = [IO.Path]::GetFullPath($Output)
if (Test-Path $Output) { throw 'Output must be a new test directory.' }
New-Item -ItemType Directory $Output | Out-Null
$engine = Join-Path $Payload 'hufu-setup-engine.exe'
if (!(Test-Path $engine)) { throw 'Production maintenance engine missing.' }
$missing = Join-Path $Output 'nonexistent-payload'
$results = @()
foreach ($case in @(
    @{ name='valid-preflight'; exe=$engine; src=$Payload; success=$true },
    @{ name='failed-preflight'; exe=$engine; src=$missing; success=$false },
    @{ name='failed-process-launch'; exe=(Join-Path $missing 'not-an-executable.exe'); src=$missing; success=$false }
)) {
    $LASTEXITCODE = 0 # A stale successful previous command must never admit failure.
    $ok = $true; $message = ''
    try {
        Invoke-Checked $case.exe @('preflight','--src',$case.src,'--log',(Join-Path $Output ($case.name + '.log')))
    } catch { $ok = $false; $message = $_.Exception.Message }
    if ($ok -ne $case.success) { throw "Unexpected result: $($case.name), success=$ok, $message" }
    $results += @{ name=$case.name; passed=$true; accepted=$ok; error=$message }
}
$report = @{ schema=1; scope='read-only preflight and process-start gate; no install/register'; engine_sha256=(Get-FileHash $engine).Hash.ToLowerInvariant(); results=$results }
[IO.File]::WriteAllText((Join-Path $Output 'report.json'), ($report | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding($false)))
Write-Output 'Passed 3 production launch-gate cases.'
