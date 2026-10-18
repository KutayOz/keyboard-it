//! Stoppable desktop listener; unauthenticated sockets never replace live sessions.
use protocol::{
    config::Config,
    desktop::{self, Channel, Message},
};
use std::{
    io,
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod encode;
#[cfg(windows)]
mod gpu;

pub struct Handle {
    pub port: u16,
    stop: Arc<AtomicBool>,
    live: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(s) = self.live.lock().unwrap().take() {
            desktop::shutdown(&s);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
pub fn start(
    cfg: &Config,
    on_conn: Arc<dyn Fn(crate::serve::ConnStatus) + Send + Sync>,
) -> io::Result<Handle> {
    let listener = TcpListener::bind(("0.0.0.0", cfg.desktop_port))
        .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let live = Arc::new(Mutex::new(None::<TcpStream>));
    let s = stop.clone();
    let l = live.clone();
    let cfg = cfg.clone();
    // Bound concurrent unauthenticated handshakes, each with a five-second timeout.
    let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let t = thread::spawn(move || {
        while !s.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((socket, _)) => {
                    if pending.fetch_add(1, Ordering::SeqCst) >= 4 {
                        pending.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                    let cfg = cfg.clone();
                    let s = s.clone();
                    let l = l.clone();
                    let pending = pending.clone();
                    let on_conn = on_conn.clone();
                    thread::spawn(move || {
                        let authenticated = protocol::secure::psk_from_config_or_env(&cfg)
                            .and_then(|key| Channel::connect(socket, &key, &cfg.device_id, false));
                        pending.fetch_sub(1, Ordering::SeqCst);
                        let (channel, _) = match authenticated {
                            Ok(v) => v,
                            Err(e) => {
                                crate::diag::plog!("desktop authentication failed: {e}");
                                return;
                            }
                        };
                        // The slot stays locked until ownership has transferred, preventing
                        // the old cleanup from touching keys belonging to this connection.
                        let generation = {
                            let mut slot = l.lock().unwrap();
                            if s.load(Ordering::SeqCst) {
                                return;
                            }
                            if let Some(old) = slot.take() {
                                desktop::shutdown(&old);
                            }
                            let Ok(clone) = channel.shutdown.try_clone() else {
                                return;
                            };
                            *slot = Some(clone);
                            crate::inject::acquire()
                        };
                        on_conn(crate::serve::ConnStatus::Connected);
                        let socket = channel.shutdown.try_clone().ok();
                        if let Err(e) = run(channel, &s, generation) {
                            crate::diag::plog!("desktop session ended: {e}");
                        }
                        if crate::inject::owned(generation) {
                            on_conn(crate::serve::ConnStatus::Disconnected);
                        }
                        crate::inject::retire(generation);
                        if let Some(socket) = socket {
                            desktop::shutdown(&socket);
                        }
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50))
                }
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        }
    });
    Ok(Handle {
        port,
        stop,
        live,
        thread: Some(t),
    })
}

#[cfg(windows)]
fn run(channel: Channel, stop: &AtomicBool, initial_owner: u64) -> io::Result<()> {
    use desktop::{Assembly, ClipboardSync, MAX_CLIPBOARD};
    use std::{sync::mpsc, time::Instant};
    struct Activity {
        visible: bool,
        clipboard: bool,
        epoch: u64,
        owner: u64,
        display: Option<desktop::Display>,
        ready: bool,
        settings: protocol::stream::StreamSettings,
        pong: Option<u64>,
    }
    let activity = Arc::new(Mutex::new(Activity {
        visible: false,
        clipboard: false,
        epoch: 0,
        owner: initial_owner,
        display: None,
        ready: false,
        settings: Default::default(),
        pong: None,
    }));
    let alive = Arc::new(AtomicBool::new(true));
    let (tx, rx) = mpsc::sync_channel(32);
    let Channel {
        mut reader,
        mut writer,
        shutdown,
    } = channel;
    let a = activity.clone();
    let running = alive.clone();
    let reader_thread = thread::spawn(move || {
        while running.load(Ordering::SeqCst) {
            let Ok(msg) = reader.recv() else {
                break;
            };
            let mut a = a.lock().unwrap();
            if !crate::inject::owned(a.owner) {
                break;
            }
            match msg {
                Message::Heartbeat => {}
                Message::Ping(id) => a.pong = Some(id),
                Message::Configure(settings) => {
                    if a.settings != settings {
                        a.settings = settings;
                        a.epoch += 1;
                        a.ready = false;
                        crate::inject::release(a.owner);
                    }
                }
                Message::Active { visible, clipboard } => {
                    if visible != a.visible || clipboard != a.clipboard {
                        a.epoch += 1;
                    }
                    if !visible {
                        crate::inject::release(a.owner);
                        a.ready = false;
                    }
                    a.visible = visible;
                    a.clipboard = clipboard;
                }
                Message::ReleaseAll => crate::inject::release(a.owner),
                Message::Input(ev) if a.visible && a.ready => {
                    if !crate::inject::event(a.owner, ev) {
                        break;
                    }
                }
                Message::Absolute { x, y } if a.visible && a.ready => {
                    if let Some(d) = &a.display {
                        crate::inject::absolute(a.owner, x, y, d);
                    }
                }
                msg @ Message::Clipboard { .. } if a.visible && a.ready && a.clipboard => {
                    if tx.try_send((a.epoch, msg)).is_err() {
                        break;
                    }
                }
                Message::Input(_) | Message::Absolute { .. } | Message::Clipboard { .. } => {}
                _ => break,
            }
        }
        let a = a.lock().unwrap();
        crate::inject::release(a.owner);
        running.store(false, Ordering::SeqCst);
    });
    let result = (|| -> io::Result<()> {
        let _runtime = encode::Encoder::initialize().map_err(io::Error::other)?;
        let clipboard = clipboard::Clipboard::new().ok();
        let mut sync = ClipboardSync::default();
        let mut assembly = Assembly::default();
        let mut epoch = u64::MAX;
        let mut clipboard_id = 0;
        let mut frame_id = 0;
        let mut pipeline = None;
        let mut allow_hardware = true;
        let mut allow_gpu = true;
        let mut last_output = Instant::now();
        let mut heartbeat = Instant::now() - Duration::from_secs(2);
        let mut retry = Instant::now();
        let mut pacer = protocol::stream::FramePacer::new(Default::default(), Instant::now());
        let mut report = Instant::now();
        let mut stats = desktop::StreamStats::default();
        let mut stat_frames = 0u64;
        let mut stat_bytes = 0u64;
        let mut stat_attempts = 0u64;
        let mut capture_us = 0u64;
        let mut encode_us = 0u64;
        let mut send_us = 0u64;
        while alive.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
            if let Some(c) = &clipboard {
                c.pump();
            }
            if heartbeat.elapsed() >= Duration::from_secs(1) {
                writer.send(&Message::Heartbeat)?;
                heartbeat = Instant::now();
            }
            let (visible, share, now_epoch, settings, pong) = {
                let mut a = activity.lock().unwrap();
                (a.visible, a.clipboard, a.epoch, a.settings, a.pong.take())
            };
            if let Some(id) = pong {
                writer.send(&Message::Pong(id))?;
            }
            if epoch != now_epoch {
                epoch = now_epoch;
                assembly.clear();
                sync.reset(clipboard.as_ref().and_then(|c| c.read()));
                while rx.try_recv().is_ok() {}
                if visible {
                    pipeline = None;
                    retry = Instant::now();
                }
            }
            if !visible {
                pipeline = None;
                thread::sleep(Duration::from_millis(25));
                continue;
            }
            if pipeline.is_none() && Instant::now() >= retry {
                match capture::Capture::new().and_then(|cap| {
                    encode::Encoder::new(
                        cap.display.width,
                        cap.display.height,
                        settings,
                        if allow_gpu { Some(&cap.device) } else { None },
                        allow_hardware,
                    )
                    .map(|enc| (cap, enc))
                }) {
                    Ok((cap, enc)) => {
                        let mut d = cap.display.clone();
                        d.width = enc.width;
                        d.height = enc.height;
                        writer.send(&Message::Display(d.clone()))?;
                        writer.send(&Message::Status(format!(
                            "{} × {} · {} FPS · {} Mbps · H.264 · {}",
                            d.width,
                            d.height,
                            enc.fps,
                            enc.bitrate_kbps / 1000,
                            if enc.uses_gpu_surfaces() {
                                "GPU yüzeyleri"
                            } else if enc.hardware {
                                "GPU kodlayıcı / CPU dönüşümü"
                            } else {
                                "CPU"
                            }
                        )))?;
                        activity.lock().unwrap().display = Some(d);
                        enc.force_keyframe();
                        let effective = protocol::stream::StreamSettings {
                            fps: enc.fps,
                            ..settings
                        };
                        pipeline = Some((cap, enc));
                        last_output = Instant::now();
                        pacer = protocol::stream::FramePacer::new(effective, Instant::now());
                        report = Instant::now();
                        stats = Default::default();
                        stat_frames = 0;
                        stat_bytes = 0;
                        stat_attempts = 0;
                        capture_us = 0;
                        encode_us = 0;
                        send_us = 0;
                    }
                    Err(e) => {
                        writer.send(&Message::Status(format!("Windows ekranı kullanılamıyor. Kilidi veya ekran bağlantısını kontrol edin. ({})",e.code())))?;
                        retry = Instant::now() + Duration::from_secs(1);
                    }
                }
            }
            if let Some((cap, enc)) = &mut pipeline {
                let before_capture = Instant::now();
                let mut capture_failed = false;
                let encoded = if enc.uses_gpu_surfaces() {
                    match cap.gpu_frame() {
                        Ok(frame) => {
                            capture_us += before_capture.elapsed().as_micros() as u64;
                            let before_encode = Instant::now();
                            let result = match frame {
                                Some(frame) => enc.encode_gpu(&frame),
                                None => Ok(vec![]),
                            };
                            encode_us += before_encode.elapsed().as_micros() as u64;
                            result
                        }
                        Err(e) => {
                            capture_failed = true;
                            Err(e)
                        }
                    }
                } else {
                    match cap.frame() {
                        Ok(frame) => {
                            capture_us += before_capture.elapsed().as_micros() as u64;
                            let before_encode = Instant::now();
                            let result = match frame {
                                Some(p) => enc.encode(
                                    &p,
                                    cap.display.source_width,
                                    cap.display.source_height,
                                ),
                                None => Ok(vec![]),
                            };
                            encode_us += before_encode.elapsed().as_micros() as u64;
                            result
                        }
                        Err(e) => {
                            capture_failed = true;
                            Err(e)
                        }
                    }
                };
                stat_attempts += 1;
                match encoded {
                    Ok(frames) if !frames.is_empty() => {
                        last_output = Instant::now();
                        let before_send = Instant::now();
                        for data in frames {
                            stat_frames += 1;
                            stat_bytes += data.len() as u64;
                            frame_id += 1;
                            for msg in desktop::video_chunks(
                                frame_id,
                                desktop::h264_keyframe(&data),
                                &data,
                            )? {
                                writer.send(&msg)?;
                            }
                        }
                        send_us += before_send.elapsed().as_micros() as u64;
                        let mut a = activity.lock().unwrap();
                        if a.epoch == epoch && a.visible {
                            a.ready = true;
                        }
                    }
                    Ok(_) if last_output.elapsed() > Duration::from_secs(2) => {
                        if !capture_failed {
                            if enc.uses_gpu_surfaces() {
                                allow_gpu = false;
                            } else if enc.hardware {
                                allow_hardware = false;
                            }
                        }
                        let mut a = activity.lock().unwrap();
                        a.ready = false;
                        crate::inject::release(a.owner);
                        drop(a);
                        pipeline = None;
                        retry = Instant::now() + Duration::from_secs(1);
                        writer.send(&Message::Status(
                            "Windows ekranı durakladı — kodlayıcı yeniden başlatılıyor…".into(),
                        ))?;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        crate::diag::plog!("desktop pipeline error: {e}");
                        if !capture_failed {
                            if enc.uses_gpu_surfaces() {
                                allow_gpu = false;
                            } else if enc.hardware {
                                allow_hardware = false;
                            }
                        }
                        let mut a = activity.lock().unwrap();
                        a.ready = false;
                        crate::inject::release(a.owner);
                        drop(a);
                        pipeline = None;
                        retry = Instant::now() + Duration::from_secs(1);
                        writer.send(&Message::Status(
                            "Windows ekranı durakladı — yeniden bağlanılıyor…".into(),
                        ))?;
                    }
                }
            }
            if share {
                while let Ok((
                    msg_epoch,
                    Message::Clipboard {
                        id,
                        total,
                        offset,
                        data,
                    },
                )) = rx.try_recv()
                {
                    if msg_epoch != epoch {
                        continue;
                    }
                    if let Some(bytes) = assembly.push(id, total, offset, &data, MAX_CLIPBOARD)? {
                        let text = String::from_utf8(bytes).map_err(io::Error::other)?;
                        if let Some(c) = &clipboard {
                            if sync.remote(id, &text) && !c.write(&text) {
                                sync.reset(c.read());
                            }
                        }
                    }
                }
                if let Some(text) = sync.local_change(clipboard.as_ref().and_then(|c| c.read())) {
                    clipboard_id += 1;
                    for msg in desktop::clipboard_chunks(clipboard_id, &text)? {
                        writer.send(&msg)?;
                    }
                }
            }
            if report.elapsed() >= Duration::from_secs(1) && pipeline.is_some() {
                let seconds = report.elapsed().as_secs_f64();
                stats.fps_milli = (stat_frames as f64 * 1000.0 / seconds) as u32;
                stats.bitrate_kbps = (stat_bytes as f64 * 8.0 / seconds / 1000.0) as u32;
                stats.capture_us = (capture_us / stat_attempts.max(1)).min(u32::MAX as u64) as u32;
                stats.encode_us = (encode_us / stat_attempts.max(1)).min(u32::MAX as u64) as u32;
                stats.send_us = (send_us / stat_frames.max(1)).min(u32::MAX as u64) as u32;
                stats.skipped = pipeline
                    .as_mut()
                    .map(|(_, e)| std::mem::take(&mut e.skipped))
                    .unwrap_or(0);
                writer.send(&Message::Stats(stats))?;
                report = Instant::now();
                stat_frames = 0;
                stat_bytes = 0;
                stat_attempts = 0;
                capture_us = 0;
                encode_us = 0;
                send_us = 0;
            }
            if pipeline.is_some() {
                thread::sleep(pacer.finish(Instant::now()));
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    })();
    alive.store(false, Ordering::SeqCst);
    desktop::shutdown(&shutdown);
    let _ = reader_thread.join();
    let a = activity.lock().unwrap();
    crate::inject::release(a.owner);
    result
}
#[cfg(not(windows))]
fn run(mut channel: Channel, stop: &AtomicBool, _owner: u64) -> io::Result<()> {
    channel.writer.send(&Message::Status(
        "Screen capture requires Windows; input-only mode is available.".into(),
    ))?;
    while !stop.load(Ordering::SeqCst) {
        channel.reader.recv()?;
        channel.writer.send(&Message::Heartbeat)?;
    }
    Ok(())
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    #[test]
    fn authenticated_reconnect_survives_bad_key_and_port_collision_then_stops() {
        let _guard = crate::inject::TEST_CONTROLLER.lock().unwrap();
        let busy = TcpListener::bind("0.0.0.0:0").unwrap();
        let cfg = Config {
            device_id: "test-pc".into(),
            shared_secret: "session-test".into(),
            desktop_port: busy.local_addr().unwrap().port(),
            ..Config::default()
        };
        let handle = start(&cfg, Arc::new(|_| {})).unwrap();
        assert_ne!(handle.port, cfg.desktop_port);
        let addr = ("127.0.0.1", handle.port);
        let psk = protocol::secure::psk_from_secret(&cfg.shared_secret);
        let (mut first, id) =
            Channel::connect(TcpStream::connect(addr).unwrap(), &psk, "test-mac", true).unwrap();
        assert_eq!(id, cfg.device_id);
        assert!(matches!(first.reader.recv().unwrap(), Message::Status(_)));
        let idle = TcpStream::connect(addr).unwrap();
        assert!(Channel::connect(
            TcpStream::connect(addr).unwrap(),
            &[0; 32],
            "wrong-key",
            true
        )
        .is_err());
        first.writer.send(&Message::Heartbeat).unwrap();
        assert_eq!(first.reader.recv().unwrap(), Message::Heartbeat);
        let (mut second, _) =
            Channel::connect(TcpStream::connect(addr).unwrap(), &psk, "test-mac", true).unwrap();
        assert!(matches!(second.reader.recv().unwrap(), Message::Status(_)));
        assert!(first.reader.recv().is_err());
        second.writer.send(&Message::Heartbeat).unwrap();
        assert_eq!(second.reader.recv().unwrap(), Message::Heartbeat);
        drop(idle);
        drop(handle);
        assert!(second.reader.recv().is_err());
    }
}
