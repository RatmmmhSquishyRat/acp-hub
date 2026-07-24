# Oh My Pi through ACP

Oh My Pi exposes a native stdio ACP endpoint:

```sh
omp acp
```

This directory contains a registration sample only. It does not read Oh My Pi
session files and does not add a repository-local proxy.

## Prerequisites

1. Install and authenticate Oh My Pi.
2. Install ACP Hub.
3. Verify the installed commands:

   ```sh
   omp --version
   omp acp --help
   acp-hub --version
   ```

The native endpoint owns its advertised models, authentication, session
behavior, modes, configuration, and tool semantics. Recheck those capabilities
after an Oh My Pi upgrade instead of copying a version-specific model or
session assumption into the Hub registry.

## Register

```sh
acp-hub agent add omp --type stdio --command omp --args acp
```

The complete [agents.json](agents.json) sample uses the same command and defaults
to **local trusted use** (`auto-allow`, filesystem + terminal on). Those settings
govern **Hub client callbacks** requested over ACP; they do not claim that every
tool internal to the vendor endpoint is disabled. Use `--sandbox` (or explicit
reject / allow=false flags) when you need a tight registration.

## Use

POSIX shell:

```sh
hub_home=/absolute/path/to/isolated-hub-home
conv="$(acp-hub --home "$hub_home" conv create omp --cwd "$PWD")"
acp-hub --home "$hub_home" send "$conv" --text "Hello"
acp-hub --home "$hub_home" conv show "$conv"
```

PowerShell:

```powershell
$hubHome = Join-Path $env:TEMP 'acp-hub-omp'
$conv = (acp-hub --home $hubHome conv create omp --cwd (Get-Location).Path).Trim()
'Hello' | acp-hub --home $hubHome send $conv --stdin
acp-hub --home $hubHome conv show $conv
```

Use `acp-hub param list <conv-id>` and `acp-hub mode list <conv-id>` to inspect
what the connected endpoint actually advertises. Installed-vendor session
restore and destructive tool behavior remain live compatibility checks; the
registry sample alone does not establish them.
