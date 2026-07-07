#![no_main]

use libfuzzer_sys::fuzz_target;
use mcrs_protocol::{ControlMessage, Packet, Payload, RoutePath};

fuzz_target!(|data: &[u8]| {
    let Ok(packet) = Packet::decode(data) else {
        return;
    };

    let encoded = packet
        .encode()
        .expect("decoded packet should re-encode successfully");
    let reparsed = Packet::decode(&encoded).expect("encoded packet should decode successfully");
    assert_eq!(reparsed, packet);

    let _ = packet.dedup_signature();

    if let RoutePath::Normal(path) = &packet.path {
        let _ = path.contains_hash(&[0, 1, 2, 3]);
        let _ = path.first_hash_matches(&[0, 1, 2, 3]);
    }

    match &packet.payload {
        Payload::Control(payload) => match payload.message() {
            Ok(ControlMessage::Unknown(_)) | Err(_) => {}
            Ok(ControlMessage::DiscoverRequest { .. })
            | Ok(ControlMessage::DiscoverResponse { .. }) => {}
        },
        Payload::Advert(payload) => {
            let _ = payload.verify_signature();
        }
        Payload::Trace(_) => {
            let _ = packet.trace_is_complete();
            let _ = packet.trace_next_hop_matches(&[0, 1, 2, 3]);
        }
        _ => {}
    }
});
