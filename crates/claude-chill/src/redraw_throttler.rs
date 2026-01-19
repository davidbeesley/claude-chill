use std::time::{Duration, Instant};

pub struct RedrawThrottler {
    min_interval: Duration,
    last_flush: Option<Instant>,
    pending_redraw: Option<Vec<u8>>,
}

impl RedrawThrottler {
    pub fn new(min_interval_ms: u64) -> Self {
        Self {
            min_interval: Duration::from_millis(min_interval_ms),
            last_flush: None,
            pending_redraw: None,
        }
    }

    pub fn submit(&mut self, data: Vec<u8>) {
        self.pending_redraw = Some(data);
    }

    pub fn should_flush(&self) -> bool {
        if self.pending_redraw.is_none() {
            return false;
        }

        match self.last_flush {
            None => true,
            Some(last) => last.elapsed() >= self.min_interval,
        }
    }

    pub fn take_pending(&mut self) -> Option<Vec<u8>> {
        if self.should_flush() {
            self.last_flush = Some(Instant::now());
            self.pending_redraw.take()
        } else {
            None
        }
    }

    pub fn time_until_next_flush(&self) -> Option<Duration> {
        self.pending_redraw.as_ref()?;

        match self.last_flush {
            None => Some(Duration::ZERO),
            Some(last) => {
                let elapsed = last.elapsed();
                if elapsed >= self.min_interval {
                    Some(Duration::ZERO)
                } else {
                    Some(self.min_interval - elapsed)
                }
            }
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending_redraw.is_some()
    }

    pub fn can_render(&self) -> bool {
        match self.last_flush {
            None => true,
            Some(last) => last.elapsed() >= self.min_interval,
        }
    }

    pub fn mark_rendered(&mut self) {
        self.last_flush = Some(Instant::now());
    }

    pub fn time_until_can_render(&self) -> Option<Duration> {
        match self.last_flush {
            None => Some(Duration::ZERO),
            Some(last) => {
                let elapsed = last.elapsed();
                if elapsed >= self.min_interval {
                    Some(Duration::ZERO)
                } else {
                    Some(self.min_interval - elapsed)
                }
            }
        }
    }
}
