//! Background task that auto-expires provisional (ghost) annotations.

use std::time::Duration;
use tokio::sync::mpsc::Sender as MpscSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::types::AgentOp;
use crate::types::ParticipantId;

/// Tick interval — how often we scan for expired provisionals.
pub const TICK: Duration = Duration::from_secs(1);

/// Spawn a tokio task that periodically asks for the list of expired provisional
/// annotation IDs (via a snapshot AgentOp) and issues delete commands for them.
///
/// `actor` is the participant id of the agent that owns the provisionals. The
/// delete is performed as if the agent itself retracted; this keeps the undo
/// stack consistent (the agent's own redo stack catches the deletion).
pub fn spawn(
    agent_op_tx: MpscSender<AgentOp>,
    actor: ParticipantId,
    room_id: Uuid,
    cancel: CancellationToken,
    runtime: tokio::runtime::Handle,
) -> tokio::task::JoinHandle<()> {
    runtime.spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {},
                _ = cancel.cancelled() => break,
            }
            // Snapshot to find expired ids.
            let (tx, rx) = tokio::sync::oneshot::channel();
            let req = AgentOp::ListExpiredProvisionals { room_id, reply: tx };
            if agent_op_tx.send(req).await.is_err() {
                break;
            }
            let ids = match rx.await {
                Ok(Ok(v)) => v,
                _ => continue,
            };
            for id in ids {
                let (dtx, _drx) = tokio::sync::oneshot::channel();
                let del = AgentOp::DeleteShape {
                    room_id,
                    actor,
                    shape_id: id,
                    reply: dtx,
                };
                if agent_op_tx.send(del).await.is_err() {
                    break;
                }
                // Reply intentionally discarded — best-effort cleanup.
            }
        }
    })
}
