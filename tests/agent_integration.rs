//! End-to-end integration test for the redline agent server.
//! Requires the `agent-server` feature.

#![cfg(feature = "agent-server")]

use base64::Engine as _;
use redline::agent::{drain_ops, AgentServer};
use redline::state::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Pick a free TCP port on the loopback.
fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Initialize an MCP session over HTTP: `initialize` + `notifications/initialized`.
/// Returns the negotiated session id (if the server set one).
fn init_session(client: &reqwest::blocking::Client, url: &str) -> Option<String> {
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {"resources": {"subscribe": true}},
            "clientInfo": {"name": "test-client", "version": "0.0.1"}
        },
        "id": 1
    });
    let init_resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&init_req)
        .send()
        .expect("initialize request");
    assert!(
        init_resp.status().is_success(),
        "initialize failed: {}",
        init_resp.status()
    );
    let session_id = init_resp
        .headers()
        .get("mcp-session-id")
        .or_else(|| init_resp.headers().get("Mcp-Session-Id"))
        .map(|v| v.to_str().unwrap().to_string());
    eprintln!("[test] session_id: {session_id:?}");
    let _ = init_resp.text();

    let initialized_req =
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&initialized_req);
    if let Some(sid) = &session_id {
        req = req.header("mcp-session-id", sid);
    }
    let _ = req.send().expect("initialized notification").text();

    session_id
}

/// Test harness: start the agent server, run the MCP/HTTP `body` on a worker
/// thread, and drain `AppState` ops on the current (main) thread until the worker
/// completes.
///
/// This mirrors production, where `agent::drain_ops` runs on the egui UI thread
/// (see `App::update`) and `AppState` never crosses a thread boundary. That
/// matters because `AppState` is **not** `Send` under the `lama-webview` feature
/// (it can hold a GTK/`wry` `WebView`, which must stay on the main thread), so we
/// cannot move it into a spawned drainer thread. Instead the blocking HTTP flow —
/// which only touches `Send` data (the client, URL, and session id) — runs on the
/// worker, and the non-`Send` state stays here.
///
/// Returns the worker's value plus the shared state handle so callers can assert
/// on the final `AppState` from the main thread after all ops are drained.
fn run_session<B, R>(tab_label: &str, wire_broadcast: bool, body: B) -> (R, Arc<Mutex<AppState>>)
where
    B: FnOnce(&reqwest::blocking::Client, &str, &Option<String>) -> R + Send,
    R: Send,
{
    let mut state = AppState::default();
    state.tab_label = tab_label.to_string();
    state.canvas.canvas_size = egui::vec2(400.0, 300.0);

    let port = pick_free_port();
    let (server, rx) = AgentServer::start(port);
    state.agent_op_rx = Some(rx);
    if wire_broadcast {
        // Wire the broadcast channel onto the canvas so CommandExecutor::execute
        // can publish ServerEvents. In the real app, App::update does this; in
        // tests we must do it explicitly before the drainer starts.
        state.canvas.event_broadcast = Some(server.event_broadcast.clone());
    }

    // Wait for the server to bind before connecting.
    std::thread::sleep(Duration::from_millis(300));

    let state_handle = Arc::new(Mutex::new(state));
    let url = format!("http://127.0.0.1:{port}/mcp");
    let mut auth_headers = reqwest::header::HeaderMap::new();
    auth_headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", server.token).parse().unwrap(),
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .default_headers(auth_headers)
        .build()
        .expect("build client");
    let session_id = init_session(&client, &url);

    let done = Arc::new(AtomicBool::new(false));

    // `client`, `url`, `session_id`, `done` and `state_handle` are owned by this
    // (run_session) frame, so they outlive the scope and can be borrowed by the
    // worker. `body` is moved into the worker (it's `FnOnce`). The non-`move`
    // outer closure keeps the borrows pointing at this frame's data.
    let result = std::thread::scope(|scope| {
        let client_ref = &client;
        let url_ref: &str = &url;
        let sid_ref = &session_id;
        let done_worker = done.clone();

        let worker = scope.spawn(move || {
            let r = body(client_ref, url_ref, sid_ref);
            done_worker.store(true, Ordering::SeqCst);
            r
        });

        // Main-thread drainer: pump ops until the worker signals completion, with
        // a generous safety deadline so a wedged worker can't hang the test forever.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !done.load(Ordering::SeqCst) {
            {
                let mut st = state_handle.lock().unwrap();
                drain_ops(&mut st, 64);
            }
            if std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        let r = worker.join().expect("worker thread panicked");

        // Flush any ops enqueued right before the worker finished.
        std::thread::sleep(Duration::from_millis(50));
        {
            let mut st = state_handle.lock().unwrap();
            while drain_ops(&mut st, 256) > 0 {}
        }
        r
    });

    // Server shutdown: drop triggers stop, removing the discovery file.
    drop(server);

    (result, state_handle)
}

#[test]
fn agent_can_list_rooms_join_and_draw() {
    let (_, state_handle) = run_session("Smoke Canvas", false, |client, url, sid| {
        // ── list_rooms — should report exactly one room labelled "Smoke Canvas" ──
        let rooms = call_tool_ok(client, url, sid, "list_rooms", &serde_json::json!({}), 2);
        let rooms_arr = rooms.as_array().expect("rooms is array");
        assert_eq!(rooms_arr.len(), 1, "expected exactly 1 room");
        assert_eq!(rooms_arr[0]["label"], "Smoke Canvas", "room label mismatch");
        let room_id = rooms_arr[0]["id"]
            .as_str()
            .expect("room id is string")
            .to_string();
        eprintln!("[test] room_id: {room_id}");

        // ── join_room ────────────────────────────────────────────────────────────
        let join = call_tool_ok(
            client,
            url,
            sid,
            "join_room",
            &serde_json::json!({"room_id": room_id, "name": "test-bot"}),
            3,
        );
        assert!(
            join["participant_id"].is_string(),
            "join_room should return a participant_id, got: {join:#}"
        );

        // ── draw_arrow ─────────────────────────────────────────────────────────────
        let draw = call_tool_ok(
            client,
            url,
            sid,
            "draw_arrow",
            &serde_json::json!({
                "room_id": room_id,
                "start": [10.0, 20.0],
                "end": [100.0, 200.0],
                "color": "#ff0000",
                "thickness": 3.0,
                "style": "Solid",
                "heads": "Single"
            }),
            4,
        );
        assert!(
            draw["shape_id"].is_string(),
            "draw_arrow should return a shape_id string"
        );

        // ── list_shapes — should now have exactly one arrow ──────────────────────
        let shapes = call_tool_ok(
            client,
            url,
            sid,
            "list_shapes",
            &serde_json::json!({"room_id": room_id}),
            5,
        );
        let shapes_arr = shapes.as_array().expect("shapes is array");
        assert_eq!(
            shapes_arr.len(),
            1,
            "expected exactly 1 shape after draw_arrow"
        );
        assert_eq!(
            shapes_arr[0]["kind"], "arrow",
            "shape kind should be 'arrow'"
        );
    });

    // ── Verify the mutation landed in the AppState (drained on the main thread) ──
    let s = state_handle.lock().unwrap();
    assert_eq!(
        s.canvas.annotations.len(),
        1,
        "AppState should have 1 annotation after draw_arrow"
    );
    assert_eq!(
        s.canvas.event_log.len(),
        1,
        "AppState event_log should have 1 entry"
    );
}

#[test]
fn agent_can_read_canvas_resource() {
    run_session("Smoke Canvas", false, |client, url, session_id| {
        // ── resources/list — should list canvas://default ─────────────────────────
        let list_req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "resources/list",
            "params": {},
            "id": 10
        });
        let mut req = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&list_req);
        if let Some(sid) = session_id {
            req = req.header("mcp-session-id", sid);
        }
        let resp = req.send().expect("resources/list request");
        assert!(
            resp.status().is_success(),
            "resources/list failed: {}",
            resp.status()
        );
        let body = parse_mcp_response(resp);
        eprintln!("[test] resources/list response: {body:#}");
        let resources = body["result"]["resources"]
            .as_array()
            .expect("resources array");
        assert_eq!(resources.len(), 1, "expected exactly 1 resource");
        assert_eq!(
            resources[0]["uri"], "canvas://default",
            "resource uri mismatch"
        );

        // ── resources/read — should return a JSON array of shapes ────────────────
        let read_req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "resources/read",
            "params": {"uri": "canvas://default"},
            "id": 11
        });
        let mut req = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&read_req);
        if let Some(sid) = session_id {
            req = req.header("mcp-session-id", sid);
        }
        let resp = req.send().expect("resources/read request");
        assert!(
            resp.status().is_success(),
            "resources/read failed: {}",
            resp.status()
        );
        let body = parse_mcp_response(resp);
        eprintln!("[test] resources/read response: {body:#}");

        let contents = body["result"]["contents"]
            .as_array()
            .expect("contents array");
        assert!(!contents.is_empty(), "expected at least one content item");

        // The text field should be a JSON array (empty since no shapes drawn yet).
        let text = contents[0]["text"].as_str().expect("text field is string");
        let shapes: serde_json::Value =
            serde_json::from_str(text).expect("text should be valid JSON");
        assert!(shapes.is_array(), "shapes JSON should be an array");
        assert_eq!(
            shapes.as_array().unwrap().len(),
            0,
            "expected 0 shapes initially"
        );
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: full Phase 4 round-trip — join, draw, snapshot, move, undo
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn end_to_end_modify_and_undo_round_trip() {
    run_session("Smoke Canvas", false, |client, url, session_id| {
        let room_id = fetch_room_id(client, url, session_id);

        // ── 1. join_room ─────────────────────────────────────────────────────────
        let join_result = call_tool_ok(
            client,
            url,
            session_id,
            "join_room",
            &serde_json::json!({"room_id": room_id, "name": "e2e-tester"}),
            101,
        );
        let participant_id = join_result["participant_id"]
            .as_str()
            .expect("join_room should return participant_id")
            .to_string();
        eprintln!("[test] participant_id: {participant_id}");

        // ── 2. draw_arrow ─────────────────────────────────────────────────────────
        let draw_result = call_tool_ok(
            client,
            url,
            session_id,
            "draw_arrow",
            &serde_json::json!({
                "room_id": room_id,
                "start": [10.0, 10.0],
                "end": [50.0, 50.0],
                "color": "#ff0000",
                "thickness": 2.0,
                "style": "Solid",
                "heads": "Single"
            }),
            102,
        );
        let shape_id = draw_result["shape_id"]
            .as_str()
            .expect("draw_arrow should return shape_id")
            .to_string();
        eprintln!("[test] shape_id: {shape_id}");

        // ── 3. list_shapes before move — capture bounding_rect for comparison ──────
        let before_shapes = call_tool_ok(
            client,
            url,
            session_id,
            "list_shapes",
            &serde_json::json!({"room_id": room_id}),
            103,
        );
        let before_arr = before_shapes.as_array().expect("list_shapes returns array");
        assert_eq!(before_arr.len(), 1, "expected 1 shape before move");
        // bounding_rect is serialized as [min_x, min_y, max_x, max_y]
        let before_min_x = before_arr[0]["bounding_rect"][0]
            .as_f64()
            .expect("bounding_rect[0] is f64");
        eprintln!("[test] before move min_x: {before_min_x}");

        // ── 4. snapshot_png — verify it returns a valid PNG ───────────────────────
        let snap1_body = call_tool_raw(
            client,
            url,
            session_id,
            "snapshot_png",
            &serde_json::json!({"room_id": room_id}),
            104,
        );
        // The snapshot returns Content::image with base64.
        // result.content[0].data holds the base64 string.
        let snap1_b64 = snap1_body["result"]["content"][0]["data"]
            .as_str()
            .expect("snapshot_png should return base64 data in result.content[0].data");
        let snap1_bytes = base64::engine::general_purpose::STANDARD
            .decode(snap1_b64)
            .expect("snap1 should be valid base64");
        assert!(
            snap1_bytes.starts_with(b"\x89PNG"),
            "snapshot PNG magic header missing"
        );
        eprintln!(
            "[test] snap1 PNG size: {} bytes — valid PNG confirmed",
            snap1_bytes.len()
        );

        // ── 5. move_shape ─────────────────────────────────────────────────────────
        let _move_result = call_tool_ok(
            client,
            url,
            session_id,
            "move_shape",
            &serde_json::json!({
                "room_id": room_id,
                "shape_id": shape_id,
                "delta": [20.0, 0.0]
            }),
            105,
        );

        // ── 6. list_shapes after move — bounding_rect should have shifted by 20 ──
        let after_move_shapes = call_tool_ok(
            client,
            url,
            session_id,
            "list_shapes",
            &serde_json::json!({"room_id": room_id}),
            106,
        );
        let after_move_arr = after_move_shapes
            .as_array()
            .expect("list_shapes returns array");
        assert_eq!(
            after_move_arr.len(),
            1,
            "shape should still exist after move"
        );
        let after_min_x = after_move_arr[0]["bounding_rect"][0]
            .as_f64()
            .expect("bounding_rect[0] is f64 after move");
        eprintln!("[test] after move min_x: {after_min_x}");
        assert!(
            (after_min_x - before_min_x - 20.0).abs() < 1.0,
            "min_x should have increased by ~20 after move, before={before_min_x} after={after_min_x}"
        );

        // ── 7. undo — should roll back the move ──────────────────────────────────
        let undo_result = call_tool_ok(
            client,
            url,
            session_id,
            "undo",
            &serde_json::json!({"room_id": room_id}),
            107,
        );
        assert!(
            undo_result["seq"].as_u64().is_some(),
            "undo should return a seq number, got: {undo_result:#}"
        );
        eprintln!("[test] undo seq: {}", undo_result["seq"]);

        // ── 8. list_shapes after undo — position should be restored ──────────────
        let after_undo_shapes = call_tool_ok(
            client,
            url,
            session_id,
            "list_shapes",
            &serde_json::json!({"room_id": room_id}),
            108,
        );
        let after_undo_arr = after_undo_shapes
            .as_array()
            .expect("list_shapes returns array after undo");
        assert_eq!(
            after_undo_arr.len(),
            1,
            "shape should still exist after undo of move"
        );
        let undo_min_x = after_undo_arr[0]["bounding_rect"][0]
            .as_f64()
            .expect("bounding_rect[0] is f64 after undo");
        eprintln!("[test] after undo min_x: {undo_min_x}");
        assert!(
            (undo_min_x - before_min_x).abs() < 1.0,
            "min_x should be restored to original after undo, original={before_min_x} got={undo_min_x}"
        );
        eprintln!("[test] shape after undo: {}", after_undo_arr[0]["kind"]);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: calling draw_arrow before join_room returns a NotJoined error
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn draw_before_join_returns_not_joined_error() {
    run_session("Smoke Canvas", false, |client, url, session_id| {
        let room_id = fetch_room_id(client, url, session_id);

        // Skip join_room. Directly attempt draw_arrow — must fail with NotJoined.
        let body = call_tool_raw(
            client,
            url,
            session_id,
            "draw_arrow",
            &serde_json::json!({
                "room_id": room_id,
                "start": [0.0, 0.0],
                "end": [10.0, 10.0],
                "color": "#ff0000",
                "thickness": 1.0,
                "style": "Solid",
                "heads": "Single"
            }),
            200,
        );

        // rmcp surfaces invalid_params errors as JSON-RPC error objects.
        let err_msg = body["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("expected JSON-RPC error with message, got: {body:#}"));
        assert!(
            err_msg.contains("join_room") || err_msg.to_lowercase().contains("not joined"),
            "expected 'must call join_room' / 'not joined' error, got: {err_msg}"
        );
        eprintln!("[test] correctly rejected draw without join: {err_msg}");
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: resources/subscribe triggers notifications/resources/updated on draw
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn subscribe_receives_push_on_draw() {
    use std::io::BufRead as _;
    use std::sync::mpsc;

    run_session("Push Canvas", true, |client, url, session_id| {
        // ── join_room ────────────────────────────────────────────────────────────
        let room_id = fetch_room_id(client, url, session_id);
        eprintln!("[push-test] room_id: {room_id}");

        let join_result = call_tool_ok(
            client,
            url,
            session_id,
            "join_room",
            &serde_json::json!({"room_id": room_id, "name": "push-test-bot"}),
            50,
        );
        eprintln!("[push-test] join result: {join_result:#}");

        // ── resources/subscribe for canvas://default ──────────────────────────────
        let subscribe_req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "resources/subscribe",
            "params": {"uri": "canvas://default"},
            "id": 51
        });
        let mut req = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&subscribe_req);
        if let Some(sid) = session_id {
            req = req.header("mcp-session-id", sid);
        }
        let sub_resp = req.send().expect("resources/subscribe request");
        assert!(
            sub_resp.status().is_success(),
            "resources/subscribe failed: {}",
            sub_resp.status()
        );
        let sub_body = parse_mcp_response(sub_resp);
        eprintln!("[push-test] subscribe response: {sub_body:#}");
        // subscribe returns an empty result on success (no error field).
        assert!(
            sub_body.get("error").is_none(),
            "resources/subscribe returned an error: {sub_body:#}"
        );

        // ── Open the SSE GET stream on a separate client ─────────────────────────
        // The GET /mcp with Accept: text/event-stream opens the server-push
        // channel where notifications are delivered.
        // Reuse `client` (it carries the bearer token as a default header).
        let mut sse_req = client
            .get(url)
            .timeout(Duration::from_secs(10))
            .header("Accept", "text/event-stream");
        if let Some(sid) = session_id {
            sse_req = sse_req.header("mcp-session-id", sid);
        }
        let sse_resp = sse_req.send().expect("open SSE GET stream");
        eprintln!("[push-test] SSE GET status: {}", sse_resp.status());
        assert!(
            sse_resp.status().is_success(),
            "SSE GET failed: {}",
            sse_resp.status()
        );

        // Spawn a reader thread that scans SSE lines for the notification.
        let (notify_tx, notify_rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(sse_resp);
            for line in reader.lines() {
                let Ok(line) = line else {
                    eprintln!("[push-test-reader] SSE read error, stopping");
                    return;
                };
                eprintln!("[push-test-reader] SSE line: {line:?}");
                if line.contains("notifications/resources/updated") {
                    let _ = notify_tx.send(line);
                    return;
                }
            }
            eprintln!("[push-test-reader] SSE stream ended without notification");
        });

        // Small pause to ensure the subscribe handler's task is wired up before
        // we fire the draw (the tokio task spawned inside subscribe runs async).
        std::thread::sleep(Duration::from_millis(100));

        // ── draw_arrow — should trigger a ServerEvent broadcast ──────────────────
        let draw_body = call_tool_raw(
            client,
            url,
            session_id,
            "draw_arrow",
            &serde_json::json!({
                "room_id": room_id,
                "start": [10.0, 10.0],
                "end": [20.0, 20.0],
                "color": "#ff0000",
                "thickness": 1.0,
                "style": "Solid",
                "heads": "Single"
            }),
            99,
        );
        eprintln!("[push-test] draw_arrow response: {draw_body:#}");

        // ── Wait up to 5 s for the push notification ─────────────────────────────
        match notify_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(line) => {
                eprintln!("[push-test] received notification: {line:?}");
                assert!(
                    line.contains("canvas://default"),
                    "notification should carry uri=canvas://default, got: {line}"
                );
            }
            Err(_) => panic!("did not receive notifications/resources/updated within 5s"),
        }
    });
}

/// rmcp's streamable HTTP can return either a plain JSON body or an SSE stream.
/// For the blocking client, we read the body and try to parse the first
/// JSON-RPC response embedded inside.
fn parse_mcp_response(resp: reqwest::blocking::Response) -> serde_json::Value {
    let text = resp.text().expect("response body text");
    // SSE format: one or more "event: message\ndata: {...}\n\n" blocks.
    // We grab the first `data:` line.
    if text.contains("data: ") {
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(json_str) = trimmed.strip_prefix("data: ") {
                if !json_str.is_empty() {
                    return serde_json::from_str(json_str)
                        .unwrap_or_else(|e| panic!("parse SSE data: {e}\nraw: {json_str}"));
                }
            }
        }
    }
    // Otherwise assume plain JSON.
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse JSON body: {e}\nraw: {text}"))
}

/// Extract the structured tool call result from a JSON-RPC response envelope.
/// Tries both `structuredContent` (MCP 2025-03-26) and `structured_content`.
fn extract_tool_result_structured(body: &serde_json::Value) -> Option<&serde_json::Value> {
    body.get("result").and_then(|r| {
        r.get("structuredContent")
            .or_else(|| r.get("structured_content"))
    })
}

/// Helper: POST a tools/call request and return the parsed JSON-RPC response body.
/// Does NOT assert success — callers can inspect both result and error.
fn call_tool_raw(
    client: &reqwest::blocking::Client,
    url: &str,
    session_id: &Option<String>,
    tool_name: &str,
    arguments: &serde_json::Value,
    id: u64,
) -> serde_json::Value {
    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments
        },
        "id": id
    });
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&req_body);
    if let Some(sid) = session_id {
        req = req.header("mcp-session-id", sid);
    }
    let resp = req
        .send()
        .unwrap_or_else(|e| panic!("tools/call {tool_name} request failed: {e}"));
    let status = resp.status();
    let body = parse_mcp_response(resp);
    eprintln!("[test] {tool_name} (id={id}) HTTP {status} body: {body:#}");
    body
}

/// Helper: call a tool and assert the structured result is returned (no JSON-RPC error).
fn call_tool_ok(
    client: &reqwest::blocking::Client,
    url: &str,
    session_id: &Option<String>,
    tool_name: &str,
    arguments: &serde_json::Value,
    id: u64,
) -> serde_json::Value {
    let body = call_tool_raw(client, url, session_id, tool_name, arguments, id);
    let result = extract_tool_result_structured(&body)
        .unwrap_or_else(|| panic!("{tool_name} expected structuredContent, got: {body:#}"));
    result.clone()
}

/// Helper: fetch the first room_id by calling list_rooms.
fn fetch_room_id(
    client: &reqwest::blocking::Client,
    url: &str,
    session_id: &Option<String>,
) -> String {
    let rooms = call_tool_ok(
        client,
        url,
        session_id,
        "list_rooms",
        &serde_json::json!({}),
        100,
    );
    let arr = rooms.as_array().expect("list_rooms should return array");
    assert!(!arr.is_empty(), "expected at least one room");
    arr[0]["id"]
        .as_str()
        .expect("room id is string")
        .to_string()
}
