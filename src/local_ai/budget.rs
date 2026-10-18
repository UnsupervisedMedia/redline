//! Token-bucket budget for the local AI's act/ask/bookmark pools.
//! Refills on a timer; user undos of own work penalize subsequent acts.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct BudgetConfig {
    pub act_max: u32,
    pub act_refill_secs: u32,
    pub ask_max: u32,
    pub ask_refill_secs: u32,
    pub bookmark_max: u32,
    pub bookmark_refill_secs: u32,
}

#[derive(Debug)]
pub struct BudgetPool {
    cfg: BudgetConfig,
    act: u32,
    ask: u32,
    bookmark: u32,
    /// Per-pool refill anchors. Each advances only by the time actually
    /// consumed by whole refills, so sub-`refill_secs` elapsed time is carried
    /// forward instead of discarded (Bug 3).
    act_last_refill: Instant,
    ask_last_refill: Instant,
    bookmark_last_refill: Instant,
    undo_penalty_until: Option<Instant>,
    actions_at_double_cost_remaining: u32,
}

impl BudgetPool {
    pub fn new(cfg: BudgetConfig) -> Self {
        let now = Instant::now();
        Self {
            cfg,
            act: cfg.act_max,
            ask: cfg.ask_max,
            bookmark: cfg.bookmark_max,
            act_last_refill: now,
            ask_last_refill: now,
            bookmark_last_refill: now,
            undo_penalty_until: None,
            actions_at_double_cost_remaining: 0,
        }
    }

    pub fn config(&self) -> BudgetConfig {
        self.cfg
    }

    pub fn snapshot(&self) -> (u32, u32, u32) {
        (self.act, self.ask, self.bookmark)
    }

    pub fn refill_due(&mut self, now: Instant) {
        // Timer-based refill — each pool refills 1 token per its `refill_secs`.
        //
        // Bug 3 fix: `refill_due` is called ~every second while `refill_secs`
        // is typically > 1, so for any individual call the number of whole
        // refills is 0. The previous implementation reset `last_refill = now`
        // unconditionally, discarding the accumulated sub-`refill_secs` time so
        // pools never refilled after the initial budget. We now advance each
        // pool's anchor only by the time the granted tokens actually consumed
        // (`add * refill_secs`), carrying the remainder forward to the next call.
        Self::refill_pool(
            &mut self.act,
            self.cfg.act_max,
            self.cfg.act_refill_secs,
            &mut self.act_last_refill,
            now,
        );
        Self::refill_pool(
            &mut self.ask,
            self.cfg.ask_max,
            self.cfg.ask_refill_secs,
            &mut self.ask_last_refill,
            now,
        );
        Self::refill_pool(
            &mut self.bookmark,
            self.cfg.bookmark_max,
            self.cfg.bookmark_refill_secs,
            &mut self.bookmark_last_refill,
            now,
        );
    }

    /// Grant whole-interval refills to a single pool, advancing its anchor only
    /// by the time consumed so leftover elapsed time is preserved.
    fn refill_pool(pool: &mut u32, max: u32, refill_secs: u32, anchor: &mut Instant, now: Instant) {
        let refill_secs = refill_secs.max(1);
        let elapsed_secs = now.saturating_duration_since(*anchor).as_secs() as u32;
        let add = elapsed_secs / refill_secs;
        if add == 0 {
            return;
        }
        *pool = (*pool + add).min(max);
        *anchor += Duration::from_secs((add * refill_secs) as u64);
    }

    /// Try to spend an action token. Returns false if not enough budget.
    /// `propose_cost_half` consumes 0.5 tokens (rounded up = 1 every other call).
    pub fn try_spend_act(&mut self, propose_cost_half: bool) -> bool {
        let cost = self.action_cost(propose_cost_half);
        if self.act < cost {
            return false;
        }
        self.act -= cost;
        if self.actions_at_double_cost_remaining > 0 {
            self.actions_at_double_cost_remaining -= 1;
        }
        true
    }

    pub fn try_spend_ask(&mut self) -> bool {
        if self.ask == 0 {
            return false;
        }
        self.ask -= 1;
        true
    }

    pub fn try_spend_bookmark(&mut self) -> bool {
        if self.bookmark == 0 {
            return false;
        }
        self.bookmark -= 1;
        true
    }

    /// User undid one of the agent's own shapes. Next N actions cost 2 tokens each.
    pub fn record_undo_of_self(&mut self) {
        self.actions_at_double_cost_remaining = 3;
        self.undo_penalty_until = Some(Instant::now() + Duration::from_secs(60));
    }

    fn action_cost(&self, propose_cost_half: bool) -> u32 {
        let base = if propose_cost_half {
            // 0.5 rounded up = 1; the half-cost is realized over many calls
            // via a deterministic flip-flop. For simplicity, we count it as 1
            // and let the role config absorb the rest (this is a v1 simplification;
            // exact 0.5 accounting deferred).
            1
        } else {
            1
        };
        if self.actions_at_double_cost_remaining > 0 {
            base * 2
        } else {
            base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn cfg() -> BudgetConfig {
        BudgetConfig {
            act_max: 5,
            act_refill_secs: 1,
            ask_max: 5,
            ask_refill_secs: 1,
            bookmark_max: 5,
            bookmark_refill_secs: 1,
        }
    }

    #[test]
    fn act_consumes_then_runs_out() {
        let mut p = BudgetPool::new(cfg());
        for _ in 0..5 {
            assert!(p.try_spend_act(false));
        }
        assert!(!p.try_spend_act(false));
    }

    #[test]
    fn undo_penalty_doubles_next_three() {
        let mut p = BudgetPool::new(cfg());
        p.record_undo_of_self();
        // Each of the next 3 acts costs 2.
        for _ in 0..2 {
            assert!(p.try_spend_act(false));
        }
        // After 2 acts at cost 2, budget is 5 - 4 = 1. Next costs 2, fails.
        assert!(!p.try_spend_act(false));
    }

    #[test]
    fn refill_caps_at_max() {
        let mut p = BudgetPool::new(cfg());
        for _ in 0..5 {
            p.try_spend_act(false);
        }
        assert_eq!(p.snapshot().0, 0);
        sleep(std::time::Duration::from_millis(1100));
        p.refill_due(Instant::now());
        // Should have refilled at least 1, but not exceed cfg.act_max.
        let (a, _, _) = p.snapshot();
        assert!(a >= 1 && a <= 5);
    }

    /// Realistic config: act_refill_secs = 30 (> 1). Bug 3 regression test —
    /// with `refill_due` called every 1s, exactly one act token must be added
    /// after 30s, and the leftover seconds must not be discarded.
    #[test]
    fn refill_over_time_with_realistic_interval() {
        let cfg = BudgetConfig {
            act_max: 10,
            act_refill_secs: 30,
            ask_max: 10,
            ask_refill_secs: 30,
            bookmark_max: 30,
            bookmark_refill_secs: 10,
        };
        let mut p = BudgetPool::new(cfg);
        for _ in 0..10 {
            assert!(p.try_spend_act(false));
        }
        assert_eq!(p.snapshot().0, 0);
        // Drain bookmarks too so we can observe their independent refill cadence.
        for _ in 0..30 {
            assert!(p.try_spend_bookmark());
        }
        assert_eq!(p.snapshot().2, 0);

        let start = Instant::now();
        // Simulate 29 one-second ticks: not enough for a whole act refill yet,
        // but the accumulated time must NOT be thrown away.
        for s in 1..=29 {
            p.refill_due(start + Duration::from_secs(s));
        }
        assert_eq!(p.snapshot().0, 0, "no act token before 30s");

        // At exactly 30s, exactly one act token is granted.
        p.refill_due(start + Duration::from_secs(30));
        assert_eq!(p.snapshot().0, 1, "exactly 1 act token after 30s");

        // Bookmark refills every 10s → 3 by the 30s mark, capped at max.
        assert_eq!(p.snapshot().2, 3, "3 bookmark tokens after 30s");

        // Another 30s yields a second act token; remainder was carried, not lost.
        for s in 31..=60 {
            p.refill_due(start + Duration::from_secs(s));
        }
        assert_eq!(p.snapshot().0, 2, "exactly 2 act tokens after 60s");
    }

    /// A single large jump should add multiple tokens, still capped at max.
    #[test]
    fn refill_large_jump_caps_at_max() {
        let cfg = BudgetConfig {
            act_max: 5,
            act_refill_secs: 30,
            ask_max: 5,
            ask_refill_secs: 30,
            bookmark_max: 5,
            bookmark_refill_secs: 10,
        };
        let mut p = BudgetPool::new(cfg);
        for _ in 0..5 {
            assert!(p.try_spend_act(false));
        }
        let start = Instant::now();
        // 10 minutes elapsed at once → would be 20 refills, capped at 5.
        p.refill_due(start + Duration::from_secs(600));
        assert_eq!(p.snapshot().0, 5);
    }
}
