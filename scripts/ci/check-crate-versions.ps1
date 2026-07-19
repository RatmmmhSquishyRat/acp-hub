# Fail if publishable crate versions are not kept in lockstep.
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "../..")
$hubToml = Join-Path $root "crates/hub/Cargo.toml"
$cliToml = Join-Path $root "crates/cli/Cargo.toml"

function Get-PackageVersion([string]$path) {
    $m = Select-String -Path $path -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if (-not $m) { throw "no package version in $path" }
    return $m.Matches[0].Groups[1].Value
}

$hubVer = Get-PackageVersion $hubToml
$cliVer = Get-PackageVersion $cliToml
$depLine = Select-String -Path $cliToml -Pattern 'acp-hub-core = \{ path = "\.\./hub", version = "([^"]+)" \}' | Select-Object -First 1
if (-not $depLine) { throw "no acp-hub-core path+version dep in $cliToml" }
$depVer = $depLine.Matches[0].Groups[1].Value

Write-Host "acp-hub-core version:     $hubVer"
Write-Host "acp-hub-cli version:      $cliVer"
Write-Host "cli -> core dep version:  $depVer"

if ($hubVer -ne $cliVer) { throw "hub and cli package versions differ ($hubVer vs $cliVer)" }
if ($hubVer -ne $depVer) { throw "cli path-dep version on acp-hub-core ($depVer) != hub package version ($hubVer)" }

if ($env:GITHUB_REF_TYPE -eq "tag" -and $env:GITHUB_REF_NAME) {
    $tag = $env:GITHUB_REF_NAME
    if ($tag.StartsWith("v")) { $tag = $tag.Substring(1) }
    Write-Host "git tag (stripped v):     $tag"
    if ($tag -ne $hubVer) { throw "tag v$tag does not match package version $hubVer" }
}

Write-Host "version check OK"
