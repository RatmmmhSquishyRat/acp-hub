# acp-hub cheat sheet

```
acp-hub [--home DIR] <cmd>
acp-hub agent add <id> --command <bin> --args ...
acp-hub agent list|inspect|remove|auth|logout|sessions
acp-hub conv create <agent> [--json] [--cwd PATH] [--agent-session-id SID]
acp-hub conv list|show|close|delete [--local-only]
acp-hub send <conv> --text "..." | --stdin
acp-hub param list|set   acp-hub mode list|set
acp-hub cancel <conv>    acp-hub search <q> [--json]
acp-hub proxy add|list|remove
acp-hub mcp | serve
```

Home: `$ACP_HUB_HOME` or `~/.acp-hub`. Daemon auto-starts. Prefer `--json` for agents.
