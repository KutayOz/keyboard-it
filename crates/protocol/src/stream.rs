//! Stream negotiation and pacing shared by both native applications.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamSettings {
    pub fps: u32,
    /// Zero selects the source dimensions (subject to encoder limits).
    pub max_width: u32,
    pub max_height: u32,
    /// Zero selects a bitrate from the actual dimensions and requested FPS.
    pub bitrate_kbps: u32,
}
impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            fps: 60,
            max_width: 0,
            max_height: 0,
            bitrate_kbps: 0,
        }
    }
}
impl StreamSettings {
    pub const PROFILES: [(&'static str, Self); 4] = [
        (
            "Doğal çözünürlük · 60 FPS",
            Self {
                fps: 60,
                max_width: 0,
                max_height: 0,
                bitrate_kbps: 0,
            },
        ),
        (
            "Doğal çözünürlük · 120 FPS",
            Self {
                fps: 120,
                max_width: 0,
                max_height: 0,
                bitrate_kbps: 0,
            },
        ),
        (
            "1080p60 · 20 Mbps",
            Self {
                fps: 60,
                max_width: 1920,
                max_height: 1080,
                bitrate_kbps: 20_000,
            },
        ),
        (
            "1440p60 · 40 Mbps",
            Self {
                fps: 60,
                max_width: 2560,
                max_height: 1440,
                bitrate_kbps: 40_000,
            },
        ),
    ];
    pub fn valid(self) -> bool {
        (15..=120).contains(&self.fps)
            && ((self.max_width == 0 && self.max_height == 0)
                || ((2..=4096).contains(&self.max_width) && (2..=4096).contains(&self.max_height)))
            && (self.bitrate_kbps == 0 || (2_000..=150_000).contains(&self.bitrate_kbps))
    }
    pub fn normalized(self) -> Self {
        if self.valid() {
            self
        } else {
            Self::default()
        }
    }
    pub fn dimensions(self, w: u32, h: u32) -> (u32, u32) {
        let s = self.normalized();
        let limit_w = if s.max_width == 0 { 4096 } else { s.max_width };
        let limit_h = if s.max_height == 0 {
            4096
        } else {
            s.max_height
        };
        let scale = (limit_w as f64 / w.max(1) as f64)
            .min(limit_h as f64 / h.max(1) as f64)
            .min(1.0);
        (
            ((w as f64 * scale) as u32 & !1).max(2),
            ((h as f64 * scale) as u32 & !1).max(2),
        )
    }
    pub fn bitrate(self, w: u32, h: u32) -> u32 {
        let s = self.normalized();
        if s.bitrate_kbps != 0 {
            return s.bitrate_kbps;
        }
        // Roughly 0.24 bits/pixel/frame. Raise quality together with frame rate.
        (w as u64 * h as u64 * s.fps as u64 * 24 / 100_000).clamp(12_000, 120_000) as u32
    }
    pub fn interval(self) -> Duration {
        Duration::from_secs_f64(1.0 / self.normalized().fps as f64)
    }
}

/// Never catch up by producing a burst of obsolete frames. The capture loop
/// measures its own work; after overload it automatically returns to target FPS.
pub struct FramePacer {
    interval: Duration,
    next: Instant,
}
impl FramePacer {
    pub fn new(settings: StreamSettings, now: Instant) -> Self {
        Self {
            interval: settings.interval(),
            next: now,
        }
    }
    pub fn finish(&mut self, now: Instant) -> Duration {
        self.next += self.interval;
        if self.next <= now {
            self.next = now;
            Duration::ZERO
        } else {
            self.next - now
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_preserve_aspect_and_do_not_upscale() {
        let s = StreamSettings::PROFILES[2].1;
        assert_eq!(s.dimensions(3840, 2160), (1920, 1080));
        assert_eq!(s.dimensions(1080, 1920), (606, 1080));
        assert_eq!(s.dimensions(1280, 720), (1280, 720));
        assert_eq!(
            StreamSettings::default().dimensions(7680, 4320),
            (4096, 2304)
        );
        assert!(!StreamSettings { fps: 0, ..s }.valid());
        assert!(!StreamSettings { max_height: 0, ..s }.valid());
        assert!(!StreamSettings {
            bitrate_kbps: u32::MAX,
            ..s
        }
        .valid());
        assert_eq!(s.bitrate(1920, 1080), 20_000);
        assert!(
            StreamSettings {
                fps: 120,
                ..Default::default()
            }
            .bitrate(1920, 1080)
                > StreamSettings::default().bitrate(1920, 1080)
        );
    }
    #[test]
    fn overload_does_not_permanently_lower_fps_or_create_catchup_bursts() {
        let t = Instant::now();
        let mut p = FramePacer::new(StreamSettings::default(), t);
        assert!(p.finish(t + Duration::from_millis(5)) > Duration::from_millis(11));
        assert_eq!(p.finish(t + Duration::from_millis(100)), Duration::ZERO);
        let sleep = p.finish(t + Duration::from_millis(105));
        assert!(sleep > Duration::from_millis(11) && sleep < Duration::from_millis(12));
    }
}
