# Connecting Claude Code to Redline — Integration Cookbook

Three patterns for wiring AI agents to the redline MCP server. Each pattern is production-ready and copy-pasteable.

---

## When to use which

| Pattern | Best for | Latency | Setup effort |
|---------|----------|---------|--------------|
| **A — Pure MCP tool calls** | Interactive, turn-based sessions where the human drives each step. Claude acts when prompted. | On-demand | Low — add an allowlist entry, done. |
| **B — `/loop` polling** | Event-driven autonomy: Claude watches the canvas and reacts to human moves (games, pair-drawing, live annotation). | 3–30 s round-trip | Low — one `/loop` prompt starts it. |
| **C — HTTP fallback** | Environments with no MCP-aware host: shell scripts, bots, CI agents. Uses raw `curl` against the HTTP transport. | On-demand | Medium — craft JSON-RPC by hand or via a helper script. |

All three patterns assume redline is already running.

---

## Discovery — finding the server

When redline starts it writes a discovery file:

```
~/.local/share/redline/mcp.json
```

Contents look like:

```json
{
  "url": "http://127.0.0.1:9275",
  "port": 9275,
  "pid": 12345,
  "token": null
}
```

The default port is **9275**. If that port was taken at startup, the OS assigns another and the discovery file reflects the actual port.

A project-level `.mcp.json` points Claude Code at the server:

```json
{
  "mcpServers": {
    "redline": {
      "type": "http",
      "url": "http://127.0.0.1:9275/mcp"
    }
  }
}
```

If redline started on a different port (discoverable from the file above), update the `url` field accordingly, or start redline before opening Claude Code so the file reflects the correct port when Claude reads it.

### Ensuring redline is running

Start redline (`redline`, or `cargo run --release`) and confirm the discovery file exists and its `pid` is alive before connecting a client.

---

## Pattern A — Pure MCP tool calls

Claude Code has the `redline` MCP server loaded and the user drives each step with a prompt: "draw a tree", "snapshot the canvas", "undo your last move". Claude calls MCP tools and returns results synchronously.

**Suitable for:** pair-drawing sessions, interactive demos, step-by-step annotation workflows.

### Allowlist

Add the following to `~/.claude/settings.json` (global) or `<project>/.claude/settings.json` (project-scoped):

```json
{
  "permissions": {
    "allow": [
      "mcp__redline__list_rooms",
      "mcp__redline__list_shapes",
      "mcp__redline__changes_since",
      "mcp__redline__snapshot_png",
      "mcp__redline__join_room",
      "mcp__redline__draw_pen",
      "mcp__redline__draw_rect",
      "mcp__redline__draw_arrow",
      "mcp__redline__draw_text",
      "mcp__redline__delete_shape",
      "mcp__redline__move_shape",
      "mcp__redline__resize_shape",
      "mcp__redline__set_prop",
      "mcp__redline__set_text_content",
      "mcp__redline__replace_pen_points",
      "mcp__redline__bring_to_front",
      "mcp__redline__send_to_back",
      "mcp__redline__reorder_to",
      "mcp__redline__renumber_steps",
      "mcp__redline__undo",
      "mcp__redline__redo"
    ]
  }
}
```

All mutating tools are included because the agent's purpose is to modify the canvas. With these entries Claude will not prompt for permission on each call.

### Example prompts

```
# Turn-based annotation
"Join the first available room and draw a labelled architecture diagram
 with boxes for 'API', 'DB', and 'Cache' connected by arrows."

# Inspection
"List all shapes on the canvas and tell me which ones have step numbers."

# Edit
"Move every shape whose label contains 'TODO' 100 pixels to the right."

# Snapshot
"Take a PNG snapshot of the canvas and describe what you see."
```

### How it works

1. Claude calls `list_rooms` to find available rooms.
2. Claude calls `join_room` with the chosen room ID to establish its actor identity.
3. Claude uses drawing and editing tools freely, calling `undo`/`redo` to correct mistakes.
4. `snapshot_png` returns a base64-encoded PNG; Claude can describe or embed it.

---

## Pattern B — `/loop` polling with `changes_since`

Claude self-paces using the superpowers `/loop` skill. Each tick it calls `changes_since` and reacts only when a new event has arrived. This is the foundation for game-like interactions (tic-tac-toe, collaborative drawing, live diagramming).

**Suitable for:** two-player games on the canvas, live reaction to human drawing, ongoing autonomous annotation.

### Starting the loop

```
/loop watch redline for new shapes drawn by HUMAN_LOCAL; when you see one,
evaluate the canvas and respond with a counter-move using draw_text or
draw_rect at the appropriate position
```

The `/loop` skill uses dynamic pacing — Claude decides when to wait before the next tick. For fast interactions (games) it will self-pace at 3–5 seconds; for long-form sessions it backs off to 30 seconds or more.

To enforce a fixed interval:

```
/loop 3s poll redline changes_since and react to any new human shape
```

### Debounce strategy — tracking `seq`

`changes_since(room_id, since)` returns events with `seq > since`. Keep a variable `last_seen_seq` initialized to 0. On each tick:

```python
# Pseudocode — Claude does this in its working memory across loop ticks
events = changes_since(room_id=ROOM_ID, since=last_seen_seq)
if events:
    new_events = [e for e in events if e["seq"] > last_seen_seq]
    last_seen_seq = max(e["seq"] for e in new_events)
    react_to(new_events)
# else: nothing new, skip this tick
```

Never react to `seq <= last_seen_seq`; those are replays you have already processed.

### Interval guidance

| Use case | Recommended interval |
|----------|---------------------|
| Turn-based game (tic-tac-toe, chess diagrams) | 3 000–5 000 ms |
| Live collaborative annotation | 5 000–10 000 ms |
| Long-form drawing session | 30 000 ms |

### Worked example — tic-tac-toe loop

```
/loop 3s
Poll the redline canvas for new shapes. Setup:
  - ROOM_ID: <paste room ID from list_rooms>
  - My actor is "CLAUDE"; human actor is "HUMAN_LOCAL"
  - Track last_seen_seq across ticks, starting at 0.

Each tick:
  1. Call changes_since(room_id=ROOM_ID, since=last_seen_seq).
  2. If no new events, do nothing.
  3. If new events exist, update last_seen_seq to the highest seq seen.
  4. If any new shape was drawn by HUMAN_LOCAL, evaluate the 3x3 grid
     (use list_shapes to get current positions) and place an "X" using
     draw_text at the center of an empty cell.
  5. If you detect a win or draw, draw_text "GAME OVER" at [400, 300]
     and stop the loop.
```

### Stopping the loop

To stop, either:
- Tell Claude "stop the loop" during the next idle window.
- Press `Ctrl+C` in the terminal running Claude Code.

---

## Pattern C — HTTP fallback (curl)

For environments without an MCP-aware host — shell scripts, CI pipelines, bots running outside Claude Code — the redline server speaks standard JSON-RPC 2.0 over HTTP. Use `curl` to drive the full flow.

**Suitable for:** automation scripts, bots, environments where the MCP client SDK is unavailable.

### Wire protocol

The server implements the MCP Streamable HTTP transport (`2025-03-26`). Each call is a `POST /mcp` with a `Content-Type: application/json` body. The session ID is returned as the `mcp-session-id` response header on `initialize` and must be echoed back on every subsequent request.

### Full flow

```bash
#!/usr/bin/env bash
set -euo pipefail

BASE_URL="http://127.0.0.1:9275/mcp"

# ── 1. Initialize a session ───────────────────────────────────────────────────
curl -sN -X POST "$BASE_URL" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{
    "jsonrpc": "2.0",
    "method": "initialize",
    "params": {
      "protocolVersion": "2025-03-26",
      "capabilities": {},
      "clientInfo": {"name": "bash-bot", "version": "0.0.1"}
    },
    "id": 1
  }' \
  -D /tmp/redline-headers.txt \
  -o /tmp/redline-init.json

SESSION_ID=$(grep -i '^mcp-session-id:' /tmp/redline-headers.txt \
             | sed 's/.*: //; s/\r//')

echo "Session: $SESSION_ID"

# ── 2. Send initialized notification ─────────────────────────────────────────
curl -s -X POST "$BASE_URL" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "mcp-session-id: $SESSION_ID" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}'

# ── 3. List rooms ─────────────────────────────────────────────────────────────
ROOMS=$(curl -s -X POST "$BASE_URL" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "mcp-session-id: $SESSION_ID" \
  -d '{
    "jsonrpc": "2.0",
    "method": "tools/call",
    "params": {"name": "list_rooms", "arguments": {}},
    "id": 2
  }')

echo "$ROOMS" | python3 -m json.tool
ROOM_ID=$(echo "$ROOMS" \
  | python3 -c "import json,sys; \
    data=json.load(sys.stdin); \
    rooms=json.loads(data['result']['content'][0]['text']); \
    print(rooms[0]['room_id'])")

echo "Room: $ROOM_ID"

# ── 4. Join room ──────────────────────────────────────────────────────────────
curl -s -X POST "$BASE_URL" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "mcp-session-id: $SESSION_ID" \
  -d "{
    \"jsonrpc\": \"2.0\",
    \"method\": \"tools/call\",
    \"params\": {\"name\": \"join_room\", \"arguments\": {\"room_id\": \"$ROOM_ID\"}},
    \"id\": 3
  }"

# ── 5. Draw a rectangle ───────────────────────────────────────────────────────
curl -s -X POST "$BASE_URL" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "mcp-session-id: $SESSION_ID" \
  -d "{
    \"jsonrpc\": \"2.0\",
    \"method\": \"tools/call\",
    \"params\": {
      \"name\": \"draw_rect\",
      \"arguments\": {
        \"room_id\": \"$ROOM_ID\",
        \"x_min\": 100, \"y_min\": 100,
        \"x_max\": 300, \"y_max\": 200,
        \"color\": \"#ff0000\",
        \"thickness\": 2.0
      }
    },
    \"id\": 4
  }"

# ── 6. Snapshot the canvas ────────────────────────────────────────────────────
curl -s -X POST "$BASE_URL" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "mcp-session-id: $SESSION_ID" \
  -d "{
    \"jsonrpc\": \"2.0\",
    \"method\": \"tools/call\",
    \"params\": {\"name\": \"snapshot_png\", \"arguments\": {\"room_id\": \"$ROOM_ID\"}},
    \"id\": 5
  }" | python3 -c "
import json, sys, base64
data = json.load(sys.stdin)
b64 = data['result']['content'][0]['text']
with open('/tmp/redline-snapshot.png', 'wb') as f:
    f.write(base64.b64decode(b64))
print('Saved to /tmp/redline-snapshot.png')
"
```

### Reading the discovery file in scripts

```bash
PORT=$(python3 -c '
import json
info = json.load(open("~/.local/share/redline/mcp.json"))
print(info["port"])
' 2>/dev/null) || { echo "redline not running"; exit 1; }

BASE_URL="http://127.0.0.1:${PORT}/mcp"
```

### Allowlist for Claude Code Bash tool

When running curl-based automation from inside a Claude Code session (via the Bash tool), use this narrow allowlist:

```json
{
  "permissions": {
    "allow": [
      "Bash(curl * http://127.0.0.1:9275/*)"
    ]
  }
}
```

**Security note:** This pattern matches `curl` only when the URL begins with `http://127.0.0.1:9275/`. It does not grant `curl` access to arbitrary hosts. If redline is running on a different port, replace `9275` with the actual port, or read it dynamically:

```json
"Bash(curl * http://127.0.0.1:*)"
```

(Still loopback-only; still more permissive than the single-port variant — prefer the single-port form when the port is stable.)

---

## Troubleshooting

### "Session not found" from the server

The session was evicted. The server keeps sessions alive for 1 hour of inactivity (`keep_alive = 3600 s` as of Phase 4.5). If you are polling at an interval longer than 1 hour, either:
- Shorten the polling interval, or
- Re-run the `initialize` → `notifications/initialized` handshake to obtain a fresh session ID.

If you see this immediately after connecting, check that you are echoing the exact `mcp-session-id` header value returned by `initialize` on every subsequent request (no trailing `\r` characters — use `sed 's/\r//'`).

### Agent not receiving canvas change notifications

Pattern A and B rely on the server's broadcast channel. In the production binary that channel is always wired: `App::update` advances canvas state, which fires `canvas.event_broadcast`. If you are running a test binary or a custom harness that does not call `App::update`, no events will flow. Use the full release binary (`cargo run --release --features agent-server` or the pre-built `target/release/redline`).

### Tools not visible in Claude Code after server starts

The MCP tool list is negotiated at session startup. If Claude Code connected before redline was up, the tool list will be empty. Fix:
- Run `/mcp` and reconnect the `redline` server, **or**
- Start a fresh Claude Code session in the project directory after redline is running.

### The port in `.mcp.json` does not match the discovery file

A project `.mcp.json` like the one above hard-codes port `9275`. If another process held that port, redline binds to an OS-assigned port and the discovery file will differ. Either:
- Stop the port-occupying process and restart redline, or
- Update `.mcp.json` to match the port in `~/.local/share/redline/mcp.json`.

### `snapshot_png` returns an empty or tiny image

The canvas render runs on the main thread. If the redline window has never been shown or has been minimised on a headless display, the framebuffer may be blank. Bring the window to the foreground, or use a virtual framebuffer (`Xvfb`).

### `changes_since` returns no events even though shapes exist

`changes_since(room_id, since=0)` replays all recorded events from the beginning. If it returns nothing, the room may be empty or `room_id` is incorrect. Call `list_rooms` to confirm the room ID, then `list_shapes` to confirm shapes are present.
