#!/usr/bin/env bash
# Prove that the packaged core exposes ACP SDK types from the same crates.io
# release line an external consumer resolves. Workspace patches are absent.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

version="$(
  sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/hub/Cargo.toml" | head -1
)"
acp_requirement="$(
  sed -n 's/^agent-client-protocol = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1
)"
conductor_requirement="$(
  sed -n 's/^agent-client-protocol-conductor = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1
)"
if [[ -z "$version" || -z "$acp_requirement" || -z "$conductor_requirement" ]]; then
  echo "error: failed to resolve package or ACP SDK version" >&2
  exit 1
fi
if [[ ! "$acp_requirement" =~ ^=[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "error: ACP SDK dependency must use an exact requirement, got $acp_requirement" >&2
  exit 1
fi
if [[ "$conductor_requirement" != "$acp_requirement" ]]; then
  echo "error: ACP SDK and conductor requirements differ" >&2
  exit 1
fi
acp_version="${acp_requirement#=}"

(
  cd "$root"
  cargo package -p acp-hub-core --allow-dirty --locked
)
archive="$root/target/package/acp-hub-core-${version}.crate"
test -f "$archive"
mkdir -p "$tmp/package" "$tmp/consumer/src"
tar -xzf "$archive" -C "$tmp/package"
package_dir="$tmp/package/acp-hub-core-${version}"
packaged_manifest="$package_dir/Cargo.toml"

packaged_dependency_requirement() {
  local dependency="$1"
  awk -v section="[dependencies.${dependency}]" '
    $0 == section { in_dependency = 1; next }
    in_dependency && /^\[/ { exit }
    in_dependency && /^version = "/ {
      sub(/^version = "/, "")
      sub(/"$/, "")
      print
      exit
    }
  ' "$packaged_manifest"
}

for dependency in agent-client-protocol agent-client-protocol-conductor; do
  packaged_requirement="$(packaged_dependency_requirement "$dependency")"
  if [[ "$packaged_requirement" != "$acp_requirement" ]]; then
    echo "error: packaged $dependency requirement is $packaged_requirement, expected $acp_requirement" >&2
    exit 1
  fi
done

cat > "$tmp/consumer/Cargo.toml" <<EOF
[package]
name = "acp-hub-public-api-consumer"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
acp-hub-core = { path = "${package_dir//\\//}" }
agent-client-protocol = "=${acp_version}"
EOF

cat > "$tmp/consumer/src/main.rs" <<'EOF'
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
        cwd: Some(PathBuf::from("/absolute/fixture")),
        agent_session_id: None,
        mcp_servers,
        additional_directories: Vec::new(),
    };
}
EOF

CARGO_TARGET_DIR="$tmp/target" cargo generate-lockfile \
  --manifest-path "$tmp/consumer/Cargo.toml"
CARGO_TARGET_DIR="$tmp/target" cargo check \
  --manifest-path "$tmp/consumer/Cargo.toml" \
  --locked
