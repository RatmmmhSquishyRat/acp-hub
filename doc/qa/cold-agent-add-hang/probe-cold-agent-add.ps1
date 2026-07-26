#requires -Version 5.1
<#
.SYNOPSIS
  Blind QA probe: cold `agent add` hang (no source knowledge).

.DESCRIPTION
  Outer-user metrics only: wall time, process exit, agent list visibility,
  agents.json contains id. See README.md in this folder.

.NOTES
  Exit 0 = cold path OK or OK_SLOW. Non-zero = gate fail.
#>
$ErrorActionPreference = 'Continue'

$Bin = if ($env:ACP_HUB_BIN) { $env:ACP_HUB_BIN } else { 'acp-hub' }
$BudgetSec = if ($env:PROBE_BUDGET_SEC) { [int]$env:PROBE_BUDGET_SEC } else { 15 }
$SoftSec = if ($env:PROBE_SOFT_SEC) { [int]$env:PROBE_SOFT_SEC } else { 3 }
$AgentId = if ($env:PROBE_AGENT_ID) { $env:PROBE_AGENT_ID } else { 'qa-cold-probe' }
$WarmRounds = if ($env:PROBE_ROUNDS_WARM) { [int]$env:PROBE_ROUNDS_WARM } else { 1 }

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$root = Join-Path ([System.IO.Path]::GetTempPath()) "acp-hub-cold-add-probe-$stamp"
$hubHome = Join-Path $root 'home'
New-Item -ItemType Directory -Force -Path $hubHome | Out-Null

# Fixture agent entry (path must not look like a CLI flag; add may not spawn it).
$fixtureJs = Join-Path $root 'fixture-stdio-agent.js'
@'
// Outer-user fixture: inert if ever spawned. Not Hub source.
process.stdin.resume();
'@ | Set-Content -LiteralPath $fixtureJs -Encoding utf8

$Command = if ($env:PROBE_COMMAND) { $env:PROBE_COMMAND } else { 'node' }
$ArgList = if ($env:PROBE_ARGS) {
  $env:PROBE_ARGS -split '\|%'
} else {
  @($fixtureJs)
}

function Write-Human([string]$msg) {
  Write-Host $msg
}

function Test-AgentListed([string]$HubHome, [string]$id) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $Bin
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.CreateNoWindow = $true
  [void]$psi.ArgumentList.Add('--home'); [void]$psi.ArgumentList.Add($HubHome)
  [void]$psi.ArgumentList.Add('agent'); [void]$psi.ArgumentList.Add('list')
  $p = New-Object System.Diagnostics.Process
  $p.StartInfo = $psi
  [void]$p.Start()
  $out = $p.StandardOutput.ReadToEnd()
  $null = $p.StandardError.ReadToEnd()
  $okWait = $p.WaitForExit(30000)
  if (-not $okWait) {
    try { $p.Kill($true) } catch {}
    return $false
  }
  return ($out -match [regex]::Escape($id))
}

function Test-AgentsJsonHasId([string]$HubHome, [string]$id) {
  $path = Join-Path $HubHome 'agents.json'
  if (-not (Test-Path $path)) { return $false }
  $raw = Get-Content -LiteralPath $path -Raw -ErrorAction SilentlyContinue
  if (-not $raw) { return $false }
  return ($raw -match [regex]::Escape($id))
}

function Invoke-AgentAdd([string]$HubHome, [string]$id, [string]$cmd, [string[]]$CmdArgs, [int]$budgetSec) {
  $tag = [guid]::NewGuid().ToString('n').Substring(0, 8)
  $outF = Join-Path $root "add-$id-$tag.out.txt"
  $errF = Join-Path $root "add-$id-$tag.err.txt"
  # Build a single argument string carefully for Start-Process (quote paths/spaces).
  function Q([string]$s) {
    if ($s -match '[\s"]') { return '"' + ($s -replace '"', '\"') + '"' }
    return $s
  }
  $parts = @(
    (Q '--home'), (Q $HubHome),
    'agent', 'add', (Q $id),
    '--type', 'stdio',
    '--command', (Q $cmd)
  )
  foreach ($a in $CmdArgs) {
    $parts += @('--args', (Q $a))
  }
  $argLine = ($parts -join ' ')

  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process -FilePath $Bin -ArgumentList $argLine -NoNewWindow -PassThru `
    -RedirectStandardOutput $outF -RedirectStandardError $errF
  $exited = $p.WaitForExit($budgetSec * 1000)
  $timedOut = -not $exited
  if ($timedOut) {
    # Do not wait on Kill tree forever; force pid then continue measuring.
    try { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } catch {}
    try { $p.Kill() } catch {}
    Start-Sleep -Milliseconds 300
  }
  $sw.Stop()
  $stdout = if (Test-Path $outF) { Get-Content -LiteralPath $outF -Raw -ErrorAction SilentlyContinue } else { '' }
  $stderr = if (Test-Path $errF) { Get-Content -LiteralPath $errF -Raw -ErrorAction SilentlyContinue } else { '' }

  $code = $null
  if (-not $timedOut -and $p.HasExited) { $code = $p.ExitCode }

  return [pscustomobject]@{
    wall_ms     = [int]$sw.ElapsedMilliseconds
    timed_out   = $timedOut
    exit_code   = $code
    stdout      = $stdout
    stderr      = $stderr
    stdout_path = $outF
    stderr_path = $errF
  }
}

function Get-Class($result, [bool]$listed, [bool]$inJson, [int]$softMs) {
  $written = $listed -or $inJson
  if ($result.timed_out) {
    if ($written) { return 'HANG_BUT_WRITTEN' }
    return 'HANG'
  }
  $code = $result.exit_code
  if ($code -eq 0) {
    if (-not $written) { return 'RETURNED_BUT_INVISIBLE' }
    if ($result.wall_ms -gt $softMs) { return 'OK_SLOW' }
    return 'OK'
  }
  if ($written) { return 'FAILED_BUT_WRITTEN' }
  return 'FAILED_CLEAN'
}

function Invoke-Round([string]$label) {
  Write-Human ""
  Write-Human "=== ROUND: $label ==="
  $add = Invoke-AgentAdd -HubHome $hubHome -id $AgentId -cmd $Command -CmdArgs $ArgList -budgetSec $BudgetSec
  $listed = Test-AgentListed -HubHome $hubHome -id $AgentId
  $inJson = Test-AgentsJsonHasId -HubHome $hubHome -id $AgentId
  $softMs = $SoftSec * 1000
  $class = Get-Class -result $add -listed $listed -inJson $inJson -softMs $softMs

  $row = [ordered]@{
    label              = $label
    wall_ms            = $add.wall_ms
    budget_sec         = $BudgetSec
    soft_sec           = $SoftSec
    timed_out          = $add.timed_out
    exit_code          = $add.exit_code
    listed_after       = $listed
    agents_json_has_id = $inJson
    class              = $class
  }

  Write-Human ("wall_ms={0} timed_out={1} exit_code={2} listed_after={3} agents_json_has_id={4} class={5}" -f `
      $row.wall_ms, $row.timed_out, $row.exit_code, $row.listed_after, $row.agents_json_has_id, $row.class)
  return [pscustomobject]$row
}

# --- preflight ---
Write-Human "ACP Hub cold agent-add hang probe (outer-user / code-blind)"
Write-Human "bin=$Bin budget_sec=$BudgetSec soft_sec=$SoftSec agent_id=$AgentId"
Write-Human "home=$hubHome"
Write-Human "command=$Command args=$($ArgList -join ' ')"

try {
  $ver = & $Bin --version 2>&1 | Out-String
  Write-Human ("version: " + $ver.Trim())
} catch {
  Write-Human "FATAL: cannot run ACP_HUB_BIN=$Bin"
  exit 2
}

$cmdOk = Get-Command $Command -ErrorAction SilentlyContinue
if (-not $cmdOk) {
  Write-Human "FATAL: PROBE_COMMAND not found on PATH: $Command"
  exit 2
}

$rounds = @()
$rounds += Invoke-Round -label 'cold'
for ($i = 1; $i -le $WarmRounds; $i++) {
  $rounds += Invoke-Round -label ("warm_$i")
}

$cold = $rounds | Where-Object { $_.label -eq 'cold' } | Select-Object -First 1
$pass = $cold -and ($cold.class -eq 'OK' -or $cold.class -eq 'OK_SLOW')

$summary = [ordered]@{
  probe        = 'cold-agent-add-hang'
  perspective  = 'PRD/QA/outer-user code-blind'
  home         = $hubHome
  bin          = $Bin
  budget_sec   = $BudgetSec
  soft_sec     = $SoftSec
  agent_id     = $AgentId
  rounds       = @($rounds | ForEach-Object {
      [ordered]@{
        label              = $_.label
        wall_ms            = $_.wall_ms
        timed_out          = $_.timed_out
        exit_code          = $_.exit_code
        listed_after       = $_.listed_after
        agents_json_has_id = $_.agents_json_has_id
        class              = $_.class
      }
    })
  cold_class   = $cold.class
  cold_wall_ms = $cold.wall_ms
  gate_pass    = [bool]$pass
  gate_rule    = 'cold class must be OK or OK_SLOW; HANG* is fail'
}

Write-Human ""
Write-Human "=== SUMMARY JSON ==="
$json = ($summary | ConvertTo-Json -Depth 6 -Compress)
Write-Host $json

$summaryPath = Join-Path $root 'summary.json'
$summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $summaryPath -Encoding utf8
Write-Human "summary_file=$summaryPath"

if ($pass) {
  Write-Human "GATE: PASS"
  exit 0
}

Write-Human "GATE: FAIL (cold class=$($cold.class))"
exit 1
