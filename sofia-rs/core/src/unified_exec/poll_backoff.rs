//! Backoff for repeated empty `write_stdin` polls.
//!
//! Models sometimes poll a long-running process over and over with empty input
//! and no new output. Once a process has produced
//! [`EMPTY_POLLS_BEFORE_BACKOFF`] consecutive such polls, every further empty
//! poll doubles the effective minimum yield time so polling slows down and the
//! model is nudged to wait longer. Any stdin input, new output, or process exit
//! resets the streak.

/// Consecutive empty, no-output polls before the backoff starts.
pub(crate) const EMPTY_POLLS_BEFORE_BACKOFF: u32 = 3;

/// Outcome of a completed `write_stdin` poll, used to maintain [`PollBackoff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollOutcome {
    /// The poll sent non-empty stdin input.
    Input,
    /// An empty poll observed new output.
    NewOutput,
    /// An empty poll observed no new output.
    NoNewOutput,
}

/// Tracks consecutive empty, no-output `write_stdin` polls for one process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PollBackoff {
    consecutive_empty_polls: u32,
}

impl PollBackoff {
    /// Minimum yield to use for the upcoming poll before it runs.
    ///
    /// Only empty-input polls back off. The first
    /// [`EMPTY_POLLS_BEFORE_BACKOFF`] polls use `base_min_ms`; each following
    /// poll doubles it, capped at `max_ms`.
    pub(crate) fn next_min_yield_ms(
        &self,
        empty_input: bool,
        base_min_ms: u64,
        max_ms: u64,
    ) -> u64 {
        if !empty_input {
            return base_min_ms.min(max_ms);
        }
        let next_count = self.consecutive_empty_polls.saturating_add(1);
        let doublings = next_count.saturating_sub(EMPTY_POLLS_BEFORE_BACKOFF);
        let factor = 1u64.checked_shl(doublings).unwrap_or(u64::MAX);
        base_min_ms.saturating_mul(factor).min(max_ms)
    }

    /// Record the outcome of a completed poll.
    pub(crate) fn record(&mut self, outcome: PollOutcome) {
        match outcome {
            PollOutcome::Input | PollOutcome::NewOutput => {
                self.consecutive_empty_polls = 0;
            }
            PollOutcome::NoNewOutput => {
                self.consecutive_empty_polls = self.consecutive_empty_polls.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
#[path = "poll_backoff_tests.rs"]
mod tests;
