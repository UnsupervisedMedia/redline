//! The local AI's tokio task. Subscribes to event_broadcast, runs survivors
//! through filter + decision + dispatch. Serializes decision passes; cancels
//! in-flight passes on shutdown. Maintains scratchpad, budget, and recent-
//! decisions ring buffer.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver as BroadcastReceiver;
use tokio::sync::mpsc::{Sender as MpscSender, UnboundedSender};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::events::ServerEvent;
use crate::agent::types::{AgentOp, AnnotationDescriptor};
use crate::local_ai::action_policy::should_drop_generic_clarification;
use crate::local_ai::analysis::analyze;
use crate::local_ai::behavioral::{render_summary, BehavioralCounters, BehavioralEventKind};
use crate::local_ai::budget::BudgetPool;
use crate::local_ai::decision::{self, DecisionAction, DecisionInput};
use crate::local_ai::event_filter::{EventFilter, Verdict};
use crate::local_ai::memory::{MemoryEntry, MemoryKind};
use crate::local_ai::ollama::OllamaClient;
use crate::local_ai::prompt::build_system_prompt;
use crate::local_ai::roles::AgentRole;
use crate::local_ai::safety::{self, CircuitBreaker};
use crate::local_ai::{AgentId, LOCAL_AI_ID};
use crate::types::ParticipantId;

const SCRATCHPAD_MAX_BYTES: usize = 2048;
const LOG_CAPACITY: usize = 200;
const RECENT_EVENT_CAPACITY: usize = 10;
const RECENT_DECISION_CAPACITY: usize = 6;
/// How long to wait for a dispatched `AgentOp` to reply before treating it as a
/// failure. Generous: ops are local in-process channel round-trips.
const DISPATCH_REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Append a line to the UI log and to stderr. Drops the line on lock contention
/// rather than awaiting (the log is best-effort; eprintln is the authoritative
/// trace).
async fn log_line(log: &Arc<RwLock<VecDeque<String>>>, line: String) {
    eprintln!("[local-ai] {line}");
    let mut l = log.write().await;
    if l.len() >= LOG_CAPACITY {
        l.pop_front();
    }
    let ts = chrono::Local::now().format("%H:%M:%S");
    l.push_back(format!("{ts}  {line}"));
}

async fn push_recent(ring: &Arc<RwLock<VecDeque<String>>>, line: String, capacity: usize) {
    let mut items = ring.write().await;
    if items.len() >= capacity {
        items.pop_front();
    }
    items.push_back(line);
}

pub struct LocalAiConfig {
    pub model: String,
    pub role: AgentRole,
    pub ollama_url: String,
}

impl Default for LocalAiConfig {
    fn default() -> Self {
        Self {
            model: "qwen3-vl:latest".into(),
            role: AgentRole::default(),
            ollama_url: "http://localhost:11434".into(),
        }
    }
}

pub struct LocalAiHandle {
    pub cancel: CancellationToken,
    pub scratchpad: Arc<RwLock<String>>,
    pub budget: Arc<Mutex<BudgetPool>>,
    pub role: Arc<RwLock<AgentRole>>,
    pub mute_until: Arc<RwLock<Option<Instant>>>,
    /// Default color the agent uses when the model doesn't specify one.
    /// Lives behind RwLock so the UI panel can mutate it live.
    pub color: Arc<RwLock<egui::Color32>>,
    /// Bounded ring of recent log lines for the UI panel. Most recent at back.
    pub log: Arc<RwLock<VecDeque<String>>>,
    /// Push a user-typed message here from the panel; the agent wakes and
    /// runs a decision pass with the message as user_instruction.
    pub user_input_tx: UnboundedSender<String>,
    /// Runtime handle for sync callers (UI) to spawn fire-and-forget tasks
    /// (mute / unmute / role change) without panicking.
    pub runtime: tokio::runtime::Handle,
    pub _join: tokio::task::JoinHandle<()>,
    pub _prov_join: tokio::task::JoinHandle<()>,
}

impl LocalAiHandle {
    pub fn stop(self) {
        self.cancel.cancel();
        // task aborts on next select.
    }
}

async fn load_memory(
    agent_op_tx: &MpscSender<AgentOp>,
    room_id: Uuid,
    agent_id: &AgentId,
) -> Vec<MemoryEntry> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = agent_op_tx
        .send(AgentOp::LoadAgentMemory {
            room_id,
            agent_id: agent_id.clone(),
            reply: tx,
        })
        .await;
    rx.await.ok().and_then(|r| r.ok()).unwrap_or_default()
}

/// Snapshot the canvas as a PNG, base64-encoded for the vision model.
/// Returns None if the snapshot fails — caller falls back to text-only.
async fn fetch_snapshot_b64(agent_op_tx: &MpscSender<AgentOp>, room_id: Uuid) -> Option<String> {
    use base64::Engine;
    let (tx, rx) = tokio::sync::oneshot::channel();
    if agent_op_tx
        .send(AgentOp::SnapshotPng {
            room_id,
            scale: 1.0,
            reply: tx,
        })
        .await
        .is_err()
    {
        return None;
    }
    let bytes = rx.await.ok()?.ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

/// List current canvas shapes as a JSON array string.
async fn fetch_selected_ids(agent_op_tx: &MpscSender<AgentOp>, room_id: Uuid) -> Vec<uuid::Uuid> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if agent_op_tx
        .send(AgentOp::GetSelectedShapeIds { room_id, reply: tx })
        .await
        .is_err()
    {
        return vec![];
    }
    match rx.await {
        Ok(Ok(ids)) => ids,
        _ => vec![],
    }
}

async fn fetch_shapes_json(agent_op_tx: &MpscSender<AgentOp>, room_id: Uuid) -> String {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if agent_op_tx
        .send(AgentOp::ListShapes { room_id, reply: tx })
        .await
        .is_err()
    {
        return "[]".to_string();
    }
    match rx.await {
        Ok(Ok(shapes)) => serde_json::to_string(&shapes).unwrap_or_else(|_| "[]".into()),
        _ => "[]".into(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_actions(
    actions: Vec<DecisionAction>,
    budget: &Arc<Mutex<BudgetPool>>,
    counters: &Arc<Mutex<BehavioralCounters>>,
    breaker: &Arc<Mutex<CircuitBreaker>>,
    agent_id: &AgentId,
    self_pid: ParticipantId,
    room_id: Uuid,
    agent_op_tx: &MpscSender<AgentOp>,
    color: &Arc<RwLock<egui::Color32>>,
    log: &Arc<RwLock<VecDeque<String>>>,
) {
    if actions.is_empty() {
        let mut c = counters.lock().await;
        c.record(BehavioralEventKind::Abstention, Instant::now());
        return;
    }

    // Safety rail: cap destructive ops per decision pass so a single bad
    // decision can't bulk-delete/move a lot of work at once (Feature 5).
    let mut destructive_used = 0usize;

    for action in actions {
        if safety::is_destructive(&action.tool) {
            if destructive_used >= safety::MAX_DESTRUCTIVE_OPS_PER_PASS {
                log_line(
                    log,
                    format!(
                        "safety: destructive cap ({}) reached this pass; skipping {}",
                        safety::MAX_DESTRUCTIVE_OPS_PER_PASS,
                        action.tool
                    ),
                )
                .await;
                let mut c = counters.lock().await;
                c.record(BehavioralEventKind::Abstention, Instant::now());
                continue;
            }
            destructive_used += 1;
        }
        // Don't spend budget on tools that have no dispatch arm (e.g. the
        // advertised but unimplemented add_bookmark/remove_bookmark/
        // append_shared_note/delete_shared_note — see `is_dispatchable`). They
        // would otherwise consume a token and then fail as "unknown tool".
        if !is_dispatchable(&action.tool) {
            log_line(
                log,
                format!(
                    "skipping {} (no dispatch arm; not spending budget)",
                    action.tool
                ),
            )
            .await;
            let mut c = counters.lock().await;
            c.record(BehavioralEventKind::Abstention, Instant::now());
            continue;
        }
        let cost_ok = match action.tool.as_str() {
            "draw_ask" => budget.lock().await.try_spend_ask(),
            "draw_propose" => budget.lock().await.try_spend_act(true),
            t if t.starts_with("draw_") => budget.lock().await.try_spend_act(false),
            "add_bookmark" | "remove_bookmark" => budget.lock().await.try_spend_bookmark(),
            _ => budget.lock().await.try_spend_act(false),
        };
        if !cost_ok {
            log_line(log, format!("no budget for {}; abstaining", action.tool)).await;
            let mut c = counters.lock().await;
            c.record(BehavioralEventKind::Abstention, Instant::now());
            continue;
        }
        let default_color = *color.read().await;
        match dispatch_action_from_json(
            &action.tool,
            action.args.clone(),
            agent_id,
            self_pid,
            room_id,
            agent_op_tx,
            default_color,
        )
        .await
        {
            Ok(()) => {
                log_line(
                    log,
                    format!("✓ {}", summarize_action(&action.tool, &action.args)),
                )
                .await;
            }
            Err(e) => {
                log_line(log, format!("✗ {} → {e}", action.tool)).await;
                continue;
            }
        }
        {
            let mut c = counters.lock().await;
            c.record(BehavioralEventKind::ActCommitted, Instant::now());
        }
        // A fresh successful commit resets the consecutive-undo streak the
        // circuit breaker watches.
        breaker.lock().await.record_commit();
    }
}

/// Returns true if `dispatch_action_from_json` has a real arm for `tool`.
/// Tools advertised in the role allowlist but lacking a backing `AgentOp`
/// (add_bookmark/remove_bookmark/append_shared_note/delete_shared_note) return
/// false so we don't spend budget on an op that can only fail.
fn is_dispatchable(tool: &str) -> bool {
    matches!(
        tool,
        "draw_ask"
            | "draw_propose"
            | "draw_arrow"
            | "draw_rect"
            | "draw_pen"
            | "draw_text"
            | "draw_oval"
            | "draw_line"
            | "draw_star"
            | "draw_callout"
            | "draw_rounded_rect"
            | "draw_highlighter"
            | "record_tool_wish"
            | "record_lesson"
            | "record_private_note"
            | "delete_shape"
            | "move_shape"
            | "resize_shape"
            | "set_prop"
            | "set_text_content"
            | "replace_pen_points"
            | "bring_to_front"
            | "send_to_back"
            | "reorder_to"
            | "undo"
            | "redo"
            | "renumber_steps"
    )
}

/// Spawn the agent loop. Returns a handle the UI uses for state and shutdown.
///
/// `runtime` is the tokio runtime Handle to spawn onto. Pass the
/// `AgentServer::handle.clone()` — the egui main thread itself isn't
/// inside a tokio runtime, so spawning via `tokio::spawn(...)` from here
/// would panic.
pub fn spawn(
    config: LocalAiConfig,
    agent_op_tx: MpscSender<AgentOp>,
    event_rx: BroadcastReceiver<ServerEvent>,
    self_pid: ParticipantId,
    room_id: Uuid,
    runtime: tokio::runtime::Handle,
    default_color: egui::Color32,
) -> LocalAiHandle {
    let cancel = CancellationToken::new();
    let prov_cancel = cancel.clone();
    let prov_op_tx = agent_op_tx.clone();
    let scratchpad = Arc::new(RwLock::new(String::new()));
    let counters = Arc::new(Mutex::new(BehavioralCounters::default()));
    let budget = Arc::new(Mutex::new(BudgetPool::new(config.role.budget())));
    let breaker = Arc::new(Mutex::new(CircuitBreaker::new()));
    let role = Arc::new(RwLock::new(config.role));
    let mute_until = Arc::new(RwLock::new(None));
    let color: Arc<RwLock<egui::Color32>> = Arc::new(RwLock::new(default_color));
    let color_for_task = color.clone();

    let agent_id: AgentId = LOCAL_AI_ID.into();
    let model = config.model.clone();
    let ollama = OllamaClient::new(config.ollama_url);

    let lessons_handle: Arc<RwLock<Vec<MemoryEntry>>> = Arc::new(RwLock::new(Vec::new()));
    let lessons_for_task = lessons_handle.clone();
    let last_compress = Arc::new(RwLock::new(Instant::now()));

    let log: Arc<RwLock<VecDeque<String>>> =
        Arc::new(RwLock::new(VecDeque::with_capacity(LOG_CAPACITY)));
    let log_for_task = log.clone();
    let recent_events: Arc<RwLock<VecDeque<String>>> =
        Arc::new(RwLock::new(VecDeque::with_capacity(RECENT_EVENT_CAPACITY)));
    let recent_events_for_task = recent_events.clone();
    let recent_decisions: Arc<RwLock<VecDeque<String>>> = Arc::new(RwLock::new(
        VecDeque::with_capacity(RECENT_DECISION_CAPACITY),
    ));
    let recent_decisions_for_task = recent_decisions.clone();

    let (user_input_tx, user_input_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let prov_join = crate::local_ai::provisional::spawn(
        prov_op_tx,
        self_pid,
        room_id,
        prov_cancel,
        runtime.clone(),
    );

    let cancel_for_task = cancel.clone();
    let scratchpad_for_task = scratchpad.clone();
    let counters_for_task = counters.clone();
    let budget_for_task = budget.clone();
    let breaker_for_task = breaker.clone();
    let role_for_task = role.clone();
    let mute_for_task = mute_until.clone();
    let agent_id_for_task = agent_id.clone();
    let agent_op_tx_for_task = agent_op_tx.clone();

    let _join = runtime.spawn(async move {
        let mut user_input_rx = user_input_rx;
        log_line(
            &log_for_task,
            format!(
                "agent started (model={}, mode={}, role={})",
                model,
                config.role.mode_label(),
                config.role.label()
            ),
        )
        .await;
        let mut filter = EventFilter::new(self_pid);
        let mut event_rx = event_rx;
        let mut idle_ticker = tokio::time::interval(Duration::from_secs(1));

        // Bootstrap: load any persisted memory for this agent on this canvas.
        {
            let loaded = load_memory(&agent_op_tx_for_task, room_id, &agent_id_for_task).await;
            let initial_lessons: Vec<MemoryEntry> = loaded.into_iter()
                .filter(|e| matches!(e.kind, MemoryKind::Lesson | MemoryKind::Pinned | MemoryKind::Summary))
                .collect();
            let n_lessons = initial_lessons.len();
            *lessons_for_task.write().await = initial_lessons;
            log_line(&log_for_task, format!("loaded {n_lessons} stable memory entries from canvas")).await;
        }

        loop {
            tokio::select! {
                biased;
                _ = cancel_for_task.cancelled() => break,
                _ = idle_ticker.tick() => {
                    let role_now = *role_for_task.read().await;
                    filter.configure(
                        role_now.reaction_quiet_period(),
                        role_now.idle_followup_period(),
                    );
                    // Refill budget on every tick.
                    {
                        let mut b = budget_for_task.lock().await;
                        b.refill_due(Instant::now());
                    }
                    // Compression: every ~30s, check if memory exceeds threshold.
                    {
                        let elapsed = Instant::now().duration_since(*last_compress.read().await);
                        if elapsed > Duration::from_secs(30) {
                            *last_compress.write().await = Instant::now();
                            let entries = load_memory(&agent_op_tx_for_task, room_id, &agent_id_for_task).await;
                            if crate::local_ai::memory::should_compress(&entries) {
                                let decisions_only: Vec<String> = entries.iter()
                                    .filter(|e| matches!(e.kind, MemoryKind::Decision))
                                    .map(|e| e.content.clone())
                                    .collect();
                                if !decisions_only.is_empty() {
                                    let req = crate::local_ai::ollama::ChatRequest::new(&model)
                                        .system("Summarize the following list of decisions into a single 3-5 sentence summary. Output only the summary text, no JSON.")
                                        .user(decisions_only.join("\n"));
                                    if let Ok(resp) = ollama.chat(&req).await {
                                        let (tx2, _rx2) = tokio::sync::oneshot::channel();
                                        let _ = agent_op_tx_for_task
                                            .send(AgentOp::CompressAgentMemory {
                                                room_id,
                                                agent_id: agent_id_for_task.clone(),
                                                summary_text: resp.message.content,
                                                reply: tx2,
                                            })
                                            .await;
                                    }
                                }
                            }
                        }
                    }
                    // Check for idle salience.
                    let verdict = filter.tick(Instant::now());
                    if matches!(verdict, Verdict::Proceed { .. }) {
                        log_line(&log_for_task, "idle salience → decision pass".into()).await;
                        run_decision_if_unmuted(
                            &mute_for_task,
                            &budget_for_task,
                            &counters_for_task,
                            &breaker_for_task,
                            &scratchpad_for_task,
                            &role_for_task,
                            &agent_id_for_task,
                            &agent_op_tx_for_task,
                            &ollama,
                            &model,
                            self_pid,
                            room_id,
                            &lessons_for_task,
                            &recent_events_for_task,
                            &recent_decisions_for_task,
                            &log_for_task,
                            &color_for_task,
                            None,
                        ).await;
                    }
                }
                evt = event_rx.recv() => {
                    let Ok(evt) = evt else {
                        log_line(&log_for_task, "event_rx returned error (lagged/closed)".into()).await;
                        continue;
                    };
                    let evt_summary = summarize_event(&evt);
                    log_line(&log_for_task, format!("event: {evt_summary}")).await;
                    push_recent(
                        &recent_events_for_task,
                        evt_summary.clone(),
                        RECENT_EVENT_CAPACITY,
                    ).await;
                    if let ServerEvent::ShapeDeleted { author, deleted_author, .. } = &evt {
                        if *author != self_pid && *deleted_author == self_pid {
                            {
                                let mut b = budget_for_task.lock().await;
                                b.record_undo_of_self();
                            }
                            {
                                let mut c = counters_for_task.lock().await;
                                c.record(BehavioralEventKind::ActUndoneByHuman, Instant::now());
                            }
                            log_line(&log_for_task, "user undid an agent shape → budget penalty".into()).await;
                            // Circuit breaker: too many consecutive self-undos →
                            // auto-mute for a cooldown rather than keep fighting
                            // the user (Feature 5).
                            let tripped = breaker_for_task.lock().await.record_self_undo();
                            if tripped {
                                *mute_for_task.write().await =
                                    Some(Instant::now() + safety::CIRCUIT_BREAKER_COOLDOWN);
                                log_line(
                                    &log_for_task,
                                    format!(
                                        "⚠ circuit breaker tripped ({} consecutive undos) → auto-muting {}s",
                                        safety::UNDO_TRIP_THRESHOLD,
                                        safety::CIRCUIT_BREAKER_COOLDOWN.as_secs()
                                    ),
                                )
                                .await;
                            }
                        }
                    }
                    let role_now = *role_for_task.read().await;
                    filter.configure(
                        role_now.reaction_quiet_period(),
                        role_now.idle_followup_period(),
                    );
                    let verdict = filter.ingest(&evt, Instant::now());
                    if let Verdict::Proceed { reason } = verdict {
                        log_line(&log_for_task, format!("filter proceed ({reason:?}) → decision pass")).await;
                        run_decision_if_unmuted(
                            &mute_for_task,
                            &budget_for_task,
                            &counters_for_task,
                            &breaker_for_task,
                            &scratchpad_for_task,
                            &role_for_task,
                            &agent_id_for_task,
                            &agent_op_tx_for_task,
                            &ollama,
                            &model,
                            self_pid,
                            room_id,
                            &lessons_for_task,
                            &recent_events_for_task,
                            &recent_decisions_for_task,
                            &log_for_task,
                            &color_for_task,
                            None,
                        ).await;
                    } else {
                        log_line(&log_for_task, "filter buffered event".into()).await;
                    }
                }
                Some(user_msg) = user_input_rx.recv() => {
                    log_line(&log_for_task, format!("user: {user_msg}")).await;
                    push_recent(
                        &recent_events_for_task,
                        format!("user_message: {user_msg}"),
                        RECENT_EVENT_CAPACITY,
                    ).await;
                    run_decision_if_unmuted(
                        &mute_for_task,
                        &budget_for_task,
                        &counters_for_task,
                        &breaker_for_task,
                        &scratchpad_for_task,
                        &role_for_task,
                        &agent_id_for_task,
                        &agent_op_tx_for_task,
                        &ollama,
                        &model,
                        self_pid,
                        room_id,
                        &lessons_for_task,
                        &recent_events_for_task,
                        &recent_decisions_for_task,
                        &log_for_task,
                        &color_for_task,
                        Some(&user_msg),
                    ).await;
                }
            }
        }
    });

    fn summarize_event(evt: &ServerEvent) -> String {
        match evt {
            ServerEvent::ShapeAdded { author, shape, .. } => {
                format!(
                    "ShapeAdded {{ author={}, kind={}, id={} }}",
                    short_pid(author),
                    shape.kind,
                    short_id(&shape.id)
                )
            }
            ServerEvent::ShapeChanged {
                author,
                shape_id,
                change,
                ..
            } => {
                format!(
                    "ShapeChanged {{ author={}, id={}, change={} }}",
                    short_pid(author),
                    short_id(shape_id),
                    change
                )
            }
            ServerEvent::ShapeDeleted {
                author,
                shape_id,
                deleted_author,
                ..
            } => {
                format!(
                    "ShapeDeleted {{ author={}, deleted_author={}, id={} }}",
                    short_pid(author),
                    short_pid(deleted_author),
                    short_id(shape_id)
                )
            }
            ServerEvent::ImageReplaced { author, .. } => {
                format!("ImageReplaced {{ author={} }}", short_pid(author))
            }
            ServerEvent::LayerChanged {
                author,
                node_id,
                change,
                ..
            } => {
                format!(
                    "LayerChanged {{ author={}, node={}, change={} }}",
                    short_pid(author),
                    short_id(node_id),
                    change
                )
            }
        }
    }
    fn short_pid(p: &ParticipantId) -> String {
        if p.is_human_local() {
            "human".into()
        } else {
            short_id(&p.0)
        }
    }
    fn short_id(id: &Uuid) -> String {
        id.to_string().chars().take(8).collect()
    }

    LocalAiHandle {
        cancel,
        scratchpad,
        budget,
        role,
        mute_until,
        color,
        log,
        user_input_tx,
        runtime,
        _join,
        _prov_join: prov_join,
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
async fn run_decision_if_unmuted(
    mute_until: &Arc<RwLock<Option<Instant>>>,
    budget: &Arc<Mutex<BudgetPool>>,
    counters: &Arc<Mutex<BehavioralCounters>>,
    breaker: &Arc<Mutex<CircuitBreaker>>,
    scratchpad: &Arc<RwLock<String>>,
    role: &Arc<RwLock<AgentRole>>,
    agent_id: &AgentId,
    agent_op_tx: &MpscSender<AgentOp>,
    ollama: &OllamaClient,
    model: &str,
    self_pid: ParticipantId,
    room_id: Uuid,
    lessons: &Arc<RwLock<Vec<MemoryEntry>>>,
    recent_events: &Arc<RwLock<VecDeque<String>>>,
    recent_decisions: &Arc<RwLock<VecDeque<String>>>,
    log: &Arc<RwLock<VecDeque<String>>>,
    color: &Arc<RwLock<egui::Color32>>,
    user_message: Option<&str>,
) {
    // Muted? abstain and update scratchpad only.
    if let Some(until) = *mute_until.read().await {
        if Instant::now() < until {
            let mut c = counters.lock().await;
            c.record(BehavioralEventKind::Abstention, Instant::now());
            log_line(log, "muted; abstaining".into()).await;
            return;
        }
    }

    let role_now = *role.read().await;
    let sp = scratchpad.read().await.clone();
    let lessons_now = lessons.read().await.clone();
    let counters_snap = counters.lock().await.clone();
    let recent_lesson_count = lessons_now
        .iter()
        .filter(|e| matches!(e.kind, MemoryKind::Lesson | MemoryKind::Pinned))
        .count();
    let summary = {
        let bs = budget.lock().await;
        render_summary(
            &counters_snap,
            &*bs,
            role_now,
            Instant::now(),
            0,
            recent_lesson_count,
        )
    };
    let system = build_system_prompt(role_now, &lessons_now);
    let recent_events_summary = {
        let items = recent_events.read().await;
        items.iter().cloned().collect::<Vec<_>>().join("\n")
    };
    let recent_decisions_summary = {
        let items = recent_decisions.read().await;
        items.iter().cloned().collect::<Vec<_>>().join("\n")
    };

    // Fetch live canvas state. Prefer structured shape JSON for active drawing
    // sessions; only attach a PNG snapshot when there are no vector shapes to
    // reason over.
    let shapes_json = fetch_shapes_json(agent_op_tx, room_id).await;
    let shapes: Vec<AnnotationDescriptor> = serde_json::from_str(&shapes_json).unwrap_or_default();
    let analysis = analyze(&shapes);
    let selected_ids = fetch_selected_ids(agent_op_tx, room_id).await;
    let snapshot_b64 = if shapes.is_empty() {
        fetch_snapshot_b64(agent_op_tx, room_id).await
    } else {
        None
    };
    let snapshot_kb = snapshot_b64.as_ref().map(|s| s.len() / 1024).unwrap_or(0);
    log_line(
        log,
        format!(
            "context: {} chars of shapes, {} kB snapshot{}, {} selected",
            shapes_json.len(),
            snapshot_kb,
            if shapes.is_empty() {
                ""
            } else {
                " (skipped while vector shapes are present)"
            },
            selected_ids.len(),
        ),
    )
    .await;

    let selection_hint = if selected_ids.is_empty() {
        String::new()
    } else {
        let ids_str = selected_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("\n\nHuman currently has selected: [{ids_str}]")
    };
    let analysis_hint = {
        let rendered = analysis.render_for_prompt();
        if rendered.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n{rendered}\nTreat strong hypotheses as permission to be specific rather than generic."
            )
        }
    };
    let default_instruction_base = "Look at the canvas state and any attached snapshot. \
        Decide what (if anything) to do next. Take initiative if there's a clear opportunity \
        (an unfinished sketch, an inferred board/task with open structure, an obvious next mark). \
        \"Do nothing\" is still valid but should be reserved for cases with genuinely no signal. \
        If the structure strongly implies a common intent, make a best-guess assumption instead \
        of asking a generic clarification question.";
    let default_instruction_owned;
    let user_instruction = if let Some(msg) = user_message {
        msg
    } else {
        default_instruction_owned =
            format!("{default_instruction_base}{selection_hint}{analysis_hint}");
        &default_instruction_owned
    };
    let input = DecisionInput {
        model,
        system_prompt: &system,
        behavioral_summary: &summary,
        scratchpad: &sp,
        recent_decisions_summary: &recent_decisions_summary,
        recent_events_summary: &recent_events_summary,
        canvas_state_json: &shapes_json,
        image_base64: snapshot_b64,
        user_instruction,
    };

    log_line(
        log,
        format!(
            "→ ollama (mode={}, role={})...",
            role_now.mode_label(),
            role_now.label()
        ),
    )
    .await;
    let t0 = Instant::now();
    let dec = match decision::run(ollama, input).await {
        Ok(d) => {
            log_line(
                log,
                format!(
                    "← ollama in {:.1}s: {} actions; thinking={:?}",
                    t0.elapsed().as_secs_f32(),
                    d.actions.len(),
                    d.thinking.chars().take(500).collect::<String>(),
                ),
            )
            .await;
            d
        }
        Err(e) => {
            log_line(
                log,
                format!(
                    "decision error after {:.1}s: {e}",
                    t0.elapsed().as_secs_f32()
                ),
            )
            .await;
            return;
        }
    };

    let (kept, dropped) = decision::enforce_justification(&dec, role_now);
    if !dropped.is_empty() {
        log_line(
            log,
            format!(
                "enforce_justification: kept={} dropped={}",
                kept.len(),
                dropped.len()
            ),
        )
        .await;
        for d in &dropped {
            log_line(log, d.clone()).await;
        }
    }

    // Apply scratchpad update.
    {
        let mut sp = scratchpad.write().await;
        if !dec.scratchpad_update.is_empty() {
            *sp = dec.scratchpad_update;
            if sp.len() > SCRATCHPAD_MAX_BYTES {
                let cut = sp.len() - SCRATCHPAD_MAX_BYTES;
                *sp = sp[cut..].to_string();
            }
        }
    }

    // Persist private notes through the dispatch channel.
    for note in dec.private_notes {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let _ = agent_op_tx
            .send(AgentOp::RecordPrivateNote {
                room_id,
                agent_id: agent_id.clone(),
                content: note,
                reply: tx,
            })
            .await;
    }

    push_recent(
        recent_decisions,
        dec.thinking.chars().take(160).collect::<String>(),
        RECENT_DECISION_CAPACITY,
    )
    .await;

    // Append the Decision entry to memory (visible).
    {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let _ = agent_op_tx
            .send(AgentOp::RecordDecision {
                room_id,
                agent_id: agent_id.clone(),
                content: dec.thinking.clone(),
                reply: tx,
            })
            .await;
    }

    let kept =
        if should_drop_generic_clarification(&analysis, role_now, &kept, user_message.is_some()) {
            log_line(
                log,
                "policy dropped a generic clarification because structured analysis was strong"
                    .into(),
            )
            .await;
            Vec::new()
        } else {
            kept
        };

    execute_actions(
        kept,
        budget,
        counters,
        breaker,
        agent_id,
        self_pid,
        room_id,
        agent_op_tx,
        color,
        log,
    )
    .await;
}

fn summarize_action(tool: &str, args: &serde_json::Value) -> String {
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    if !content.is_empty() {
        let preview: String = content.chars().take(80).collect();
        format!("{tool}: \"{preview}\"")
    } else {
        let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if !kind.is_empty() {
            format!("{tool} kind={kind}")
        } else {
            tool.to_string()
        }
    }
}

/// Send an op whose reply is `Result<(), AgentError>` and await the real result.
async fn send_and_await_unit(
    agent_op_tx: &MpscSender<AgentOp>,
    op: AgentOp,
    rx: tokio::sync::oneshot::Receiver<Result<(), crate::agent::types::AgentError>>,
) -> Result<(), String> {
    if agent_op_tx.send(op).await.is_err() {
        return Err("agent_op channel closed".into());
    }
    match tokio::time::timeout(DISPATCH_REPLY_TIMEOUT, rx).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(e))) => Err(format!("{e}")),
        Ok(Err(_)) => Err("op reply channel dropped".into()),
        Err(_) => Err("op timed out".into()),
    }
}

/// Send an op whose reply is `Result<u64, AgentError>` (a seq) and await it.
async fn send_and_await_seq(
    agent_op_tx: &MpscSender<AgentOp>,
    op: AgentOp,
    rx: tokio::sync::oneshot::Receiver<Result<u64, crate::agent::types::AgentError>>,
) -> Result<(), String> {
    if agent_op_tx.send(op).await.is_err() {
        return Err("agent_op channel closed".into());
    }
    match tokio::time::timeout(DISPATCH_REPLY_TIMEOUT, rx).await {
        Ok(Ok(Ok(_seq))) => Ok(()),
        Ok(Ok(Err(e))) => Err(format!("{e}")),
        Ok(Err(_)) => Err("op reply channel dropped".into()),
        Err(_) => Err("op timed out".into()),
    }
}

/// Translate a (tool, args) JSON pair into an AgentOp and send it.
/// Returns the action's "kind" string for counters/logging.
#[allow(clippy::too_many_arguments)]
async fn dispatch_action_from_json(
    tool: &str,
    args: serde_json::Value,
    agent_id: &AgentId,
    self_pid: ParticipantId,
    room_id: Uuid,
    agent_op_tx: &MpscSender<AgentOp>,
    default_color: egui::Color32,
) -> Result<(), String> {
    use crate::types::{ArrowHeads, LineStyle, ShapeData};

    fn pos2(v: &serde_json::Value) -> Result<egui::Pos2, String> {
        let arr = v.as_array().ok_or("expected [x,y]")?;
        if arr.len() != 2 {
            return Err("expected [x,y]".into());
        }
        Ok(egui::pos2(
            arr[0].as_f64().ok_or("x")? as f32,
            arr[1].as_f64().ok_or("y")? as f32,
        ))
    }
    fn rect4(v: &serde_json::Value) -> Result<egui::Rect, String> {
        let arr = v.as_array().ok_or("expected [x_min,y_min,x_max,y_max]")?;
        if arr.len() != 4 {
            return Err("expected [x_min,y_min,x_max,y_max]".into());
        }
        Ok(egui::Rect::from_min_max(
            egui::pos2(
                arr[0].as_f64().ok_or("x_min")? as f32,
                arr[1].as_f64().ok_or("y_min")? as f32,
            ),
            egui::pos2(
                arr[2].as_f64().ok_or("x_max")? as f32,
                arr[3].as_f64().ok_or("y_max")? as f32,
            ),
        ))
    }
    fn color(v: &serde_json::Value) -> Result<egui::Color32, String> {
        let s = v.as_str().ok_or("color must be #RRGGBB string")?;
        let s = s.trim_start_matches('#');
        if s.len() != 6 {
            return Err("color must be 6 hex chars".into());
        }
        let r = u8::from_str_radix(&s[0..2], 16).map_err(|_| "color hex")?;
        let g = u8::from_str_radix(&s[2..4], 16).map_err(|_| "color hex")?;
        let b = u8::from_str_radix(&s[4..6], 16).map_err(|_| "color hex")?;
        Ok(egui::Color32::from_rgb(r, g, b))
    }

    // Reply channel for ops that return a `DrawResult` (all draw + most modify
    // ops). Ops with a different reply type (record_*, undo/redo/renumber_steps)
    // build their own channel and await it inline below, returning early.
    let (tx, rx) = tokio::sync::oneshot::channel::<
        Result<crate::agent::types::DrawResult, crate::agent::types::AgentError>,
    >();
    let op = match tool {
        "draw_ask" => {
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("content")?
                .to_string();
            let size = args.get("size").and_then(|v| v.as_f64()).unwrap_or(18.0) as f32;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let anchor = args.get("anchor").map(pos2).transpose()?;
            AgentOp::DrawAsk {
                room_id,
                actor: self_pid,
                anchor,
                content,
                size,
                color: c,
                reply: tx,
            }
        }
        "draw_propose" => {
            let shape_kind = args.get("kind").and_then(|v| v.as_str()).ok_or("kind")?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let shape = match shape_kind {
                "rect" => ShapeData::plain_rect(rect4(args.get("rect").ok_or("rect")?)?),
                "oval" => ShapeData::plain_oval(rect4(args.get("rect").ok_or("rect")?)?),
                "arrow" => ShapeData::Arrow {
                    start: pos2(args.get("start").ok_or("start")?)?,
                    end: pos2(args.get("end").ok_or("end")?)?,
                    style: match args
                        .get("style")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Solid")
                    {
                        "Dashed" => LineStyle::Dashed,
                        "Dotted" => LineStyle::Dotted,
                        _ => LineStyle::Solid,
                    },
                    heads: match args
                        .get("heads")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Single")
                    {
                        "Double" => ArrowHeads::Double,
                        _ => ArrowHeads::Single,
                    },
                },
                "line" => ShapeData::Line {
                    start: pos2(args.get("start").ok_or("start")?)?,
                    end: pos2(args.get("end").ok_or("end")?)?,
                    style: match args
                        .get("style")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Solid")
                    {
                        "Dashed" => LineStyle::Dashed,
                        "Dotted" => LineStyle::Dotted,
                        _ => LineStyle::Solid,
                    },
                },
                "star" => ShapeData::Star {
                    rect: rect4(args.get("rect").ok_or("rect")?)?,
                },
                "highlighter" => ShapeData::Rect {
                    rect: rect4(args.get("rect").ok_or("rect")?)?,
                    radius: 0.0,
                    fill: Some(egui::Color32::from_rgba_unmultiplied(
                        c.r(),
                        c.g(),
                        c.b(),
                        120,
                    )),
                    stroke: false,
                    stroke_style: LineStyle::Solid,
                },
                "callout" => ShapeData::Callout {
                    rect: rect4(args.get("rect").ok_or("rect")?)?,
                    tail: pos2(args.get("tail").ok_or("tail")?)?,
                },
                "rounded_rect" => ShapeData::Rect {
                    rect: rect4(args.get("rect").ok_or("rect")?)?,
                    radius: args.get("radius").and_then(|v| v.as_f64()).unwrap_or(8.0) as f32,
                    fill: Some(egui::Color32::from_rgba_unmultiplied(
                        c.r(),
                        c.g(),
                        c.b(),
                        20,
                    )),
                    stroke: true,
                    stroke_style: LineStyle::Solid,
                },
                "text" => {
                    let p = pos2(args.get("pos").ok_or("pos")?)?;
                    let content = args
                        .get("content")
                        .and_then(|v| v.as_str())
                        .ok_or("content")?
                        .to_string();
                    let size = args.get("size").and_then(|v| v.as_f64()).unwrap_or(18.0) as f32;
                    ShapeData::Text {
                        pos: p,
                        content,
                        size,
                    }
                }
                "pen" => {
                    let pts_arr = args
                        .get("points")
                        .and_then(|v| v.as_array())
                        .ok_or("points must be array")?;
                    let raw_points: Vec<egui::Pos2> =
                        pts_arr.iter().map(pos2).collect::<Result<_, _>>()?;
                    let smooth = args.get("smooth").and_then(|v| v.as_bool()).unwrap_or(true);
                    let smoothed_points = if smooth && raw_points.len() >= 2 {
                        crate::smoothing::catmull_rom(&raw_points, 8)
                    } else {
                        raw_points.clone()
                    };
                    ShapeData::Pen {
                        raw_points,
                        smoothed_points,
                    }
                }
                _ => return Err(format!("propose: unsupported kind {shape_kind}")),
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            let expires_secs = args
                .get("expires_in_secs")
                .and_then(|v| v.as_f64())
                .unwrap_or(30.0) as u64;
            AgentOp::DrawPropose {
                room_id,
                actor: self_pid,
                shape,
                color: c,
                thickness,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                expires_in: Duration::from_secs(expires_secs),
                reply: tx,
            }
        }
        "record_tool_wish" => {
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("content")?
                .to_string();
            let (tx2, rx2) = tokio::sync::oneshot::channel();
            let op = AgentOp::RecordToolWish {
                room_id,
                agent_id: agent_id.clone(),
                content,
                reply: tx2,
            };
            return send_and_await_unit(agent_op_tx, op, rx2).await;
        }
        "record_lesson" => {
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("content")?
                .to_string();
            let (tx2, rx2) = tokio::sync::oneshot::channel();
            let op = AgentOp::RecordLesson {
                room_id,
                agent_id: agent_id.clone(),
                content,
                reply: tx2,
            };
            return send_and_await_unit(agent_op_tx, op, rx2).await;
        }
        "record_private_note" => {
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("content")?
                .to_string();
            let (tx2, rx2) = tokio::sync::oneshot::channel();
            let op = AgentOp::RecordPrivateNote {
                room_id,
                agent_id: agent_id.clone(),
                content,
                reply: tx2,
            };
            return send_and_await_unit(agent_op_tx, op, rx2).await;
        }
        "draw_arrow" => {
            let start = pos2(args.get("start").ok_or("start")?)?;
            let end = pos2(args.get("end").ok_or("end")?)?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            let style = match args
                .get("style")
                .and_then(|v| v.as_str())
                .unwrap_or("Solid")
            {
                "Dashed" => LineStyle::Dashed,
                "Dotted" => LineStyle::Dotted,
                _ => LineStyle::Solid,
            };
            let heads = match args
                .get("heads")
                .and_then(|v| v.as_str())
                .unwrap_or("Single")
            {
                "Double" => ArrowHeads::Double,
                _ => ArrowHeads::Single,
            };
            AgentOp::DrawArrow {
                room_id,
                actor: self_pid,
                start,
                end,
                color: c,
                thickness,
                style,
                heads,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        "draw_rect" => {
            let rect = rect4(args.get("rect").ok_or("rect")?)?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            AgentOp::DrawRect {
                room_id,
                actor: self_pid,
                rect,
                color: c,
                thickness,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        "draw_pen" => {
            let pts_arr = args
                .get("points")
                .and_then(|v| v.as_array())
                .ok_or("points must be array")?;
            let points: Vec<egui::Pos2> = pts_arr.iter().map(pos2).collect::<Result<_, _>>()?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            let smooth = args.get("smooth").and_then(|v| v.as_bool()).unwrap_or(true);
            AgentOp::DrawPen {
                room_id,
                actor: self_pid,
                points,
                smooth,
                color: c,
                thickness,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        "draw_text" => {
            let p = pos2(args.get("pos").ok_or("pos")?)?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("content")?
                .to_string();
            let size = args.get("size").and_then(|v| v.as_f64()).unwrap_or(18.0) as f32;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            AgentOp::DrawText {
                room_id,
                actor: self_pid,
                pos: p,
                content,
                size,
                color: c,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        "draw_oval" => {
            let rect = rect4(args.get("rect").ok_or("rect")?)?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            AgentOp::DrawOval {
                room_id,
                actor: self_pid,
                rect,
                color: c,
                thickness,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        "draw_line" => {
            let start = pos2(args.get("start").ok_or("start")?)?;
            let end = pos2(args.get("end").ok_or("end")?)?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            let style = match args
                .get("style")
                .and_then(|v| v.as_str())
                .unwrap_or("Solid")
            {
                "Dashed" => LineStyle::Dashed,
                "Dotted" => LineStyle::Dotted,
                _ => LineStyle::Solid,
            };
            AgentOp::DrawLine {
                room_id,
                actor: self_pid,
                start,
                end,
                style,
                color: c,
                thickness,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        "draw_star" => {
            let rect = rect4(args.get("rect").ok_or("rect")?)?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            AgentOp::DrawStar {
                room_id,
                actor: self_pid,
                rect,
                color: c,
                thickness,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        "draw_callout" => {
            let rect = rect4(args.get("rect").ok_or("rect")?)?;
            let tail = pos2(args.get("tail").ok_or("tail")?)?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            AgentOp::DrawCallout {
                room_id,
                actor: self_pid,
                rect,
                tail,
                color: c,
                thickness,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        "draw_rounded_rect" => {
            let rect = rect4(args.get("rect").ok_or("rect")?)?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            let radius = args.get("radius").and_then(|v| v.as_f64()).unwrap_or(8.0) as f32;
            AgentOp::DrawRoundedRect {
                room_id,
                actor: self_pid,
                rect,
                radius,
                color: c,
                thickness,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        "draw_highlighter" => {
            let rect = rect4(args.get("rect").ok_or("rect")?)?;
            let c = match args.get("color") {
                Some(v) => color(v)?,
                None => default_color,
            };
            let thickness = args
                .get("thickness")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0) as f32;
            AgentOp::DrawHighlighter {
                room_id,
                actor: self_pid,
                rect,
                color: c,
                thickness,
                label: args.get("label").and_then(|v| v.as_str()).map(String::from),
                reply: tx,
            }
        }
        // ── Modify operations ──────────────────────────────────────────────────
        "delete_shape" => {
            let shape_id = args
                .get("shape_id")
                .and_then(|v| v.as_str())
                .ok_or("shape_id")?
                .parse::<uuid::Uuid>()
                .map_err(|_| "shape_id must be a UUID")?;
            AgentOp::DeleteShape {
                room_id,
                actor: self_pid,
                shape_id,
                reply: tx,
            }
        }
        "move_shape" => {
            let shape_id = args
                .get("shape_id")
                .and_then(|v| v.as_str())
                .ok_or("shape_id")?
                .parse::<uuid::Uuid>()
                .map_err(|_| "shape_id must be a UUID")?;
            let delta_arr = args
                .get("delta")
                .and_then(|v| v.as_array())
                .ok_or("delta must be [dx,dy]")?;
            if delta_arr.len() != 2 {
                return Err("delta must be [dx,dy]".into());
            }
            let dx = delta_arr[0].as_f64().ok_or("dx")? as f32;
            let dy = delta_arr[1].as_f64().ok_or("dy")? as f32;
            AgentOp::MoveShape {
                room_id,
                actor: self_pid,
                shape_id,
                delta: egui::Vec2::new(dx, dy),
                reply: tx,
            }
        }
        "resize_shape" => {
            let shape_id = args
                .get("shape_id")
                .and_then(|v| v.as_str())
                .ok_or("shape_id")?
                .parse::<uuid::Uuid>()
                .map_err(|_| "shape_id must be a UUID")?;
            let new_rect = rect4(args.get("new_rect").ok_or("new_rect")?)?;
            AgentOp::ResizeShape {
                room_id,
                actor: self_pid,
                shape_id,
                new_rect,
                reply: tx,
            }
        }
        "set_prop" => {
            use crate::types::PropChange;
            let shape_id = args
                .get("shape_id")
                .and_then(|v| v.as_str())
                .ok_or("shape_id")?
                .parse::<uuid::Uuid>()
                .map_err(|_| "shape_id must be a UUID")?;
            let prop = if let Some(v) = args.get("color") {
                PropChange::Color(color(v)?)
            } else if let Some(v) = args.get("thickness") {
                PropChange::Thickness(v.as_f64().ok_or("thickness")? as f32)
            } else if let Some(v) = args.get("label") {
                PropChange::Label(v.as_str().map(String::from))
            } else if let Some(v) = args.get("locked") {
                PropChange::Locked(v.as_bool().ok_or("locked must be bool")?)
            } else if let Some(v) = args.get("visible") {
                PropChange::Visible(v.as_bool().ok_or("visible must be bool")?)
            } else if let Some(v) = args.get("step_number") {
                PropChange::StepNumber(v.as_u64().ok_or("step_number must be u64")? as u32)
            } else {
                return Err(
                    "set_prop: one of color/thickness/label/locked/visible/step_number required"
                        .into(),
                );
            };
            AgentOp::SetProp {
                room_id,
                actor: self_pid,
                shape_id,
                prop,
                reply: tx,
            }
        }
        "set_text_content" => {
            let shape_id = args
                .get("shape_id")
                .and_then(|v| v.as_str())
                .ok_or("shape_id")?
                .parse::<uuid::Uuid>()
                .map_err(|_| "shape_id must be a UUID")?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("content")?
                .to_string();
            AgentOp::SetTextContent {
                room_id,
                actor: self_pid,
                shape_id,
                content,
                reply: tx,
            }
        }
        "replace_pen_points" => {
            let shape_id = args
                .get("shape_id")
                .and_then(|v| v.as_str())
                .ok_or("shape_id")?
                .parse::<uuid::Uuid>()
                .map_err(|_| "shape_id must be a UUID")?;
            let pts_arr = args
                .get("points")
                .and_then(|v| v.as_array())
                .ok_or("points must be array")?;
            let points: Vec<egui::Pos2> = pts_arr.iter().map(pos2).collect::<Result<_, _>>()?;
            let smooth = args.get("smooth").and_then(|v| v.as_bool()).unwrap_or(true);
            AgentOp::ReplacePenPoints {
                room_id,
                actor: self_pid,
                shape_id,
                points,
                smooth,
                reply: tx,
            }
        }
        "bring_to_front" => {
            let shape_id = args
                .get("shape_id")
                .and_then(|v| v.as_str())
                .ok_or("shape_id")?
                .parse::<uuid::Uuid>()
                .map_err(|_| "shape_id must be a UUID")?;
            AgentOp::BringToFront {
                room_id,
                actor: self_pid,
                shape_id,
                reply: tx,
            }
        }
        "send_to_back" => {
            let shape_id = args
                .get("shape_id")
                .and_then(|v| v.as_str())
                .ok_or("shape_id")?
                .parse::<uuid::Uuid>()
                .map_err(|_| "shape_id must be a UUID")?;
            AgentOp::SendToBack {
                room_id,
                actor: self_pid,
                shape_id,
                reply: tx,
            }
        }
        "reorder_to" => {
            let shape_id = args
                .get("shape_id")
                .and_then(|v| v.as_str())
                .ok_or("shape_id")?
                .parse::<uuid::Uuid>()
                .map_err(|_| "shape_id must be a UUID")?;
            let index = args
                .get("index")
                .and_then(|v| v.as_u64())
                .ok_or("index must be u64")? as usize;
            AgentOp::ReorderTo {
                room_id,
                actor: self_pid,
                shape_id,
                index,
                reply: tx,
            }
        }
        "undo" => {
            let (tx2, rx2) = tokio::sync::oneshot::channel();
            let op = AgentOp::Undo {
                room_id,
                actor: self_pid,
                reply: tx2,
            };
            return send_and_await_seq(agent_op_tx, op, rx2).await;
        }
        "redo" => {
            let (tx2, rx2) = tokio::sync::oneshot::channel();
            let op = AgentOp::Redo {
                room_id,
                actor: self_pid,
                reply: tx2,
            };
            return send_and_await_seq(agent_op_tx, op, rx2).await;
        }
        "renumber_steps" => {
            let (tx2, rx2) = tokio::sync::oneshot::channel();
            let op = AgentOp::RenumberSteps {
                room_id,
                actor: self_pid,
                reply: tx2,
            };
            return send_and_await_seq(agent_op_tx, op, rx2).await;
        }
        other => return Err(format!("local-ai dispatch: unknown tool: {other}")),
    };
    // Send the op and await its real result so failed ops (bad UUID, locked,
    // out-of-range, etc.) propagate as `Err` instead of being reported as
    // success (Bug 4).
    if agent_op_tx.send(op).await.is_err() {
        return Err("agent_op channel closed".into());
    }
    match tokio::time::timeout(DISPATCH_REPLY_TIMEOUT, rx).await {
        Ok(Ok(Ok(_draw_result))) => Ok(()),
        Ok(Ok(Err(e))) => Err(format!("{e}")),
        Ok(Err(_)) => Err("op reply channel dropped".into()),
        Err(_) => Err("op timed out".into()),
    }
}
