//! Agent workspace server. Hosts a tokio runtime + MCP server that lets external
//! agents (Claude, Copilot, local LLMs) participate in the canvas alongside the
//! human user. All gated behind the `agent-server` cargo feature; this module
//! is invisible in builds without that feature.

pub mod discovery;
pub mod events;
pub mod handlers;
pub mod server;
pub mod snapshot;
pub mod types;
pub mod validation;

pub use server::AgentServer;

/// Drain up to `max` AgentOps from the channel and dispatch each one against
/// `state` on the main thread. Returns the number of ops processed.
pub fn drain_ops(state: &mut crate::state::AppState, max: usize) -> usize {
    let mut count = 0;
    loop {
        if count >= max {
            break;
        }
        let op = {
            let Some(rx) = state.agent_op_rx.as_mut() else {
                break;
            };
            rx.try_recv().ok()
        };
        match op {
            Some(op) => {
                handlers::dispatch(op, state);
                count += 1;
            }
            None => break,
        }
    }
    count
}
