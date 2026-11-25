use pretty_assertions::assert_eq;

use super::PollBackoff;
use super::PollOutcome;

const BASE_MIN_MS: u64 = 5_000;
const MAX_MS: u64 = 300_000;

fn simulate(outcomes: &[PollOutcome], base_min_ms: u64, max_ms: u64) -> Vec<u64> {
    let mut backoff = PollBackoff::default();
    let mut waits = Vec::with_capacity(outcomes.len());
    for &outcome in outcomes {
        let empty_input = outcome != PollOutcome::Input;
        waits.push(backoff.next_min_yield_ms(empty_input, base_min_ms, max_ms));
        backoff.record(outcome);
    }
    waits
}

#[test]
fn empty_polls_double_after_three_consecutive_no_output_polls() {
    let outcomes = vec![PollOutcome::NoNewOutput; 7];

    let waits = simulate(&outcomes, BASE_MIN_MS, MAX_MS);

    assert_eq!(
        waits,
        vec![5_000, 5_000, 5_000, 10_000, 20_000, 40_000, 80_000]
    );
}

#[test]
fn backoff_is_capped_at_max() {
    let outcomes = vec![PollOutcome::NoNewOutput; 8];

    let waits = simulate(&outcomes, BASE_MIN_MS, 20_000);

    assert_eq!(
        waits,
        vec![5_000, 5_000, 5_000, 10_000, 20_000, 20_000, 20_000, 20_000]
    );
}

#[test]
fn new_output_resets_the_streak() {
    let outcomes = vec![
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
    ];

    let waits = simulate(&outcomes, BASE_MIN_MS, MAX_MS);

    assert_eq!(
        waits,
        vec![
            5_000, 5_000, 5_000, 10_000, 20_000, 5_000, 5_000, 5_000, 10_000
        ]
    );
}

#[test]
fn stdin_input_resets_the_streak() {
    let outcomes = vec![
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::Input,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
        PollOutcome::NoNewOutput,
    ];

    let waits = simulate(&outcomes, BASE_MIN_MS, MAX_MS);

    assert_eq!(
        waits,
        vec![
            5_000, 5_000, 5_000, 10_000, 5_000, 5_000, 5_000, 5_000, 10_000
        ]
    );
}
