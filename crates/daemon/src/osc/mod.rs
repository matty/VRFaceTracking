pub mod parameters;
pub mod query;
pub mod resonite;
pub mod vrchat;

use rosc::{encoder, OscMessage, OscPacket};

/// The most a bundle sends in one datagram. A larger one is split into IP
/// fragments, and losing any one of them loses the whole bundle.
pub const MAX_BUNDLE_BYTES: usize = 1400;

/// `#bundle`, then the time tag for "immediately".
const BUNDLE_HEADER: [u8; 16] = *b"#bundle\0\0\0\0\0\0\0\0\x01";

/// `messages` as OSC bundles of at most [`MAX_BUNDLE_BYTES`] each, in
/// order; a message too big for one goes in a bundle of its own.
pub fn encode_bundles(messages: Vec<OscMessage>) -> Result<Vec<Vec<u8>>, rosc::OscError> {
    let mut bundles: Vec<Vec<u8>> = Vec::new();
    for message in messages {
        let encoded = encoder::encode(&OscPacket::Message(message))?;
        let element = 4 + encoded.len();
        match bundles.last_mut() {
            Some(bundle) if bundle.len() + element <= MAX_BUNDLE_BYTES => {}
            _ => bundles.push(BUNDLE_HEADER.to_vec()),
        }
        let bundle = bundles.last_mut().expect("just pushed one");
        bundle.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        bundle.extend_from_slice(&encoded);
    }
    Ok(bundles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{decoder, OscType};

    #[test]
    fn many_messages_split_into_bundles_that_fit_a_datagram() {
        let messages: Vec<OscMessage> = (0..200)
            .map(|index| OscMessage {
                addr: format!("/avatar/parameters/FT/v2/Shape{index}"),
                args: vec![OscType::Float(index as f32)],
            })
            .collect();
        let bundles = encode_bundles(messages.clone()).unwrap();
        assert!(bundles.len() > 1);
        let mut decoded = Vec::new();
        for bytes in &bundles {
            assert!(bytes.len() <= MAX_BUNDLE_BYTES);
            let (_, packet) = decoder::decode_udp(bytes).unwrap();
            let OscPacket::Bundle(bundle) = packet else {
                panic!("not a bundle");
            };
            assert_eq!(bundle.timetag, rosc::OscTime::from((0, 1)));
            decoded.extend(bundle.content.into_iter().map(|packet| match packet {
                OscPacket::Message(message) => message,
                OscPacket::Bundle(_) => panic!("nested bundle"),
            }));
        }
        assert_eq!(decoded, messages);
        assert!(encode_bundles(Vec::new()).unwrap().is_empty());
    }
}
