# Prove that the packaged core exposes ACP SDK types from the same crates.io
# release line an external consumer resolves. Workspace patches are absent.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$temp = Join-Path ([System.IO.Path]::GetTempPath()) (
    'acp-hub-public-consumer-' + [Guid]::NewGuid().ToString('N')
)

function Get-FirstVersion([string]$path, [string]$pattern) {
    $match = Select-String -Path $path -Pattern $pattern | Select-Object -First 1
    if (-not $match) { throw "version not found in $path" }
    $match.Matches[0].Groups[1].Value
}

function Write-Utf8NoBom([string]$path, [string]$content) {
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($path, $content, $encoding)
}

function Get-PackagedDependencyVersion([string]$path, [string]$name) {
    $inDependency = $false
    foreach ($line in Get-Content -LiteralPath $path) {
        if ($line -eq "[dependencies.$name]") {
            $inDependency = $true
            continue
        }
        if ($inDependency -and $line.StartsWith('[')) { break }
        if ($inDependency -and $line -match '^version = "([^"]+)"$') {
            return $Matches[1]
        }
    }
    throw "dependency $name version not found in $path"
}

try {
    $version = Get-FirstVersion (Join-Path $root 'crates/hub/Cargo.toml') '^version = "([^"]+)"'
    $acpRequirement = Get-FirstVersion (Join-Path $root 'Cargo.toml') '^agent-client-protocol = "([^"]+)"'
    $conductorRequirement = Get-FirstVersion (Join-Path $root 'Cargo.toml') '^agent-client-protocol-conductor = "([^"]+)"'
    if ($acpRequirement -notmatch '^=[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$') {
        throw "ACP SDK dependency must use an exact requirement, got $acpRequirement"
    }
    if ($conductorRequirement -ne $acpRequirement) {
        throw "ACP SDK and conductor requirements differ ($acpRequirement vs $conductorRequirement)"
    }
    $acpVersion = $acpRequirement.Substring(1)

    Push-Location $root
    try {
        cargo package -p acp-hub-core --allow-dirty --locked
        if ($LASTEXITCODE -ne 0) { throw 'cargo package failed' }
    } finally {
        Pop-Location
    }

    $archive = Join-Path $root "target/package/acp-hub-core-$version.crate"
    if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
        throw "package archive missing: $archive"
    }
    $packageRoot = Join-Path $temp 'package'
    $consumer = Join-Path $temp 'consumer'
    New-Item -ItemType Directory -Path $packageRoot, (Join-Path $consumer 'src') -Force |
        Out-Null
    tar -xzf $archive -C $packageRoot
    if ($LASTEXITCODE -ne 0) { throw 'package extraction failed' }
    $extractedPackageDir = Join-Path $packageRoot "acp-hub-core-$version"
    $packagedManifest = Join-Path $extractedPackageDir 'Cargo.toml'
    foreach ($dependency in @('agent-client-protocol', 'agent-client-protocol-conductor')) {
        $packagedRequirement = Get-PackagedDependencyVersion $packagedManifest $dependency
        if ($packagedRequirement -ne $acpRequirement) {
            throw "packaged $dependency requirement is $packagedRequirement, expected $acpRequirement"
        }
    }
    $packageDir = $extractedPackageDir.Replace('\', '/')

    $consumerToml = @"
[package]
name = "acp-hub-public-api-consumer"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
acp-hub-core = { path = "$packageDir" }
agent-client-protocol = "=$acpVersion"
"@
    Write-Utf8NoBom (Join-Path $consumer 'Cargo.toml') $consumerToml

    $consumerMain = @'
use std::path::PathBuf;

use acp_hub::hub::{CreateConversationParams, SendPromptParams};
use agent_client_protocol::schema::v1::{ContentBlock, McpServer};

fn main() {
    let prompt = ContentBlock::from(String::from("public type identity"));
    let _: SendPromptParams = SendPromptParams {
        conv_id: "fixture".to_string(),
        prompt: vec![prompt],
        params: Vec::new(),
        mode_id: None,
    };
    let mcp_servers: Vec<McpServer> = Vec::new();
    let _: CreateConversationParams = CreateConversationParams {
        agent_id: "fixture".to_string(),
        cwd: Some(PathBuf::from(r"C:\absolute\fixture")),
        agent_session_id: None,
        mcp_servers,
        additional_directories: Vec::new(),
    };
}
'@
    Write-Utf8NoBom (Join-Path $consumer 'src/main.rs') $consumerMain

    $env:CARGO_TARGET_DIR = Join-Path $temp 'target'
    cargo generate-lockfile --manifest-path (Join-Path $consumer 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'external package consumer lock failed' }
    cargo check --manifest-path (Join-Path $consumer 'Cargo.toml') --locked
    if ($LASTEXITCODE -ne 0) { throw 'external package consumer check failed' }
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}
