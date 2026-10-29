//! Exponential backoff for reconnect loops.

use std::time::Duration;

pub struct Backoff {
    min: Duration,
    max: Duration,
    cur: Duration,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Backoff {
        Backoff { min, max, cur: min }
    }

    /// The delay to wait now; the following call returns twice as much, up to `max`.
    pub fn next(&mut self) -> Duration {
        let d = self.cur;
        self.cur = (self.cur * 2).min(self.max);
        d
    }

    pub fn reset(&mut self) {
        self.cur = self.min;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_caps_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
        let got: Vec<u64> = (0..8).map(|_| b.next().as_secs()).collect();
        assert_eq!(got, vec![1, 2, 4, 8, 16, 32, 60, 60]);
        b.reset();
        assert_eq!(b.next(), Duration::from_secs(1));
    }
}
