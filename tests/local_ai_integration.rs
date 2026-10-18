//! End-to-end integration test for the local AI agent.
//! Uses wiremock to stand in for Ollama; runs the full
//! event → filter → decision → dispatch loop in-process.

#![cfg(all(feature = "local-ai", feature = "agent-server"))]

use redline::agent::{drain_ops, AgentServer};
use redline::local_ai::agent::{spawn, LocalAiConfig};
use redline::local_ai::roles::AgentRole;
use redline::state::AppState;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_ai_dispatches_canned_decision_action() {
    // ── 1. Mocked Ollama returns a Decision with one draw_ask action.
    let mock = MockServer::start().await;
    let canned = serde_json::json!({
        "model": "qwen3-vl:latest",
        "message": {
            "role": "assistant",
            "content": "{\"thinking\":\"I should ask the user to clarify what they are building before acting.\",\"scratchpad_update\":\"asked\",\"actions\":[{\"tool\":\"draw_ask\",\"args\":{\"content\":\"what are you building?\",\"size\":18.0,\"color\":\"#aaaa00\"}}],\"private_notes\":[]}"
        },
        "done": true
    });
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(canned))
        .mount(&mock)
        .await;

    // ── 2. Spin up redline's agent server (provides agent_op_tx + event_broadcast).
    let mut state = AppState::default();
    state.tab_label = "test".to_string();
    state.canvas.canvas_size = egui::vec2(400.0, 300.0);
    let port = 0;
    let (server, rx) = AgentServer::start(port);
    state.agent_op_rx = Some(rx);
    // Wire the broadcast so canvas events flow.
    state.canvas.event_broadcast = Some(server.event_broadcast.clone());

    // ── 3. Spawn local AI pointed at the mocked Ollama.
    let state_handle = Arc::new(Mutex::new(state));
    let drain_handle = state_handle.clone();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            {
                let mut s = drain_handle.lock().unwrap();
                drain_ops(&mut s, 64);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });

    let cfg = LocalAiConfig {
        model: "qwen3-vl:latest".into(),
        role: AgentRole::CollaboratorLight,
        ollama_url: mock.uri(),
    };
    let agent_op_tx = server.agent_op_tx.clone();
    let event_rx = server.event_broadcast.subscribe();
    let pid = redline::types::ParticipantId::new();
    let room_id = redline::agent::handlers::room_id_for_tab_index(0);
    let runtime = server.handle.clone();
    let handle = spawn(
        cfg,
        agent_op_tx,
        event_rx,
        pid,
        room_id,
        runtime,
        egui::Color32::from_rgb(0xaa, 0xaa, 0x00),
    );

    // ── 4. Trigger the agent by simulating a canvas event from a different actor.
    let other = redline::types::ParticipantId::new();
    let ann = redline::types::Annotation::new_authored(
        redline::types::ShapeData::plain_rect(egui::Rect::from_min_size(
            egui::pos2(10.0, 10.0),
            egui::vec2(50.0, 50.0),
        )),
        egui::Color32::BLUE,
        1.0,
        other,
    );
    {
        let mut s = state_handle.lock().unwrap();
        redline::command::CommandExecutor::execute(
            &mut s.canvas,
            other,
            redline::types::Command::Add(ann),
        );
    }

    // ── 5. Wait for the agent's draw_ask to land.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut saw_question = false;
    while std::time::Instant::now() < deadline {
        {
            let s = state_handle.lock().unwrap();
            if s.canvas
                .annotations
                .iter()
                .any(|a| matches!(a.shape, redline::types::ShapeData::Question { .. }))
            {
                saw_question = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handle.stop();
    drop(server);

    assert!(
        saw_question,
        "expected the agent to drop a Question annotation in response to the simulated draw"
    );
}

/// Verifies that the local AI dispatcher wires draw_rect (and by extension all
/// newly-added draw tools) — the agent must land a Rect annotation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_ai_dispatches_draw_rect() {
    let mock = MockServer::start().await;
    let canned = serde_json::json!({
        "model": "qwen3-vl:latest",
        "message": {
            "role": "assistant",
            "content": "{\"thinking\":\"I will draw a rectangle to highlight the important area.\",\"scratchpad_update\":\"\",\"actions\":[{\"tool\":\"draw_rect\",\"args\":{\"rect\":[10,10,100,80],\"color\":\"#ff0000\",\"thickness\":2.0}}],\"private_notes\":[]}"
        },
        "done": true
    });
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(canned))
        .mount(&mock)
        .await;

    let mut state = AppState::default();
    state.tab_label = "test-rect".to_string();
    state.canvas.canvas_size = egui::vec2(400.0, 300.0);
    let (server, rx) = AgentServer::start(0);
    state.agent_op_rx = Some(rx);
    state.canvas.event_broadcast = Some(server.event_broadcast.clone());

    let state_handle = Arc::new(Mutex::new(state));
    let drain_handle = state_handle.clone();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            {
                let mut s = drain_handle.lock().unwrap();
                redline::agent::drain_ops(&mut s, 64);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });

    let cfg = LocalAiConfig {
        model: "qwen3-vl:latest".into(),
        role: AgentRole::CollaboratorLight,
        ollama_url: mock.uri(),
    };
    let agent_op_tx = server.agent_op_tx.clone();
    let event_rx = server.event_broadcast.subscribe();
    let pid = redline::types::ParticipantId::new();
    let room_id = redline::agent::handlers::room_id_for_tab_index(0);
    let handle = spawn(
        cfg,
        agent_op_tx,
        event_rx,
        pid,
        room_id,
        server.handle.clone(),
        egui::Color32::RED,
    );

    // Trigger the agent.
    let other = redline::types::ParticipantId::new();
    let ann = redline::types::Annotation::new_authored(
        redline::types::ShapeData::plain_rect(egui::Rect::from_min_size(
            egui::pos2(5.0, 5.0),
            egui::vec2(20.0, 20.0),
        )),
        egui::Color32::BLUE,
        1.0,
        other,
    );
    {
        let mut s = state_handle.lock().unwrap();
        redline::command::CommandExecutor::execute(
            &mut s.canvas,
            other,
            redline::types::Command::Add(ann),
        );
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut saw_rect = false;
    while std::time::Instant::now() < deadline {
        {
            let s = state_handle.lock().unwrap();
            if s.canvas.annotations.iter().any(|a| {
                matches!(a.shape, redline::types::ShapeData::Rect { .. }) && a.author == pid
            }) {
                saw_rect = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handle.stop();
    drop(server);
    assert!(
        saw_rect,
        "expected agent to draw a Rect annotation via draw_rect dispatch"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn local_ai_round_trip_against_real_ollama() {
    let mut state = AppState::default();
    state.tab_label = "smoke".to_string();
    state.canvas.canvas_size = egui::vec2(400.0, 300.0);
    let (server, rx) = AgentServer::start(0);
    state.agent_op_rx = Some(rx);
    state.canvas.event_broadcast = Some(server.event_broadcast.clone());

    let state_handle = Arc::new(Mutex::new(state));
    let drain_handle = state_handle.clone();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            {
                let mut s = drain_handle.lock().unwrap();
                drain_ops(&mut s, 64);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });

    let cfg = LocalAiConfig {
        model: "qwen3-vl:latest".into(),
        role: AgentRole::CollaboratorLight,
        ollama_url: "http://localhost:11434".into(),
    };
    let agent_op_tx = server.agent_op_tx.clone();
    let event_rx = server.event_broadcast.subscribe();
    let pid = redline::types::ParticipantId::new();
    let room_id = redline::agent::handlers::room_id_for_tab_index(0);
    let runtime = server.handle.clone();
    let handle = spawn(
        cfg,
        agent_op_tx,
        event_rx,
        pid,
        room_id,
        runtime,
        egui::Color32::from_rgb(0xaa, 0xaa, 0x00),
    );

    // Trigger event.
    let other = redline::types::ParticipantId::new();
    let ann = redline::types::Annotation::new_authored(
        redline::types::ShapeData::plain_rect(egui::Rect::from_min_size(
            egui::pos2(10.0, 10.0),
            egui::vec2(50.0, 50.0),
        )),
        egui::Color32::BLUE,
        1.0,
        other,
    );
    {
        let mut s = state_handle.lock().unwrap();
        redline::command::CommandExecutor::execute(
            &mut s.canvas,
            other,
            redline::types::Command::Add(ann),
        );
    }

    // Wait up to 60s for ANY action to land (real models are slow).
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut acted = false;
    while std::time::Instant::now() < deadline {
        {
            let s = state_handle.lock().unwrap();
            if s.canvas.annotations.len() > 1 {
                acted = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    handle.stop();
    drop(server);
    assert!(
        acted,
        "expected qwen3-vl to emit at least one action within 60s"
    );
}
