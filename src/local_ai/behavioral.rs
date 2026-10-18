//! Computes the `behavioral_summary` block injected into the decision prompt.
//! Lets the agent reason explicitly about its own recent track record.

use std::time::{Duration, Instant};

use crate::local_ai::budget::BudgetPool;
use crate::local_ai::roles::AgentRole;

#[derive(Debug, Clone, Default)]
pub struct BehavioralCounters {
    pub acts_committed: u32,
    pub acts_undone_by_human: u32,
    pub abstentions: u32,
    /// Ring of (instant, event_kind) for windowed reporting.
    pub recent: std::collections::VecDeque<(Instant, BehavioralEventKind)>,
}

#[derive(Debug, Clone, Copy)]
pub enum BehavioralEventKind {
    ActCommitted,
    ActUndoneByHuman,
    Abstention,
}

impl BehavioralCounters {
    pub fn record(&mut self, kind: BehavioralEventKind, now: Instant) {
        match kind {
            BehavioralEventKind::ActCommitted => self.acts_committed += 1,
            BehavioralEventKind::ActUndoneByHuman => self.acts_undone_by_human += 1,
            BehavioralEventKind::Abstention => self.abstentions += 1,
        }
        self.recent.push_back((now, kind));
        // Cap ring at 200 entries.
        while self.recent.len() > 200 {
            self.recent.pop_front();
        }
    }

    pub fn window_counts(&self, since: Instant) -> BehavioralCounters {
        let mut out = BehavioralCounters::default();
        for (ts, kind) in &self.recent {
            if *ts >= since {
                out.record(*kind, *ts);
            }
        }
        out
    }
}

pub fn render_summary(
    counters: &BehavioralCounters,
    budget: &BudgetPool,
    role: AgentRole,
    now: Instant,
    open_wishes: usize,
    recent_lessons: usize,
) -> String {
    let last_hour = counters.window_counts(now - Duration::from_secs(3600));
    let (act, ask, bm) = budget.snapshot();
    let cfg = budget.config();

    format!(
        "behavioral_summary:\n  last_hour:\n    acts_committed: {}\n    acts_undone_by_human: {}\n    abstentions: {}\n  current_mode: \"{}\"\n  current_budgets: {{ act: {}/{}, ask: {}/{}, bookmark: {}/{} }}\n  open_wishes: {}\n  recent_lessons: {}\n",
        last_hour.acts_committed,
        last_hour.acts_undone_by_human,
        last_hour.abstentions,
        role.mode_label(),
        act, cfg.act_max,
        ask, cfg.ask_max,
        bm, cfg.bookmark_max,
        open_wishes,
        recent_lessons,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_ai::budget::BudgetConfig;

    #[test]
    fn render_summary_includes_role_and_budgets() {
        let mut counters = BehavioralCounters::default();
        let now = Instant::now();
        counters.record(BehavioralEventKind::ActCommitted, now);
        let pool = BudgetPool::new(BudgetConfig {
            act_max: 10,
            act_refill_secs: 30,
            ask_max: 10,
            ask_refill_secs: 30,
            bookmark_max: 30,
            bookmark_refill_secs: 10,
        });
        let s = render_summary(&counters, &pool, AgentRole::CollaboratorLight, now, 0, 0);
        assert!(s.contains("Interactive"));
        assert!(s.contains("acts_committed: 1"));
    }
}
