# ACP Hub — BDD (Behavior-Driven Development)

> Grounded in: `doc/dev/spec.md` + `doc/ssot/pillars/README.md`

## Feature 1: Register ACP Agent Endpoints (Spec 1, design 1)

### Scenario: Register a stdio agent
```gherkin
Given no agents are registered
When I run "acp-hub agent add omp --command omp --args acp"
Then "omp" appears in "agent list"
And agents.json contains an acpAgents entry with command="omp" args=["acp"]
```

### Scenario: Register an HTTP agent
```gherkin
Given no agents are registered
When I run "acp-hub agent add remote --type http --url https://agent.example.com/acp"
Then "remote" appears in "agent list"
```


### Scenario: Register a WebSocket agent
```gherkin
When I run "acp-hub agent add realtime --type ws --url ws://localhost:8080/acp"
Then "realtime" appears in "agent list"
```
### Scenario: Remove an agent
```gherkin
Given agent "omp" is registered
When I run "acp-hub agent remove omp"
Then "omp" does not appear in "agent list"
```

### Scenario: Reject invalid id
```gherkin
When I run "acp-hub agent add 'bad id!' --command foo"
Then the command fails with "invalid agent id"
```

## Feature 2: Two-Layer Conversation Data (FAQ lines 36-40)

### Scenario: Delete a conversation
```gherkin
Given conversation "conv-1" exists with agent "omp"
When I run "acp-hub conv delete conv-1"
Then the conversation is removed from the projection
And if the agent supports session/delete, the agent-side session is also deleted
```

### Scenario: Delete conversation local-only
```gherkin
Given conversation "conv-1" exists
When I run "acp-hub conv delete conv-1 --local-only"
Then the Hub projection is deleted
And the agent-side session is NOT deleted
```

### Scenario: List agent-side sessions (Layer 1 discovery)
```gherkin
Given agent "cursor" is registered and supports session/list
When I run "acp-hub agent sessions cursor"
Then I see all sessions the cursor agent knows about
And each session is auto-imported into the Hub projection

### Scenario: Discover pre-existing agent-side session not created by Hub
```gherkin
Given agent "cursor" has sessions created OUTSIDE the Hub (e.g., via Cursor IDE)
And agent "cursor" supports session/list
When I run "acp-hub agent sessions cursor"
Then I see ALL sessions including those NOT created by the Hub
And each discovered session is imported into the projection
And their messages are loaded via session/load (if supported)
```

### Scenario: View agent original messages (Layer 1 content)
```gherkin
Given agent "cursor" supports session/load
And a session "abc-123" exists on the cursor agent with message history
When I run "acp-hub conv create cursor --agent-session-id abc-123"
Then the conversation loads the session via ACP session/load
And the replayed messages are stored with source="load_replay"
And I can view them with "conv show"
```

### Scenario: Both layers displayed independently
```gherkin
Given conversation "conv-1" has messages from both layers:
  | source       | content           |
  | load_replay  | agent original    |
  | local_turn   | hub capture       |
When I run "acp-hub conv show conv-1"
Then I see messages from both sources
And each message is labeled with its source
```

### Scenario: Fallback to Hub capture only
```gherkin
Given agent "basic" does NOT support session/list
And agent "basic" does NOT support session/load
When I run "acp-hub conv list --agent basic"
Then I see only Hub-created conversations
And conv show displays only Hub-captured messages (source="local_turn")
```

## Feature 3: Send Message and Receive Reply (Spec 3)

### Scenario: Send a prompt and receive a streamed response
```gherkin
Given conversation "conv-1" exists with agent "omp"
When I run "acp-hub send conv-1 --text 'Hello'"
Then I see streamed session/update notifications on stdout
And the final response includes a stop_reason
And the user prompt and all agent responses are captured in the Store
```

### Scenario: Send with parameters (Spec 4)
```gherkin
Given conversation "conv-1" exists with agent "omp"
When I run "acp-hub send conv-1 --text 'Plan this' --mode plan --param model=zai/glm-4.5"
Then the prompt is sent with the specified mode and model
And the response reflects the mode/model change
```

### Scenario: Cancel an in-flight turn
```gherkin
Given a prompt is in-flight on conversation "conv-1"
When I run "acp-hub cancel conv-1"
Then a CancelNotification is sent to the agent
And the in-flight prompt resolves with a cancelled stop_reason
```

## Feature 4: Global Search (Spec 2)

### Scenario: Search message content
```gherkin
Given conversation "conv-1" has a captured message containing "hello world"
When I run "acp-hub search 'hello world'"
Then I see a search hit for "conv-1" with a snippet
```

### Scenario: Search conversation titles
```gherkin
Given conversation "conv-1" has title "My Planning Session"
When I run "acp-hub search 'Planning'"
Then I see a conversation-type hit for "conv-1"
```

### Scenario: Search across both layers
```gherkin
Given conversation "conv-1" has:
  | source       | body_text           |
  | load_replay  | "agent original"    |
  | local_turn   | "hub captured data" |
When I run "acp-hub search 'original'"
Then I see the Layer 1 message
When I run "acp-hub search 'captured'"
Then I see the Layer 2 message
```

## Feature 5: Proxy Chains (Spec 5)

### Scenario: Send through a proxy chain
```gherkin
Given agent "agent-x" has proxyChain=["proxy-1"]
And proxy "proxy-1" is a stdio process that prepends "[proxied]" to outbound
When I run "acp-hub send conv-1 --text 'Hello'"
Then the agent receives "[proxied] Hello"
And the response is post-processed by the proxy
And the stored message reflects the post-processed content
```

## Feature 6: Daemon Lifecycle (design 5)

### Scenario: Auto-spawn daemon
```gherkin
Given no daemon is running
When I run any acp-hub command
Then the daemon is spawned automatically
And the command connects to it via JSON-RPC
```

### Scenario: Idle exit
```gherkin
Given the daemon is running with IDLE_TIMEOUT=2
And no clients are connected
And no runs are active
When 2 seconds pass
Then the daemon exits cleanly
And daemon metadata files are removed
```

### Scenario: Singleton enforcement
```gherkin
Given a daemon is already running for home "/tmp/acp-hub"
When another process tries to ensure_daemon with the same home
Then it connects to the existing daemon (does NOT spawn a second one)
```

## Feature 7: MCP Facade (design 5)

### Scenario: MCP tools available
```gherkin
Given the daemon is running
When an MCP client connects to "acp-hub mcp"
Then it can call list_agents, create_conversation, send_message, search, etc.
And the results match the CLI output for equivalent operations
```
