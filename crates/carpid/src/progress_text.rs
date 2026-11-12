//! RNS-E update progress on DIS top line 1 (0x363, 8 cells), twice a second:
//! `TX 47% /` = phase, the whole job's percent, a spinner that turns on every
//! update so a black screen still shows the Pi is working. The main loop puts
//! it on the top line ahead of everything else (`Car::set_update_text`).
use crate::Msg;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const PERIOD: Duration = Duration::from_millis(500);
const SPIN: [char; 4] = ['-', '\\', '|', '/'];
/// The result stays up this long, then the top line goes back to its sources.
const RESULT_HOLD: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
struct State {
    phase: &'static str,
    pct: u32,
    /// Percent `pct` -> `to` over `over` from `t0`: a step with no feedback (erase, program, reboot).
    ramp: Option<(Instant, u32, Duration)>,
}

impl State {
    fn pct(&self, now: Instant) -> u32 {
        match self.ramp {
            Some((t0, to, over)) => {
                let f = now.saturating_duration_since(t0).as_secs_f64() / over.as_secs_f64();
                self.pct + ((to - self.pct) as f64 * f.min(1.0)) as u32
            }
            None => self.pct,
        }
        .min(99)
    }
}

/// The line: two-letter phase, percent, spinner.
pub fn line(phase: &str, pct: u32, tick: usize) -> String {
    format!("{phase:<2.2}{pct:>3}% {}", SPIN[tick % SPIN.len()])
}

pub struct Progress {
    state: Arc<Mutex<State>>,
    tx: mpsc::Sender<Msg>,
    ticker: tokio::task::JoinHandle<()>,
    ended: bool,
}

impl Progress {
    pub fn start(tx: mpsc::Sender<Msg>) -> Self {
        let state = Arc::new(Mutex::new(State {
            phase: "CK",
            pct: 0,
            ramp: None,
        }));
        let (s, t) = (state.clone(), tx.clone());
        let ticker = tokio::spawn(async move {
            let mut iv = tokio::time::interval(PERIOD);
            for tick in 0usize.. {
                iv.tick().await;
                let st = *s.lock().unwrap();
                let text = line(st.phase, st.pct(Instant::now()), tick);
                if t.send(Msg::UpdateText(Some(text))).await.is_err() {
                    break;
                }
            }
        });
        Progress {
            state,
            tx,
            ticker,
            ended: false,
        }
    }

    pub fn set(&self, phase: &'static str, pct: u32) {
        *self.state.lock().unwrap() = State {
            phase,
            pct,
            ramp: None,
        };
    }

    /// `from` -> `to` percent over `over`, for a step that reports nothing.
    pub fn ramp(&self, phase: &'static str, from: u32, to: u32, over: Duration) {
        *self.state.lock().unwrap() = State {
            phase,
            pct: from,
            ramp: Some((Instant::now(), to.max(from), over)),
        };
    }

    /// Shows the result for a few seconds, then gives the line back.
    pub fn end(mut self, ok: bool) {
        self.ended = true;
        self.ticker.abort();
        let tx = self.tx.clone();
        let text = if ok { "UPD  OK" } else { "UPD FAIL" };
        tokio::spawn(async move {
            let _ = tx.send(Msg::UpdateText(Some(text.into()))).await;
            tokio::time::sleep(RESULT_HOLD).await;
            let _ = tx.send(Msg::UpdateText(None)).await;
        });
    }
}

impl Drop for Progress {
    /// A job that ends without `end` (panic, cancel) still gives the line back.
    fn drop(&mut self) {
        self.ticker.abort();
        if !self.ended {
            let _ = self.tx.try_send(Msg::UpdateText(None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_is_eight_cells_and_spins() {
        assert_eq!(line("TX", 47, 0), "TX 47% -");
        assert_eq!(line("FL", 5, 3), "FL  5% /");
        assert_eq!(line("TX", 99, 1).chars().count(), 8);
        assert_ne!(line("TX", 47, 0), line("TX", 47, 1));
    }

    #[test]
    fn ramp_climbs_and_stops_short_of_done() {
        let t0 = Instant::now();
        let s = State {
            phase: "FL",
            pct: 57,
            ramp: Some((t0, 94, Duration::from_secs(10))),
        };
        assert_eq!(s.pct(t0), 57);
        assert_eq!(s.pct(t0 + Duration::from_secs(5)), 75);
        assert_eq!(s.pct(t0 + Duration::from_secs(60)), 94);
        let done = State {
            phase: "VF",
            pct: 100,
            ramp: None,
        };
        assert_eq!(done.pct(t0), 99);
    }
}
