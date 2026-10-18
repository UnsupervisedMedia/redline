//! Pre-decision event shaping: drops self-events, coalesces same-shape bursts,
//! and waits for a short silence window before triggering a decision.
//! Rules only — no LLM gate. The decision pass handles "should I act?" as its
//! first weight.

use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::agent::events::ServerEvent;
use crate::types::ParticipantId;

/// Coalesce window: same-shape events within this period collapse to one.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(1500);
/// Default silence window before reacting to a burst of drawing activity.
pub const DEFAULT_QUIET_PERIOD: Duration = Duration::from_millis(1400);
/// Default longer idle-after-activity threshold for a follow-up pass.
pub const DEFAULT_IDLE_AFTER_ACTIVITY: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy)]
pub enum Verdict {
    Ignore,
    Proceed { reason: SalienceReason },
}

#[derive(Debug, Clone, Copy)]
pub enum SalienceReason {
    NewShape,
    IdleAfterActivity,
    Coalesced, // representative of a coalesced burst
}

pub struct EventFilter {
    self_pid: ParticipantId,
    /// shape_id → last-seen timestamp; events on a shape within COALESCE_WINDOW
    /// of the previous event for that shape are dropped.
    recent_per_shape: std::collections::HashMap<Uuid, Instant>,
    /// Last time any non-self event arrived.
    last_activity: Option<Instant>,
    /// Pending reason to emit once the canvas has been quiet long enough.
    pending_reason: Option<SalienceReason>,
    /// Activity timestamp we already emitted an idle follow-up for.
    last_idle_emitted_for: Option<Instant>,
    quiet_period: Duration,
    idle_after_activity: Duration,
}

impl EventFilter {
    pub fn new(self_pid: ParticipantId) -> Self {
        Self {
            self_pid,
            recent_per_shape: std::collections::HashMap::new(),
            last_activity: None,
            pending_reason: None,
            last_idle_emitted_for: None,
            quiet_period: DEFAULT_QUIET_PERIOD,
            idle_after_activity: DEFAULT_IDLE_AFTER_ACTIVITY,
        }
    }

    pub fn configure(&mut self, quiet_period: Duration, idle_after_activity: Duration) {
        self.quiet_period = quiet_period;
        self.idle_after_activity = idle_after_activity;
    }

    pub fn ingest(&mut self, evt: &ServerEvent, now: Instant) -> Verdict {
        // Drop self-events.
        if event_author(evt) == self.self_pid {
            return Verdict::Ignore;
        }
        self.last_activity = Some(now);
        self.last_idle_emitted_for = None;

        // Coalesce: drop same-shape events within window.
        if let Some(sid) = event_shape_id(evt) {
            if let Some(prev) = self.recent_per_shape.get(&sid) {
                if now.duration_since(*prev) < COALESCE_WINDOW {
                    self.recent_per_shape.insert(sid, now);
                    self.pending_reason.get_or_insert(SalienceReason::Coalesced);
                    return Verdict::Ignore;
                }
            }
            self.recent_per_shape.insert(sid, now);
        }

        let reason = if matches!(evt, ServerEvent::ShapeAdded { .. }) {
            SalienceReason::NewShape
        } else {
            SalienceReason::Coalesced
        };
        self.pending_reason = Some(match self.pending_reason {
            Some(SalienceReason::NewShape) => SalienceReason::NewShape,
            _ => reason,
        });
        Verdict::Ignore
    }

    /// Call periodically (e.g. every 1s on a separate timer). Emits once when a
    /// burst settles, and optionally once more on longer idle.
    pub fn tick(&mut self, now: Instant) -> Verdict {
        let Some(la) = self.last_activity else {
            return Verdict::Ignore;
        };
        if let Some(reason) = self.pending_reason {
            if now.duration_since(la) >= self.quiet_period {
                self.pending_reason = None;
                return Verdict::Proceed { reason };
            }
        }
        if now.duration_since(la) >= self.idle_after_activity
            && self.last_idle_emitted_for != Some(la)
        {
            self.last_idle_emitted_for = Some(la);
            return Verdict::Proceed {
                reason: SalienceReason::IdleAfterActivity,
            };
        }
        Verdict::Ignore
    }
}

fn event_author(evt: &ServerEvent) -> ParticipantId {
    match evt {
        ServerEvent::ShapeAdded { author, .. } => *author,
        ServerEvent::ShapeChanged { author, .. } => *author,
        ServerEvent::ShapeDeleted { author, .. } => *author,
        ServerEvent::ImageReplaced { author, .. } => *author,
        ServerEvent::LayerChanged { author, .. } => *author,
    }
}

fn event_shape_id(evt: &ServerEvent) -> Option<Uuid> {
    match evt {
        ServerEvent::ShapeAdded { shape, .. } => Some(shape.id),
        ServerEvent::ShapeChanged { shape_id, .. } => Some(*shape_id),
        ServerEvent::ShapeDeleted { shape_id, .. } => Some(*shape_id),
        ServerEvent::ImageReplaced { .. } => None,
        ServerEvent::LayerChanged { node_id, .. } => Some(*node_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::events::ServerEvent;
    use crate::agent::types::AnnotationDescriptor;

    fn other() -> ParticipantId {
        ParticipantId::new()
    }
    fn me() -> ParticipantId {
        ParticipantId::new()
    }

    fn shape_added(author: ParticipantId, id: Uuid) -> ServerEvent {
        ServerEvent::ShapeAdded {
            room_id: Uuid::nil(),
            seq: 1,
            author,
            shape: AnnotationDescriptor {
                id,
                kind: "rect".into(),
                bounding_rect: egui::Rect::ZERO,
                color: egui::Color32::BLACK,
                label: None,
                locked: false,
                visible: true,
                author,
                layer: None,
                layer_name: None,
            },
        }
    }

    #[test]
    fn self_events_dropped() {
        let me = me();
        let mut f = EventFilter::new(me);
        let evt = shape_added(me, Uuid::new_v4());
        assert!(matches!(f.ingest(&evt, Instant::now()), Verdict::Ignore));
    }

    #[test]
    fn same_shape_burst_coalesces() {
        let me = me();
        let other = other();
        let mut f = EventFilter::new(me);
        let sid = Uuid::new_v4();
        let now = Instant::now();
        assert!(matches!(
            f.ingest(&shape_added(other, sid), now),
            Verdict::Ignore
        ));
        // Second event on same shape 500ms later — coalesce.
        let later = now + Duration::from_millis(500);
        assert!(matches!(
            f.ingest(&shape_added(other, sid), later),
            Verdict::Ignore
        ));
        let settled = later + DEFAULT_QUIET_PERIOD + Duration::from_millis(10);
        assert!(matches!(f.tick(settled), Verdict::Proceed { .. }));
    }

    #[test]
    fn different_shapes_wait_for_quiet_before_reacting() {
        let me = me();
        let other = other();
        let mut f = EventFilter::new(me);
        let now = Instant::now();
        assert!(matches!(
            f.ingest(&shape_added(other, Uuid::new_v4()), now),
            Verdict::Ignore
        ));
        let soon = now + Duration::from_millis(500);
        assert!(matches!(
            f.ingest(&shape_added(other, Uuid::new_v4()), soon),
            Verdict::Ignore
        ));
        assert!(matches!(
            f.tick(soon + Duration::from_millis(500)),
            Verdict::Ignore
        ));
        let after = soon + DEFAULT_QUIET_PERIOD + Duration::from_millis(10);
        assert!(matches!(f.tick(after), Verdict::Proceed { .. }));
    }

    #[test]
    fn idle_tick_fires_once_after_long_silence() {
        let me = me();
        let other = other();
        let mut f = EventFilter::new(me);
        let now = Instant::now();
        // Some activity.
        let _ = f.ingest(&shape_added(other, Uuid::new_v4()), now);
        let settled = now + DEFAULT_QUIET_PERIOD + Duration::from_millis(10);
        assert!(matches!(f.tick(settled), Verdict::Proceed { .. }));
        let idle_time = now + DEFAULT_IDLE_AFTER_ACTIVITY + Duration::from_millis(10);
        assert!(matches!(
            f.tick(idle_time),
            Verdict::Proceed {
                reason: SalienceReason::IdleAfterActivity
            }
        ));
        // Won't refire immediately.
        assert!(matches!(
            f.tick(idle_time + Duration::from_millis(100)),
            Verdict::Ignore
        ));
    }
}
