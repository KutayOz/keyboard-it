use controller_client::{Event, PairEvent, Pairing, Session, Target};
use protocol::{
    capabilities::Capabilities,
    desktop::{self, Channel, Display, Message},
    devices::Platform,
    pairing, secure, InputEvent,
};
use std::{
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};
fn target(listener: &TcpListener, id: &str, secret: &str) -> Target {
    Target {
        name: "Fixture".into(),
        host: "127.0.0.1".into(),
        port: 5599,
        desktop_port: listener.local_addr().unwrap().port(),
        device_id: id.into(),
        secret: secret.into(),
    }
}
#[test]
fn verified_pairing_emits_complete_fingerprint_and_pins_the_host_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let secret = secure::generate_key();
    let expected = secret.clone();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        pairing::pair_responder_authorized_with_identity(
            &mut s,
            5599,
            "Fixture",
            "host-id",
            |req| {
                assert_eq!(req.device_id, "android-id");
                assert_eq!(req.code.len(), 64);
                Ok(Some(secret))
            },
        )
        .unwrap();
    });
    let pair = Pairing::start("127.0.0.1", port, 5601, "Android", "android-id").unwrap();
    match pair.poll(Duration::from_millis(500)).unwrap() {
        PairEvent::Fingerprint(code) => assert_eq!(code.split_whitespace().count(), 8),
        _ => panic!("fingerprint must precede credentials"),
    }
    match pair.poll(Duration::from_millis(500)).unwrap() {
        PairEvent::Paired(t) => {
            assert_eq!(t.secret, expected);
            assert_eq!(t.device_id, "host-id");
        }
        _ => panic!("expected verified target"),
    }
    server.join().unwrap();
}
#[test]
fn screenless_mac_negotiates_input_and_unicode_without_claiming_screen_or_clipboard() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let secret = secure::generate_key();
    let t = target(&listener, "mac-id", &secret);
    let (tx, rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let mut caps = Capabilities::default();
        caps.platform = Platform::Macos;
        caps.receive_input = true;
        caps.text_input = true;
        let (mut c, _, _) =
            Channel::accept(socket, &[secure::psk_from_secret(&secret)], "mac-id", &caps).unwrap();
        assert!(!c.negotiated.screen);
        assert!(c.negotiated.input && c.negotiated.text_input);
        assert!(!c.negotiated.clipboard);
        assert_eq!(
            c.reader.recv().unwrap(),
            Message::Active {
                visible: true,
                clipboard: false
            }
        );
        loop {
            match c.reader.recv() {
                Ok(Message::Heartbeat) => {}
                Ok(Message::Input(e)) => {
                    tx.send(e).unwrap();
                }
                Ok(Message::TextInput(text)) => {
                    assert_eq!(text, "İstanbul 👋");
                    tx.send(InputEvent::Scroll { dx: 0, dy: 7 }).unwrap();
                }
                Ok(Message::ReleaseAll) => break,
                Err(_) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
    });
    let session = Session::connect(&t, "android-id").unwrap();
    assert!(matches!(
        session.poll(Duration::from_millis(100)),
        Some(Event::Capabilities {
            screen: false,
            input: true,
            text: true
        })
    ));
    assert!(session.absolute(1, 1).is_err());
    session
        .input(InputEvent::MouseMove { dx: 4, dy: -2 })
        .unwrap();
    session.text("İstanbul 👋").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        InputEvent::MouseMove { dx: 4, dy: -2 }
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        InputEvent::Scroll { dx: 0, dy: 7 }
    );
    session.release().unwrap();
    server.join().unwrap();
    session.close();
}
#[test]
fn windows_video_is_reassembled_in_order_and_oversized_display_is_rejected() {
    for oversized in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let secret = secure::generate_key();
        let t = target(&listener, "pc-id", &secret);
        let server = thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let (mut c, _, _) = Channel::accept(
                socket,
                &[secure::psk_from_secret(&secret)],
                "pc-id",
                &Capabilities::host(Platform::Windows),
            )
            .unwrap();
            assert!(matches!(c.reader.recv().unwrap(), Message::Configure(_)));
            assert!(matches!(
                c.reader.recv().unwrap(),
                Message::Active {
                    visible: true,
                    clipboard: false
                }
            ));
            c.writer
                .send(&Message::Display(Display {
                    width: if oversized { 4096 } else { 320 },
                    height: 180,
                    left: 0,
                    top: 0,
                    source_width: 320,
                    source_height: 180,
                }))
                .unwrap();
            if !oversized {
                let bytes = include_bytes!("../../mac-sender/tests/fixtures/desktop.h264");
                for packet in desktop::video_chunks(1, true, bytes).unwrap() {
                    c.writer.send(&packet).unwrap();
                }
            }
            while let Ok(Message::Heartbeat) = c.reader.recv() {}
        });
        let session = Session::connect(&t, "android-id").unwrap();
        assert!(matches!(
            session.poll(Duration::from_millis(500)),
            Some(Event::Capabilities { screen: true, .. })
        ));
        if oversized {
            assert!(matches!(
                session.poll(Duration::from_millis(500)),
                Some(Event::Disconnected(_))
            ));
        } else {
            assert!(matches!(
                session.poll(Duration::from_millis(500)),
                Some(Event::Display {
                    width: 320,
                    height: 180
                })
            ));
            match session.poll(Duration::from_millis(500)) {
                Some(Event::Video { key, bytes }) => {
                    assert!(key);
                    assert_eq!(
                        bytes,
                        include_bytes!("../../mac-sender/tests/fixtures/desktop.h264")
                    );
                }
                other => panic!("{other:?}"),
            }
        }
        session.close();
        server.join().unwrap();
    }
}
#[test]
fn a_correct_key_with_a_different_authenticated_device_id_never_receives_activity_or_input() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let secret = secure::generate_key();
    let t = target(&listener, "pinned-id", &secret);
    let server = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let (mut c, _, _) = Channel::accept(
            socket,
            &[secure::psk_from_secret(&secret)],
            "different-id",
            &Capabilities::host(Platform::Windows),
        )
        .unwrap();
        assert!(c.reader.recv().is_err());
    });
    assert!(Session::connect(&t, "android-id").is_err());
    server.join().unwrap();
}
#[test]
fn cancelled_pairing_cannot_return_a_saved_credential() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (server_tx, server_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let _ =
            pairing::pair_responder_authorized_with_identity(&mut s, 5599, "PC", "host-id", |_| {
                server_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(None)
            });
    });
    let pair = Pairing::start("127.0.0.1", port, 5601, "Android", "id").unwrap();
    assert!(matches!(
        pair.poll(Duration::from_millis(500)),
        Some(PairEvent::Fingerprint(_))
    ));
    pair.cancel();
    let _ = server_tx.send(());
    server.join().unwrap();
    assert!(!matches!(
        pair.poll(Duration::from_millis(100)),
        Some(PairEvent::Paired(_))
    ));
}
#[test]
fn unicode_record_is_bounded_and_invalid_utf8_or_nul_is_rejected() {
    let text = Message::TextInput("Türkçe 👋".into());
    assert_eq!(Message::decode(&text.encode().unwrap()).unwrap(), text);
    for invalid in [vec![15], vec![15, 0], vec![15, 0xff], vec![15; 4098]] {
        assert!(Message::decode(&invalid).is_err());
    }
    assert!(Message::TextInput("x".repeat(4097)).encode().is_err());
}
