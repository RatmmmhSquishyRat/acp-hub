#!/usr/bin/env bash
# Idempotent crates.io publish for a workspace package.
# Skips when the exact version is already on the registry (safe re-runs).
set -euo pipefail

pkg="${1:?usage: publish-crate.sh <package-name>}"
ver="${2:?usage: publish-crate.sh <package-name> <version>}"

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  echo "error: CARGO_REGISTRY_TOKEN is not set" >&2
  exit 1
fi

already_published() {
  # Prefer crates.io HTTP API (stable), fall back to cargo search.
  if curl -fsSL -A "acp-hub-release" \
    "https://crates.io/api/v1/crates/${pkg}/${ver}" >/dev/null 2>&1; then
    return 0
  fi
  if cargo search "$pkg" --limit 10 2>/dev/null | grep -Eq "^${pkg} = \"${ver}\""; then
    return 0
  fi
  return 1
}

if already_published; then
  echo "${pkg} ${ver} already on crates.io — skipping publish (idempotent)"
  exit 0
fi

echo "publishing ${pkg} ${ver}..."
cargo publish -p "$pkg" --locked
echo "published ${pkg} ${ver}"
