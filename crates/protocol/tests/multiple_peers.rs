//! Real encrypted sessions with simulated PCs; no OS capture/injection or LAN devices.
use protocol::{
    config::Config,
    desktop::{Channel, Message},
    devices::{PairedDevice, Platform},
    secure, InputEvent,
};
use std::{
    net::{TcpListener, TcpStream},
    thread,
};

fn simulated_pc(name: &'static str, connections: usize) -> (PairedDevice, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let secret = format!("private-key-for-{name}");
    let key = secure::psk_from_secret(&secret);
    let server = thread::spawn(move || {
        for _ in 0..connections {
            let (socket, _) = listener.accept().unwrap();
            let (mut channel, controller) = Channel::connect(socket, &key, name, false).unwrap();
            assert_eq!(controller, "controller");
            assert!(matches!(
                channel.reader.recv().unwrap(),
                Message::Input(InputEvent::MouseMove { dx: 7, dy: 9 })
            ));
            assert!(matches!(
                channel.reader.recv().unwrap(),
                Message::ReleaseAll
            ));
            channel.writer.send(&Message::Status(name.into())).unwrap();
            assert!(channel.reader.recv().is_err()); // client switch closes its old socket
        }
    });
    (
        PairedDevice {
            transport: Default::default(),
            capabilities: None,
            id: name.into(),
            name: name.into(),
            platform: Platform::Windows,
            permissions: Default::default(),
            peer_device_id: name.into(),
            shared_secret: secret,
            host: "127.0.0.1".into(),
            ip: String::new(),
            port,
            desktop_port: port,
        },
        server,
    )
}

#[test]
fn a_b_a_switching_uses_each_saved_pairing_key_and_authenticated_identity() {
    let (a, server_a) = simulated_pc("pc-a", 2);
    let (b, server_b) = simulated_pc("pc-b", 1);
    let mut cfg = Config::default();
    cfg.remember_device(a).unwrap();
    cfg.remember_device(b).unwrap();
    for selected in ["pc-a", "pc-b", "pc-a"] {
        cfg.activate_device(selected).unwrap();
        let socket = TcpStream::connect(format!("{}:{}", cfg.peer_host, cfg.desktop_port)).unwrap();
        let key = secure::psk_from_secret(&cfg.shared_secret);
        let (mut channel, identity) = Channel::connect(socket, &key, "controller", true).unwrap();
        assert_eq!(identity, cfg.peer_device_id);
        channel
            .writer
            .send(&Message::Input(InputEvent::MouseMove { dx: 7, dy: 9 }))
            .unwrap();
        channel.writer.send(&Message::ReleaseAll).unwrap();
        assert!(
            matches!(channel.reader.recv().unwrap(), Message::Status(name) if name == selected)
        );
        protocol::desktop::shutdown(&channel.shutdown);
    }
    server_a.join().unwrap();
    server_b.join().unwrap();
    assert_eq!(cfg.devices.len(), 2);
}
