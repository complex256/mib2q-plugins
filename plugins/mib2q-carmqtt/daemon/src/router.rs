//! Merges updates from all sources into one value per signal and throttles them.

use std::collections::HashMap;
use std::time::Instant;

use crate::catalog::Rate;
use crate::debug;
use crate::value::{SignalId, SourceId, Update, Value};

struct Sent {
    value: Value,
    at: Instant,
    /// The newest selected value not yet forwarded.
    pending: Option<Value>,
}

pub struct Router {
    rates: HashMap<SignalId, Rate>,
    priorities: HashMap<SourceId, i32>,
    inputs: HashMap<SignalId, Vec<(SourceId, Value)>>,
    sent: HashMap<SignalId, Sent>,
}

impl Router {
    pub fn new(rates: HashMap<SignalId, Rate>, priorities: HashMap<SourceId, i32>) -> Router {
        Router { rates, priorities, inputs: HashMap::new(), sent: HashMap::new() }
    }

    /// Highest-priority source with an available value; `Unavailable` if none.
    fn select(&self, signal: SignalId) -> Value {
        let Some(inputs) = self.inputs.get(signal) else { return Value::Unavailable };
        let mut best: Option<(i32, &Value)> = None;
        for (src, v) in inputs {
            if !v.is_available() {
                continue;
            }
            let p = self.priorities.get(src).copied().unwrap_or(0);
            if best.map(|(bp, _)| p > bp).unwrap_or(true) {
                best = Some((p, v));
            }
        }
        best.map(|(_, v)| v.clone()).unwrap_or(Value::Unavailable)
    }

    pub fn ingest(&mut self, u: Update, now: Instant) -> Option<(SignalId, Value)> {
        let Some(rate) = self.rates.get(u.signal).copied() else {
            debug!("dropping update for unknown signal {}", u.signal);
            return None;
        };
        if !rate.enabled {
            return None;
        }
        let inputs = self.inputs.entry(u.signal).or_default();
        if u.value == Value::Lost {
            // The source went away (not "no data"): forget its input. If no
            // other source still has one, keep what was last forwarded.
            inputs.retain(|(s, _)| *s != u.source);
            if inputs.is_empty() {
                if let Some(sent) = self.sent.get_mut(u.signal) {
                    sent.pending = None;
                }
                return None;
            }
        } else {
            match inputs.iter_mut().find(|(s, _)| *s == u.source) {
                Some(slot) => slot.1 = u.value,
                None => inputs.push((u.source, u.value)),
            }
        }
        let selected = self.select(u.signal);
        let Some(sent) = self.sent.get_mut(u.signal) else {
            self.sent.insert(u.signal, Sent { value: selected.clone(), at: now, pending: None });
            return Some((u.signal, selected));
        };
        if sent.value == selected {
            sent.pending = None;
            return None;
        }
        let due = now.duration_since(sent.at) >= rate.min_interval;
        if bypasses_throttle(&sent.value, &selected) || (due && beyond_deadband(&sent.value, &selected, rate.deadband)) {
            sent.value = selected.clone();
            sent.at = now;
            sent.pending = None;
            Some((u.signal, selected))
        } else {
            sent.pending = Some(selected);
            None
        }
    }

    pub fn tick(&mut self, now: Instant) -> Vec<(SignalId, Value)> {
        let mut out = Vec::new();
        for (signal, sent) in self.sent.iter_mut() {
            let Some(pending) = sent.pending.as_ref() else { continue };
            let Some(rate) = self.rates.get(signal) else { continue };
            let since = now.duration_since(sent.at);
            if since >= rate.min_interval && (beyond_deadband(&sent.value, pending, rate.deadband) || since >= rate.max_interval) {
                let v = sent.pending.take().expect("checked above");
                sent.value = v.clone();
                sent.at = now;
                out.push((*signal, v));
            }
        }
        out
    }
}

/// Availability flips, unit changes and non-numeric changes are never held back.
fn bypasses_throttle(old: &Value, new: &Value) -> bool {
    match (old, new) {
        (Value::Num { unit: a, .. }, Value::Num { unit: b, .. }) => a != b,
        (Value::Geo { .. }, Value::Geo { .. }) => false,
        _ => true,
    }
}

fn beyond_deadband(old: &Value, new: &Value, deadband: f64) -> bool {
    match (old, new) {
        (Value::Num { v: a, .. }, Value::Num { v: b, .. }) => (b - a).abs() >= deadband && a != b,
        _ => old != new,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Rate;
    use crate::value::Unit;
    use std::time::Duration;

    fn num(v: f64) -> Value {
        Value::Num { v, unit: Some(Unit::Mph) }
    }

    fn upd(signal: SignalId, source: SourceId, value: Value) -> Update {
        Update { signal, value, ts: None, source }
    }

    fn router() -> Router {
        let rates = HashMap::from([
            ("vehicle.speed", Rate::throttled(5, 1.0)),
            ("odometer", Rate::STATUS),
            ("parking_brake", Rate::throttled(5, 0.0)),
            ("accel.lateral", Rate::off(1, 0.1)),
        ]);
        Router::new(rates, HashMap::from([("exlap", 10), ("dsi", 20)]))
    }

    #[test]
    fn first_value_forwards_and_repeats_do_not() {
        let mut r = router();
        let t = Instant::now();
        assert_eq!(r.ingest(upd("odometer", "exlap", num(1.0)), t), Some(("odometer", num(1.0))));
        assert_eq!(r.ingest(upd("odometer", "exlap", num(1.0)), t), None);
        assert_eq!(r.ingest(upd("odometer", "exlap", num(2.0)), t), Some(("odometer", num(2.0))));
    }

    #[test]
    fn unknown_and_disabled_signals_are_dropped() {
        let mut r = router();
        let t = Instant::now();
        assert_eq!(r.ingest(upd("nope", "exlap", num(1.0)), t), None);
        assert_eq!(r.ingest(upd("accel.lateral", "exlap", num(1.0)), t), None);
    }

    #[test]
    fn higher_priority_wins_and_falls_back() {
        let mut r = router();
        let t = Instant::now();
        r.ingest(upd("odometer", "exlap", num(10.0)), t);
        assert_eq!(r.ingest(upd("odometer", "dsi", num(11.0)), t), Some(("odometer", num(11.0))));
        // lower-priority source changing does not override
        assert_eq!(r.ingest(upd("odometer", "exlap", num(12.0)), t), None);
        // dsi goes away -> exlap's value
        assert_eq!(r.ingest(upd("odometer", "dsi", Value::Unavailable), t), Some(("odometer", num(12.0))));
        // both gone -> unavailable
        assert_eq!(r.ingest(upd("odometer", "exlap", Value::Unavailable), t), Some(("odometer", Value::Unavailable)));
    }

    #[test]
    fn min_interval_holds_then_tick_releases() {
        let mut r = router();
        let t0 = Instant::now();
        assert!(r.ingest(upd("vehicle.speed", "exlap", num(30.0)), t0).is_some());
        assert_eq!(r.ingest(upd("vehicle.speed", "exlap", num(40.0)), t0 + Duration::from_secs(1)), None);
        assert!(r.tick(t0 + Duration::from_secs(3)).is_empty());
        assert_eq!(r.tick(t0 + Duration::from_secs(5)), vec![("vehicle.speed", num(40.0))]);
        assert!(r.tick(t0 + Duration::from_secs(6)).is_empty());
    }

    #[test]
    fn deadband_holds_until_max_interval() {
        let mut r = router();
        let t0 = Instant::now();
        r.ingest(upd("vehicle.speed", "exlap", num(30.0)), t0);
        assert_eq!(r.ingest(upd("vehicle.speed", "exlap", num(30.5)), t0 + Duration::from_secs(10)), None);
        assert!(r.tick(t0 + Duration::from_secs(10)).is_empty());
        assert_eq!(r.tick(t0 + Duration::from_secs(900)), vec![("vehicle.speed", num(30.5))]);
    }

    #[test]
    fn bools_and_availability_bypass_throttle() {
        let mut r = router();
        let t0 = Instant::now();
        r.ingest(upd("parking_brake", "exlap", Value::Bool(true)), t0);
        let t1 = t0 + Duration::from_millis(100);
        assert_eq!(r.ingest(upd("parking_brake", "exlap", Value::Bool(false)), t1), Some(("parking_brake", Value::Bool(false))));
        r.ingest(upd("vehicle.speed", "exlap", num(30.0)), t0);
        assert_eq!(r.ingest(upd("vehicle.speed", "exlap", Value::Unavailable), t1), Some(("vehicle.speed", Value::Unavailable)));
    }

    #[test]
    fn lost_source_keeps_last_value() {
        let mut r = router();
        let t = Instant::now();
        r.ingest(upd("odometer", "exlap", num(10.0)), t);
        assert_eq!(r.ingest(upd("odometer", "exlap", Value::Lost), t), None);
        // back again with the same value: nothing new to publish
        assert_eq!(r.ingest(upd("odometer", "exlap", num(10.0)), t), None);
        assert_eq!(r.ingest(upd("odometer", "exlap", num(11.0)), t), Some(("odometer", num(11.0))));
    }

    #[test]
    fn lost_source_falls_back_to_another() {
        let mut r = router();
        let t = Instant::now();
        r.ingest(upd("odometer", "exlap", num(10.0)), t);
        r.ingest(upd("odometer", "dsi", num(11.0)), t);
        assert_eq!(r.ingest(upd("odometer", "dsi", Value::Lost), t), Some(("odometer", num(10.0))));
        assert_eq!(r.ingest(upd("odometer", "exlap", Value::Lost), t), None);
    }

    #[test]
    fn unit_change_bypasses_throttle() {
        let mut r = router();
        let t0 = Instant::now();
        r.ingest(upd("vehicle.speed", "exlap", num(30.0)), t0);
        let kmh = Value::Num { v: 30.5, unit: Some(Unit::Kmh) };
        assert_eq!(r.ingest(upd("vehicle.speed", "exlap", kmh.clone()), t0 + Duration::from_millis(10)), Some(("vehicle.speed", kmh)));
    }
}
