# Security Policy

## Supported versions

Only the latest patch of the newest published minor line receives security
fixes. Older minor lines are unsupported after a newer stable minor is
published; prereleases do not end support for the latest stable line.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security-sensitive reports.

Report privately via one of:

1. GitHub **Security Advisories** on this repository:  
   `https://github.com/RatmmmhSquishyRat/acp-hub/security/advisories/new`
2. Email the maintainer listed on the crates.io owner account for `acp-hub-core` / `acp-hub-cli`.

Include:

- Affected version(s) and install method (binary release vs crates.io vs source)
- Platform (OS / arch)
- Reproduction steps or PoC
- Impact assessment (data disclosure, local privilege, RCE via agent, etc.)

You can expect an acknowledgement within **7 days**. Coordinated disclosure is preferred.

## Trust boundaries (product model)

`acp-hub` is a **local** daemon that:

- Spawns or connects to user-configured ACP agents (arbitrary local commands)
- Stores conversation projections under the Hub home directory
- Speaks JSON-RPC over a local socket / named pipe

Threat model assumptions:

- The machine user is trusted; agents registered in `agents.json` run with the same privileges as the Hub process.
- Network-facing exposure is out of scope unless you deliberately put the process on a shared host or tunnel the socket.
- Treat agent stdout/logs and Hub DB contents as sensitive if prompts contain secrets.

## Supply chain

- Release binaries are built in GitHub Actions from tagged commits and published with SHA-256 digests.
- Library/CLI crates are published to crates.io; verify versions on `https://crates.io/crates/acp-hub-cli`.
- Dependencies are scanned in CI via `cargo-deny` (advisories + licenses).
