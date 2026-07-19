# acp-hub cheat sheet

```
acp-hub [--home DIR] <cmd>
acp-hub agent add <id> --command <bin> --args ...
acp-hub agent list|inspect|remove|auth|logout|sessions
acp-hub conv create <agent> [--json] [--cwd PATH] [--agent-session-id SID]
acp-hub conv list|show|close
acp-hub conv delete <conv> [--local-only]
acp-hub send <conv> --text "..."
command-producing-prompt | acp-hub send <conv> --stdin
acp-hub param list|set   acp-hub mode list|set
acp-hub cancel <conv>    acp-hub search <q> [--json]
acp-hub proxy add|list|remove
acp-hub mcp | serve
```

`send` and `search` are top-level commands. `agent sessions` has no `--import`.

Home: `$ACP_HUB_HOME` or `~/.acp-hub`. Daemon auto-starts. Prefer `--json` for
machine parsing. Keep the same explicit `--home` on every command in a workflow.
