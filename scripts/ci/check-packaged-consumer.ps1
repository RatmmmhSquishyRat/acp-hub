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

try {
    $version = Get-FirstVersion (Join-Path $root 'crates/hub/Cargo.toml') '^version = "([^"]+)"'
    $acpVersion = Get-FirstVersion (Join-Path $root 'Cargo.toml') '^agent-client-protocol = "([^"]+)"'

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
    $packageDir = (Join-Path $packageRoot "acp-hub-core-$version").Replace('\', '/')

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
