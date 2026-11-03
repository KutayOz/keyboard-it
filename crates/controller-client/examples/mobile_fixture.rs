//! Deliberately consenting, loopback-only fixture for isolated Android emulator
//! integration tests. It never loads native input adapters or real user configs.
use protocol::{
    access::AccessStore,
    capabilities::Capabilities,
    desktop::{self, Channel, Display, Message},
    devices::Platform,
    pairing,
};
use std::{net::TcpListener, thread, time::Duration};
fn host(pair_port: u16, desktop_port: u16, screen: bool) {
    let access = AccessStore::in_memory("");
    let pairing_access = access.clone();
    let id = if screen {
        "fixture-windows"
    } else {
        "fixture-mac"
    };
    let pair = TcpListener::bind(("127.0.0.1", pair_port)).unwrap();
    let desktop = TcpListener::bind(("127.0.0.1", desktop_port)).unwrap();
    thread::spawn(move || {
        for socket in pair.incoming() {
            let mut socket = socket.unwrap();
            let access = pairing_access.clone();
            thread::spawn(move || {
                let _ = pairing::pair_responder_authorized_with_identity(
                    &mut socket,
                    5599,
                    id,
                    id,
                    |req| access.grant(&req.peer_name, &req.device_id).map(Some),
                );
            });
        }
    });
    thread::spawn(move || {
        for socket in desktop.incoming() {
            let socket = socket.unwrap();
            let access = access.clone();
            thread::spawn(move || {
                let caps = if screen {
                    Capabilities::host(Platform::Windows)
                } else {
                    Capabilities {
                        platform: Platform::Macos,
                        receive_input: true,
                        text_input: true,
                        ..Default::default()
                    }
                };
                let keys = access.authentication_keys();
                let psks: Vec<_> = keys.iter().map(|k| k.key).collect();
                let Ok((mut c, peer, index)) = Channel::accept(socket, &psks, id, &caps) else {
                    return;
                };
                if keys[index].device_id != peer {
                    return;
                }
                let _lease = access.attach(&keys[index], &c.shutdown).unwrap();
                if screen {
                    assert!(matches!(c.reader.recv(), Ok(Message::Configure(_))));
                }
                if !matches!(c.reader.recv(), Ok(Message::Active { visible: true, .. })) {
                    return;
                }
                if screen {
                    let _ = c.writer.send(&Message::Display(Display {
                        width: 320,
                        height: 180,
                        left: 0,
                        top: 0,
                        source_width: 320,
                        source_height: 180,
                    }));
                    for p in desktop::video_chunks(
                        1,
                        true,
                        include_bytes!("../../mac-sender/tests/fixtures/desktop.h264"),
                    )
                    .unwrap()
                    {
                        if c.writer.send(&p).is_err() {
                            return;
                        }
                    }
                }
                while let Ok(msg) = c.reader.recv() {
                    match msg {
                        Message::Heartbeat => {
                            if c.writer.send(&Message::Heartbeat).is_err() {
                                break;
                            }
                        }
                        Message::TextInput(text) => {
                            if text != "Türkçe 👋" {
                                break;
                            }
                            let _ = c.writer.send(&Message::Status("fixture:unicode-ok".into()));
                        }
                        Message::Input(_) | Message::Absolute { .. } => {
                            let _ = c.writer.send(&Message::Status("fixture:input-ok".into()));
                        }
                        Message::ReleaseAll => {
                            let _ = c.writer.send(&Message::Status("fixture:release-ok".into()));
                        }
                        _ => break,
                    }
                }
            });
        }
    });
}
fn main() {
    host(57811, 57812, true);
    host(57821, 57822, false);
    println!("loopback mobile fixtures ready");
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}
