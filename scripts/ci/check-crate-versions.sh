#!/usr/bin/env bash
# Fail if publishable crate versions are not kept in lockstep.
# Used by CI and release so tag/publish cannot ship mismatched versions.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
hub_toml="$root/crates/hub/Cargo.toml"
cli_toml="$root/crates/cli/Cargo.toml"

hub_ver="$(sed -n 's/^version = "\(.*\)"/\1/p' "$hub_toml" | head -1)"
cli_ver="$(sed -n 's/^version = "\(.*\)"/\1/p' "$cli_toml" | head -1)"
# path dependency version on acp-hub-core inside the CLI package
dep_ver="$(
  sed -n 's/^acp-hub-core = { path = "\.\.\/hub", version = "\(.*\)" }/\1/p' "$cli_toml" | head -1
)"

echo "acp-hub-core version:     $hub_ver"
echo "acp-hub-cli version:      $cli_ver"
echo "cli → core dep version:   $dep_ver"

if [[ -z "$hub_ver" || -z "$cli_ver" || -z "$dep_ver" ]]; then
  echo "error: failed to parse one or more versions" >&2
  exit 1
fi

if [[ "$hub_ver" != "$cli_ver" ]]; then
  echo "error: hub and cli package versions differ ($hub_ver vs $cli_ver)" >&2
  exit 1
fi

if [[ "$hub_ver" != "$dep_ver" ]]; then
  echo "error: cli path-dep version on acp-hub-core ($dep_ver) != hub package version ($hub_ver)" >&2
  exit 1
fi

if [[ -n "${GITHUB_REF_NAME:-}" && "${GITHUB_REF_TYPE:-}" == "tag" ]]; then
  tag="${GITHUB_REF_NAME#v}"
  echo "git tag (stripped v):     $tag"
  if [[ "$tag" != "$hub_ver" ]]; then
    echo "error: tag v$tag does not match package version $hub_ver" >&2
    exit 1
  fi
fi

echo "version check OK"
