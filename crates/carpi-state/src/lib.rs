//! The value store.
//!
//! A value is the value or nothing, plus when it was last set. Error/init raw
//! values decode to nothing; a value older than its timeout reads as nothing.
//! That's all the validity handling there is.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Num(f64),
    Text(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{b}"),
            Value::Num(n) => write!(f, "{n}"),
            Value::Text(s) => write!(f, "{s}"),
        }
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Num(n)
    }
}

#[derive(Clone, Debug)]
struct Entry {
    value: Option<Value>,
    at: Instant,
    timeout: Duration,
}

/// One row of a snapshot. `value` is `None` if the source sent an error/init
/// value or the value has timed out.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub name: String,
    pub value: Option<Value>,
    pub age: Duration,
}

#[derive(Default)]
pub struct Store {
    entries: BTreeMap<String, Entry>,
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: &str, value: Option<Value>, at: Instant, timeout: Duration) {
        match self.entries.get_mut(name) {
            Some(e) => {
                e.value = value;
                e.at = at;
                e.timeout = timeout;
            }
            None => {
                self.entries
                    .insert(name.to_string(), Entry { value, at, timeout });
            }
        }
    }

    /// The current value, or `None` if unknown, invalid or timed out.
    pub fn get(&self, name: &str, now: Instant) -> Option<&Value> {
        let e = self.entries.get(name)?;
        if now.saturating_duration_since(e.at) > e.timeout {
            return None;
        }
        e.value.as_ref()
    }

    pub fn get_bool(&self, name: &str, now: Instant) -> Option<bool> {
        match self.get(name, now)? {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn get_num(&self, name: &str, now: Instant) -> Option<f64> {
        match self.get(name, now)? {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// Time since the value was last set, whatever it was.
    pub fn age(&self, name: &str, now: Instant) -> Option<Duration> {
        self.entries
            .get(name)
            .map(|e| now.saturating_duration_since(e.at))
    }

    /// Every known value whose name starts with `prefix`, sorted by name.
    pub fn snapshot(&self, prefix: &str, now: Instant) -> Vec<Reading> {
        self.entries
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, e)| {
                let age = now.saturating_duration_since(e.at);
                let value = if age > e.timeout {
                    None
                } else {
                    e.value.clone()
                };
                Reading {
                    name: k.clone(),
                    value,
                    age,
                }
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_out() {
        let t0 = Instant::now();
        let mut s = Store::new();
        s.set(
            "vehicle.speed",
            Some(Value::Num(50.0)),
            t0,
            Duration::from_secs(1),
        );
        assert_eq!(
            s.get_num("vehicle.speed", t0 + Duration::from_millis(900)),
            Some(50.0)
        );
        assert_eq!(
            s.get_num("vehicle.speed", t0 + Duration::from_millis(1100)),
            None
        );
        assert_eq!(
            s.age("vehicle.speed", t0 + Duration::from_secs(2)),
            Some(Duration::from_secs(2))
        );
    }

    #[test]
    fn invalid_is_none() {
        let t0 = Instant::now();
        let mut s = Store::new();
        s.set("x", Some(Value::Bool(true)), t0, Duration::from_secs(1));
        s.set("x", None, t0, Duration::from_secs(1));
        assert_eq!(s.get("x", t0), None);
    }

    #[test]
    fn snapshot_prefix() {
        let t0 = Instant::now();
        let mut s = Store::new();
        let d = Duration::from_secs(1);
        s.set("a.b", Some(Value::Num(1.0)), t0, d);
        s.set("light.x", Some(Value::Num(2.0)), t0, d);
        s.set("light.y", Some(Value::Num(3.0)), t0, d);
        s.set("lights.z", Some(Value::Num(4.0)), t0, d);
        let snap = s.snapshot("light.", t0);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].name, "light.x");
        assert_eq!(s.snapshot("", t0).len(), 4);
    }
}
