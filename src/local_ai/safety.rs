//! AI safety rails for the local agent (Feature 5).
//!
//! Two self-contained mechanisms layered on top of the budget/role system:
//!
//! 1. A **circuit breaker** that watches for the human repeatedly undoing the
//!    agent's own actions. After `UNDO_TRIP_THRESHOLD` *consecutive* self-undos
//!    (a successful committed act resets the streak), the agent auto-mutes for
//!    a cooldown rather than continuing to fight the user.
//! 2. A **per-decision-pass cap** on destructive operations, so a single bad
//!    decision can't wipe out a large amount of work in one go.

use std::time::Duration;

/// Consecutive human-undos of the agent's own actions before the breaker trips.
pub const UNDO_TRIP_THRESHOLD: u32 = 3;

/// How long the agent stays auto-muted after the breaker trips.
pub const CIRCUIT_BREAKER_COOLDOWN: Duration = Duration::from_secs(300);

/// Maximum destructive ops the agent may perform in a single decision pass.
pub const MAX_DESTRUCTIVE_OPS_PER_PASS: usize = 2;

/// Returns true if `tool` mutates/removes existing canvas content destructively
/// and therefore must pass through the safety gate.
pub fn is_destructive(tool: &str) -> bool {
    matches!(
        tool,
        // Removes a shape outright.
        "delete_shape"
            // Bulk / state-rewinding operations.
            | "undo"
            | "redo"
            | "renumber_steps"
            // Relocates existing geometry.
            | "move_shape"
    )
}

/// Tracks consecutive human-undos of the agent's own work and decides when the
/// circuit breaker should trip. Self-contained; the agent loop owns one of these
/// behind a mutex.
#[derive(Debug, Default)]
pub struct CircuitBreaker {
    consecutive_self_undos: u32,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that the human undid one of the agent's shapes. Returns `true` if
    /// this pushes the streak to (or past) the trip threshold, meaning the
    /// caller should mute the agent for [`CIRCUIT_BREAKER_COOLDOWN`].
    pub fn record_self_undo(&mut self) -> bool {
        self.consecutive_self_undos = self.consecutive_self_undos.saturating_add(1);
        self.consecutive_self_undos >= UNDO_TRIP_THRESHOLD
    }

    /// The agent successfully committed an act the human did not (yet) reject —
    /// the consecutive-undo streak resets.
    pub fn record_commit(&mut self) {
        self.consecutive_self_undos = 0;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn consecutive_self_undos(&self) -> u32 {
        self.consecutive_self_undos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_classification() {
        assert!(is_destructive("delete_shape"));
        assert!(is_destructive("move_shape"));
        assert!(is_destructive("undo"));
        assert!(is_destructive("renumber_steps"));
        assert!(!is_destructive("draw_rect"));
        assert!(!is_destructive("draw_ask"));
        assert!(!is_destructive("set_prop"));
    }

    #[test]
    fn breaker_trips_after_three_consecutive_undos() {
        let mut cb = CircuitBreaker::new();
        assert!(!cb.record_self_undo(), "1st undo must not trip");
        assert!(!cb.record_self_undo(), "2nd undo must not trip");
        assert!(cb.record_self_undo(), "3rd consecutive undo trips");
        assert_eq!(cb.consecutive_self_undos(), 3);
    }

    #[test]
    fn commit_resets_the_streak() {
        let mut cb = CircuitBreaker::new();
        assert!(!cb.record_self_undo());
        assert!(!cb.record_self_undo());
        cb.record_commit();
        assert_eq!(cb.consecutive_self_undos(), 0);
        // After a reset it takes three fresh undos again to trip.
        assert!(!cb.record_self_undo());
        assert!(!cb.record_self_undo());
        assert!(cb.record_self_undo());
    }

    #[test]
    fn per_pass_cap_is_small() {
        assert!(MAX_DESTRUCTIVE_OPS_PER_PASS >= 1);
        assert!(MAX_DESTRUCTIVE_OPS_PER_PASS <= 3);
    }
}
