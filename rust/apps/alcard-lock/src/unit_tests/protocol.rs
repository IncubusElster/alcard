use super::*;

#[test]
fn secret_frame_roundtrip_preserves_unicode_and_generations() {
    let secret = Secret::from_bytes("4 5 6 7 8 Пароль año".as_bytes()).unwrap();
    let encoded = encode_command(&Command::Respond {
        attempt: 7,
        serial: 3,
        secret,
    })
    .unwrap();
    let Command::Respond {
        attempt,
        serial,
        secret,
    } = read_command(&mut &encoded[..]).unwrap()
    else {
        panic!("wrong command")
    };
    assert_eq!((attempt, serial), (7, 3));
    assert_eq!(secret.as_str(), "4 5 6 7 8 Пароль año");
    assert!(encode_command(&Command::Begin { attempt: 0 }).is_err());
    assert!(decode_command(&[101, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
}

#[test]
fn truncated_oversized_and_invalid_secret_packets_fail_closed() {
    let packet = encode_command(&Command::Begin { attempt: 8 }).unwrap();
    for end in 0..packet.len() {
        assert!(read_command(&mut &packet[..end]).is_err());
    }
    assert!(read_command(&mut &(u32::MAX.to_be_bytes())[..]).is_err());
    let mut invalid_secret = Encoder::new(102);
    invalid_secret.number(8);
    invalid_secret.number(1);
    invalid_secret.bytes(b"password\0suffix").unwrap();
    assert!(read_command(&mut &invalid_secret.finish()[..]).is_err());
    let mut trailing = packet[4..].to_vec();
    trailing.push(0);
    assert!(decode_command(&trailing).is_err());
}

#[test]
fn fragmented_events_do_not_emit_an_early_success() {
    let packet = encode_event(&Event::Finished {
        attempt: 91,
        result: Ok(()),
    })
    .unwrap();
    let mut buffer = Vec::new();
    for (index, byte) in packet.iter().enumerate() {
        buffer.push(*byte);
        let event = next_event(&mut buffer).unwrap();
        if index + 1 < packet.len() {
            assert!(event.is_none());
        } else {
            assert!(matches!(
                event,
                Some(Event::Finished {
                    attempt: 91,
                    result: Ok(())
                })
            ));
        }
    }
    assert!(buffer.is_empty());
    let mut corrupted = packet[4..].to_vec();
    *corrupted.last_mut().unwrap() = 255;
    assert!(decode_event(&corrupted).is_err());
}

#[test]
fn public_prompts_are_bounded_and_cannot_embed_bidi_or_controls() {
    let event = Event::Challenge {
        attempt: 1,
        serial: 2,
        prompt: "a\u{202e}b\n\u{1b}c".into(),
        echo: true,
    };
    let encoded = encode_event(&event).unwrap();
    let Event::Challenge { prompt, echo, .. } = decode_event(&encoded[4..]).unwrap() else {
        panic!("wrong event")
    };
    assert_eq!(prompt, "abc");
    assert!(echo);
    let oversized = Event::Notice {
        attempt: 1,
        text: "x".repeat(MAX_PACKET),
        error: false,
    };
    assert!(encode_event(&oversized).is_err());
}
