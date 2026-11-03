use protocol::{
    capabilities::{Capabilities, SessionCapabilities},
    desktop::{Channel, Message},
    devices::Platform,
    secure,
};
use std::{
    net::{TcpListener, TcpStream},
    thread,
};

fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    (client, listener.accept().unwrap().0)
}

#[test]
fn authenticated_endpoints_negotiate_view_only_without_clipboard() {
    let (client, server) = pair();
    let mut host = Capabilities::host(Platform::Linux);
    host.receive_input = false;
    host.clipboard = false;
    let expected = host.clone();
    let server = thread::spawn(move || {
        let (mut channel, peer, index) =
            Channel::accept(server, &[[8; 32], [4; 32]], "linux-host", &host).unwrap();
        assert_eq!(index, 1);
        assert_eq!(peer, "mac-controller");
        assert_eq!(
            channel.negotiated,
            SessionCapabilities {
                screen: true,
                input: false,
                clipboard: false
            }
        );
        channel.writer.send(&Message::Heartbeat).unwrap();
    });
    let (mut channel, id) = Channel::connect_with_capabilities(
        client,
        &[4; 32],
        "mac-controller",
        &Capabilities::controller(Platform::Macos),
    )
    .unwrap();
    assert_eq!(id, "linux-host");
    assert_eq!(channel.peer_capabilities, Some(expected));
    assert!(!channel.negotiated.input && !channel.negotiated.clipboard);
    assert_eq!(channel.reader.recv().unwrap(), Message::Heartbeat);
    server.join().unwrap();
}

#[test]
fn new_controller_connects_to_old_host_without_sending_extra_records() {
    let (client, server) = pair();
    let server = thread::spawn(move || {
        let (mut channel, _) = Channel::connect(server, &[1; 32], "old-host", false).unwrap();
        assert_eq!(channel.reader.recv().unwrap(), Message::Ping(99));
        channel.writer.send(&Message::Pong(99)).unwrap();
    });
    let (mut channel, _) = Channel::connect_with_capabilities(
        client,
        &[1; 32],
        "new-controller",
        &Capabilities::controller(Platform::Macos),
    )
    .unwrap();
    assert_eq!(channel.peer_capabilities, None);
    channel.writer.send(&Message::Ping(99)).unwrap();
    assert_eq!(channel.reader.recv().unwrap(), Message::Pong(99));
    server.join().unwrap();
}

#[test]
fn old_controller_connects_to_new_host_using_a_dedicated_key() {
    let (client, server) = pair();
    let server = thread::spawn(move || {
        let (mut channel, _, index) = Channel::accept(
            server,
            &[[2; 32], [3; 32]],
            "new-host",
            &Capabilities::host(Platform::Windows),
        )
        .unwrap();
        assert_eq!(index, 1);
        assert!(channel.peer_capabilities.is_none());
        assert_eq!(channel.reader.recv().unwrap(), Message::Ping(1));
        channel.writer.send(&Message::Pong(1)).unwrap();
    });
    let (mut channel, _) = Channel::connect(client, &[3; 32], "old-controller", true).unwrap();
    channel.writer.send(&Message::Ping(1)).unwrap();
    assert_eq!(channel.reader.recv().unwrap(), Message::Pong(1));
    server.join().unwrap();
}

#[test]
fn unsupported_codec_disables_the_desktop_and_unknown_capability_bits_are_ignored() {
    let controller = Capabilities::controller(Platform::Macos);
    let mut host = Capabilities::host(Platform::Windows);
    host.encode_h264 = false;
    assert_eq!(
        SessionCapabilities::negotiate(&controller, &host),
        SessionCapabilities {
            screen: false,
            input: false,
            clipboard: false
        }
    );
    let mut encoded = controller.encode();
    encoded[2] = 0xff;
    assert_eq!(Capabilities::decode(&encoded).unwrap(), controller);
    assert!(Capabilities::decode(&[2, 0, 0, 0]).is_err());
    assert!(Message::decode(&[14, 1]).is_err());
}

#[test]
fn keyring_limit_is_checked_before_reading_network_data() {
    let mut io = std::io::Cursor::new(Vec::<u8>::new());
    assert_eq!(
        secure::handshake_responder_many(&mut io, &vec![[0; 32]; 34], 0)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
}
