#![no_main]

use libfuzzer_sys::fuzz_target;
use mcrs_protocol::{
    AdvertAppData, AdvertPayload, AnonymousRequestPayload, ControlPayload, DirectEncryptedPayload,
    GroupEncryptedPayload, MultipartPayload, PathPlaintext, RepeaterLoginResponsePlaintext,
    RepeaterResponsePlaintext, RepeaterSubRequestPlaintext, RequestPlaintext, TextMessagePlaintext,
    TracePayload,
};

fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };

    match selector % 14 {
        0 => {
            if let Ok(value) = AdvertAppData::decode(payload) {
                let encoded = value.encode().expect("decoded app data should encode");
                assert_eq!(AdvertAppData::decode(&encoded), Ok(value));
            }
        }
        1 => {
            if let Ok(value) = AdvertPayload::decode(payload) {
                let _ = value.verify_signature();
            }
        }
        2 => {
            let _ = AnonymousRequestPayload::decode(payload);
        }
        3 => {
            let _ = ControlPayload::decode(payload).map(|value| value.message());
        }
        4 => {
            let _ = DirectEncryptedPayload::decode(payload);
        }
        5 => {
            let _ = GroupEncryptedPayload::decode(payload);
        }
        6 => {
            let _ = MultipartPayload::decode(payload);
        }
        7 => {
            if let Ok(value) = PathPlaintext::decode(payload) {
                let encoded = value
                    .encode()
                    .expect("decoded path plaintext should encode");
                assert_eq!(PathPlaintext::decode(&encoded), Ok(value));
            }
        }
        8 => {
            let _ = RepeaterLoginResponsePlaintext::decode(payload);
        }
        9 => {
            let _ = RepeaterResponsePlaintext::decode(payload);
        }
        10 => {
            if let Ok(value) = RepeaterSubRequestPlaintext::decode(payload) {
                let encoded = value
                    .encode()
                    .expect("decoded repeater sub-request should encode");
                assert_eq!(RepeaterSubRequestPlaintext::decode(&encoded), Ok(value));
            }
        }
        11 => {
            let _ = RequestPlaintext::decode(payload);
        }
        12 => {
            if let Ok(value) = TextMessagePlaintext::decode(payload) {
                let encoded = value.encode().expect("decoded text message should encode");
                assert_eq!(TextMessagePlaintext::decode(&encoded), Ok(value));
            }
        }
        _ => {
            let _ = TracePayload::decode(payload);
        }
    }
});
