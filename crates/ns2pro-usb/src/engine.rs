//! Event-driven URB scheduler; caller supplies monotonic microseconds.
use crate::{
    InputSample, Ns2ProDevice, Output,
    usbip::{Urb, reply_into},
};
use std::collections::VecDeque;
/// Output of one engine step: write `replies`, forward `outputs`.
#[derive(Debug, Default)]
pub struct Effects {
    /// Encoded USB/IP replies, ready to write.
    pub replies: Vec<u8>,
    /// Host requests for the physical controller.
    pub outputs: Vec<Output>,
    /// Interrupt-IN completions since the last [`Self::take_completion_stats`].
    pub input_completions: u64,
    /// How many of those re-encoded the current input without a new sample.
    pub keepalives: u64,
    /// Sequence number of the sample in the newest completion appended to `replies`.
    pub completed_sample_seq: Option<u64>,
}
impl Effects {
    /// Returns and resets `(sample seq, completions, keepalives)` once a
    /// completion carrying a sample has been appended.
    pub fn take_completion_stats(&mut self) -> Option<(u64, u64, u64)> {
        let completions = std::mem::take(&mut self.input_completions);
        let keepalives = std::mem::take(&mut self.keepalives);
        self.completed_sample_seq
            .take()
            .map(|seq| (seq, completions, keepalives))
    }
}
/// Schedules USB/IP URBs for one virtual controller. Times are monotonic
/// microseconds supplied by the caller.
#[derive(Debug)]
pub struct UrbEngine {
    /// The emulated device.
    pub device: Ns2ProDevice,
    sample: InputSample,
    dirty: bool,
    pending: VecDeque<Urb>,
    last_report: u64,
}
impl Default for UrbEngine {
    fn default() -> Self {
        Self {
            device: Ns2ProDevice::default(),
            sample: InputSample::default(),
            dirty: true,
            pending: VecDeque::with_capacity(256),
            last_report: 0,
        }
    }
}
impl UrbEngine {
    /// Handles a submitted URB (or an unlink) received at `now`.
    pub fn submit(&mut self, u: Urb, data: &[u8], now: u64, e: &mut Effects) {
        if u.command == 2 {
            self.unlink(u, e);
            return;
        }
        let mut status = 0;
        let mut payload = Vec::new();
        let mut output = None;
        match (u.direction, u.ep) {
            (_, 0) => match self.device.control(u.setup, data, &self.sample) {
                Ok((p, o)) => {
                    payload = p;
                    output = o;
                }
                Err(s) => status = s,
            },
            (1, 1) => {
                if self.pending.len() >= 256 {
                    reply_into(&mut e.replies, u, -12, &[], 0);
                    return;
                }
                self.pending.push_back(u);
                self.drain(now, false, e);
                return;
            }
            (1, 2) => {
                if let Some(p) = self.device.read_bulk_in(u.length) {
                    payload = p;
                } else {
                    if self.pending.len() >= 256 {
                        reply_into(&mut e.replies, u, -12, &[], 0);
                        return;
                    }
                    self.pending.push_back(u);
                    return;
                }
            }
            (0, 1) => output = self.device.interrupt_out(data),
            (0, 2) => output = self.device.bulk_out(data),
            _ => status = -32,
        }
        payload.truncate(u.length);
        let actual = if status != 0 {
            0
        } else if u.direction == 0 {
            data.len()
        } else {
            payload.len()
        };
        reply_into(
            &mut e.replies,
            u,
            status,
            if u.direction == 1 { &payload } else { &[] },
            actual,
        );
        if let Some(o) = output {
            e.outputs.push(o);
        }
        self.drain(now, false, e);
    }
    /// Cancels the pending URB named by `u.unlink_seq`; the reply status is
    /// `-ECONNRESET` if it was pending and 0 otherwise.
    pub fn unlink(&mut self, u: Urb, e: &mut Effects) {
        let found = self.pending.iter().position(|p| p.seq == u.unlink_seq);
        if let Some(i) = found {
            self.pending.remove(i);
        }
        reply_into(
            &mut e.replies,
            u,
            if found.is_some() { -104 } else { 0 },
            &[],
            0,
        );
    }
    /// Records new input and completes waiting interrupt-IN URBs.
    pub fn on_sample(&mut self, sample: InputSample, now: u64, e: &mut Effects) {
        self.sample = sample;
        self.dirty = true;
        self.drain(now, false, e);
    }
    /// When a waiting interrupt-IN URB should repeat the current report (4 ms after
    /// the last one), or `None` if none is waiting.
    #[must_use]
    pub fn next_keepalive_at(&self) -> Option<u64> {
        self.pending
            .iter()
            .any(|u| u.ep == 1)
            .then_some(self.last_report.saturating_add(4000))
    }
    /// Completes interrupt-IN URBs whose keepalive deadline has passed.
    pub fn on_keepalive_due(&mut self, now: u64, e: &mut Effects) {
        self.drain(now, true, e);
    }
    fn drain(&mut self, now: u64, keepalive: bool, e: &mut Effects) {
        let mut i = 0;
        while let Some(&urb) = self.pending.get(i) {
            if urb.ep == 1
                && (self.dirty || (keepalive && now >= self.last_report.saturating_add(4000)))
            {
                let report = self.device.interrupt_in(&self.sample);
                if !self.dirty {
                    e.keepalives += 1;
                }
                self.dirty = false;
                self.last_report = now;
                e.input_completions += 1;
                e.completed_sample_seq = Some(self.sample.seq);
                self.pending.remove(i);
                let payload = report.get(..urb.length).unwrap_or(&report);
                reply_into(&mut e.replies, urb, 0, payload, payload.len());
            } else if urb.ep == 2 {
                if let Some(data) = self.device.read_bulk_in(urb.length) {
                    self.pending.remove(i);
                    reply_into(&mut e.replies, urb, 0, &data, data.len());
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
    }
}
