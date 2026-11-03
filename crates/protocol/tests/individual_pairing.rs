use protocol::{access::AccessStore, pairing};
use std::{
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    (client, listener.accept().unwrap().0)
}

#[test]
fn approved_pairing_binds_both_identities_and_transfers_only_its_individual_key() {
    let store = AccessStore::in_memory("previous-shared-key");
    let server_store = store.clone();
    let (mut client, mut server) = pair();
    let server = thread::spawn(move || {
        pairing::pair_responder_authorized_with_identity(
            &mut server,
            5599,
            "PC",
            "host-id",
            |req| {
                assert_eq!(req.device_id, "controller-id");
                Ok(Some(server_store.grant(&req.peer_name, &req.device_id)?))
            },
        )
        .unwrap()
    });
    let mut code = String::new();
    let outcome = pairing::pair_initiator_with_identity(&mut client, "Mac", "controller-id", |c| {
        code = c.into()
    })
    .unwrap();
    assert_eq!(outcome.peer_device_id, "host-id");
    assert_ne!(outcome.secret, "previous-shared-key");
    assert_eq!(store.controllers().iter().filter(|c| !c.legacy).count(), 1);
    let pairing::PairDecision::Accepted(request) = server.join().unwrap() else {
        panic!("not accepted")
    };
    assert_eq!(request.code, code);
}

#[test]
fn persistence_failure_never_sends_an_accepted_key() {
    let (mut client, mut server) = pair();
    let server = thread::spawn(move || {
        pairing::pair_responder_authorized(&mut server, 5599, "PC", |_| {
            Err(std::io::Error::other("disk full"))
        })
    });
    let result = pairing::pair_initiator_with_identity(&mut client, "Mac", "id", |_| {});
    assert!(result.is_err());
    assert!(server.join().unwrap().is_err());
}

#[test]
fn legacy_pairing_still_works_and_has_no_self_asserted_stable_identity() {
    let (mut client, mut server) = pair();
    let server = thread::spawn(move || {
        pairing::pair_responder_authorized_with_identity(
            &mut server,
            5599,
            "PC",
            "host-id",
            |req| {
                assert!(req.device_id.is_empty());
                Ok(Some("dedicated-old-client-key".into()))
            },
        )
        .unwrap()
    });
    let outcome = pairing::pair_initiator(&mut client, "Old Mac", |_| {}).unwrap();
    assert_eq!(outcome.secret, "dedicated-old-client-key");
    server.join().unwrap();
}
