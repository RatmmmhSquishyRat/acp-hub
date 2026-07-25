# Scratch verification — wait mid-poll incremental emit (#57)

Date: 2026-07-25  
Branch: `main` @ `714f6b0` (`fix: restore CLI wait mid-poll incremental emit (#57)`)

## Goal

Restore CLI `wait` V3/G3: print new Store view lines **each poll while run is open**, not only after terminal (regression after #56 batch path).

## Unit / CI

| Check | Result |
|-------|--------|
| `cargo test -p acp-hub-core wait_` | **PASS** 6 tests (incl. `wait_run_emits_views_before_terminal`) |
| `cargo clippy -p acp-hub-core -p acp-hub-cli --all-targets -- -D warnings` | **PASS** |
| PR #57 CI (fmt/clippy/test, linux, macos, MSRV, deny, package) | **PASS** (after rustfmt fix) |
| Merged | **yes** — squash to main |

## Binary smoke (release `acp-hub-cli`)

| Command | Result |
|---------|--------|
| `acp-hub --help` | Four-primitive surface; `send` / `wait` / `show` / `cancel` |
| `acp-hub wait --help` | Attach + stream until terminal; `--run` / `--since-seq` / `--timeout` / `--json` |
| `acp-hub doctor` | UX-CORE surface: send / wait / show / cancel cold-start |
| `acp-hub wait no-such` | `error: conversation_not_found: conversation not found: no-such` (no hang) |

## Contract locked by test

`wait_run_emits_views_before_terminal`:

1. Start `wait_run_with_emit` on a running run.
2. Append assistant message while status is still `running`.
3. Assert `on_new` fired ≥1 **before** `finalize_run_cas`.
4. After terminal, result messages include mid-stream body.

## Notes

- MCP intentionally stays on batch `HubClient::wait_run` (no mid-poll streaming requirement).
- Live multi-process attach against a real agent was not re-run in this scratch; unit emit-before-terminal + green CI is the acceptance bar for the regression fix.
