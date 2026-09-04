//! Simulated clock (spec §8.3). Every provider-facing timestamp is sim
//! time; `api_requests`/log timestamps are wall time. Cheap to `Clone` —
//! shared via an `Arc<RwLock<_>>` across `AppState` and the ticker task.

use std::sync::{Arc, RwLock};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};

struct ClockState {
    epoch: DateTime<Utc>,
    started_at: Instant,
    multiplier: f64,
}

fn compute(state: &ClockState) -> DateTime<Utc> {
    let elapsed_sim_secs = state.started_at.elapsed().as_secs_f64() * state.multiplier;
    state.epoch + Duration::milliseconds((elapsed_sim_secs * 1000.0) as i64)
}

#[derive(Clone)]
pub struct SimClock(Arc<RwLock<ClockState>>);

impl SimClock {
    /// `multiplier` is simulated seconds per wall second; `0.0` pauses.
    pub fn new(epoch: DateTime<Utc>, multiplier: f64) -> Self {
        Self(Arc::new(RwLock::new(ClockState {
            epoch,
            started_at: Instant::now(),
            multiplier,
        })))
    }

    pub fn now(&self) -> DateTime<Utc> {
        compute(&self.0.read().unwrap())
    }

    pub fn multiplier(&self) -> f64 {
        self.0.read().unwrap().multiplier
    }

    /// Rebases the clock at the current sim time before changing speed, so
    /// time already elapsed under the old multiplier isn't lost or redone.
    pub fn set_multiplier(&self, multiplier: f64) {
        let mut state = self.0.write().unwrap();
        let now = compute(&state);
        state.epoch = now;
        state.started_at = Instant::now();
        state.multiplier = multiplier;
    }

    /// Jumps sim time forward (or backward) by `delta`, e.g. the
    /// dashboard's "advance 6 hours" button.
    pub fn jump(&self, delta: Duration) {
        let mut state = self.0.write().unwrap();
        let now = compute(&state);
        state.epoch = now + delta;
        state.started_at = Instant::now();
    }

    pub fn reset(&self, to: DateTime<Utc>) {
        let mut state = self.0.write().unwrap();
        state.epoch = to;
        state.started_at = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_advances_with_multiplier() {
        let epoch = Utc::now();
        let clock = SimClock::new(epoch, 3600.0);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(clock.now() > epoch);
    }

    #[test]
    fn zero_multiplier_pauses() {
        let epoch = Utc::now();
        let clock = SimClock::new(epoch, 0.0);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(clock.now(), epoch);
    }

    #[test]
    fn jump_moves_time_forward() {
        let epoch = Utc::now();
        let clock = SimClock::new(epoch, 0.0);
        clock.jump(Duration::hours(6));
        assert_eq!(clock.now(), epoch + Duration::hours(6));
    }

    #[test]
    fn set_multiplier_preserves_elapsed_time() {
        let epoch = Utc::now();
        let clock = SimClock::new(epoch, 0.0);
        clock.jump(Duration::hours(1));
        let before = clock.now();
        clock.set_multiplier(60.0);
        assert!((clock.now() - before).num_milliseconds().abs() < 50);
    }
}
