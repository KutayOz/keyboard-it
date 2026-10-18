//! Bounded, local performance samples. No screen, clipboard, keys or pairing
//! material is recorded. CPU stage timings and RTT are not input latency.
use protocol::desktop::StreamStats;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub struct Metrics {
    started: Instant,
    window: Instant,
    received: u64,
    presented: u64,
    replaced: u64,
    decode_us: u64,
    queue_us: u64,
    host: StreamStats,
    host_at: Option<Instant>,
    pending_ping: Option<(u64, Instant)>,
    rtt_us: Option<u64>,
    rows: VecDeque<String>,
    pub summary: String,
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            window: Instant::now(),
            received: 0,
            presented: 0,
            replaced: 0,
            decode_us: 0,
            queue_us: 0,
            host: Default::default(),
            host_at: None,
            pending_ping: None,
            rtt_us: None,
            rows: VecDeque::new(),
            summary: String::new(),
        }
    }
}
impl Metrics {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn host(&mut self, stats: StreamStats) {
        self.host = stats;
        self.host_at = Some(Instant::now());
    }
    pub fn ping(&mut self, id: u64) {
        self.pending_ping = Some((id, Instant::now()));
    }
    pub fn pong(&mut self, id: u64) {
        if let Some((pending, time)) = self.pending_ping {
            if pending == id {
                self.rtt_us = Some(time.elapsed().as_micros() as u64);
                self.pending_ping = None;
            }
        }
    }
    pub fn decoded(&mut self, us: u64, replaced: bool) {
        self.received += 1;
        self.decode_us += us;
        self.replaced += replaced as u64;
    }
    pub fn presented(&mut self, queue_us: u64) {
        self.presented += 1;
        self.queue_us += queue_us;
    }
    pub fn tick(&mut self, width: u32, height: u32, requested_fps: u32, active: bool) {
        if self.window.elapsed() < Duration::from_secs(1) {
            return;
        }
        let seconds = self.window.elapsed().as_secs_f64();
        if active {
            let host = if self
                .host_at
                .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
            {
                self.host
            } else {
                Default::default()
            };
            let received = self.received as f64 / seconds;
            let presented = self.presented as f64 / seconds;
            let decode = self.decode_us as f64 / self.received.max(1) as f64 / 1000.;
            let queue = self.queue_us as f64 / self.presented.max(1) as f64 / 1000.;
            let rtt = self
                .rtt_us
                .map(|us| format!("{:.2}", us as f64 / 1000.))
                .unwrap_or_default();
            self.summary = format!("{}×{} · hedef {} / alınan {:.1} / sunulan {:.1} FPS · {:.1} Mbps · çözme {:.1} ms · RTT {} ms",
                width, height, requested_fps, received, presented, host.bitrate_kbps as f64/1000., decode,
                if rtt.is_empty() { "—" } else { &rtt });
            self.rows.push_back(format!("{:.3},{width},{height},{requested_fps},{:.2},{received:.2},{presented:.2},{},{:.3},{:.3},{:.3},{decode:.3},{queue:.3},{},{},{}\n",
                self.started.elapsed().as_secs_f64(), host.fps_milli as f64/1000., host.bitrate_kbps,
                host.capture_us as f64/1000., host.encode_us as f64/1000., host.send_us as f64/1000., rtt, host.skipped, self.replaced));
            if self.rows.len() > 300 {
                self.rows.pop_front();
            }
        }
        self.window = Instant::now();
        self.received = 0;
        self.presented = 0;
        self.replaced = 0;
        self.decode_us = 0;
        self.queue_us = 0;
    }
    pub fn csv(&self) -> String {
        let mut output = String::from("elapsed_s,width,height,requested_fps,host_fps,decoded_fps,submitted_fps,video_kbps,host_capture_cpu_ms,host_encode_cpu_ms,host_send_cpu_ms,decode_ms,after_decode_queue_ms,rtt_ms,host_skipped_captures,client_replaced_frames\n");
        for row in &self.rows {
            output.push_str(row);
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reporting_is_bounded_and_reconnect_resets_stale_measurements() {
        let mut m = Metrics::default();
        for _ in 0..305 {
            m.window = Instant::now() - Duration::from_secs(1);
            m.host(StreamStats {
                fps_milli: 60000,
                bitrate_kbps: 20000,
                ..Default::default()
            });
            m.decoded(2000, false);
            m.presented(1000);
            m.tick(1920, 1080, 60, true);
        }
        assert_eq!(m.csv().lines().count(), 301);
        assert!(m.csv().contains(",60.00,"));
        assert!(!m.csv().contains("NaN"));
        m.reset();
        assert_eq!(m.csv().lines().count(), 1);
        assert!(m.summary.is_empty());
    }
}
