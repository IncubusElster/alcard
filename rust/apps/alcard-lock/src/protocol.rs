//! In-process UI commands and the bounded private PAM pipe protocol.
use crate::secret::Secret;
use std::io::{self, Read, Write};
use zeroize::Zeroizing;

pub(crate) const MAX_PACKET: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Denied,
    Cancelled,
    Timeout,
    Unavailable,
    AccountDenied,
    IdentityChanged,
}

#[derive(Debug)]
pub enum Event {
    Ready {
        uid: u32,
        user: String,
    },
    Challenge {
        attempt: u64,
        serial: u64,
        prompt: String,
        echo: bool,
    },
    Notice {
        attempt: u64,
        text: String,
        error: bool,
    },
    Finished {
        attempt: u64,
        result: Result<(), Failure>,
    },
    Fatal(Failure),
}

pub enum Command {
    Begin {
        attempt: u64,
    },
    Respond {
        attempt: u64,
        serial: u64,
        secret: Secret,
    },
    Cancel {
        attempt: u64,
    },
    Shutdown,
}

pub fn plain(value: &str, limit: usize) -> String {
    value.chars().filter(|c| !c.is_control() && !matches!(*c,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )).take(limit).collect()
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid private authentication frame",
    )
}

fn failure_code(failure: Failure) -> u8 {
    match failure {
        Failure::Denied => 1,
        Failure::Cancelled => 2,
        Failure::Timeout => 3,
        Failure::Unavailable => 4,
        Failure::AccountDenied => 5,
        Failure::IdentityChanged => 6,
    }
}

fn failure(value: u8) -> io::Result<Failure> {
    match value {
        1 => Ok(Failure::Denied),
        2 => Ok(Failure::Cancelled),
        3 => Ok(Failure::Timeout),
        4 => Ok(Failure::Unavailable),
        5 => Ok(Failure::AccountDenied),
        6 => Ok(Failure::IdentityChanged),
        _ => Err(invalid()),
    }
}

struct Encoder(Zeroizing<Vec<u8>>);
impl Encoder {
    fn new(kind: u8) -> Self {
        let mut value = Zeroizing::new(Vec::with_capacity(MAX_PACKET + 4));
        value.extend_from_slice(&[0; 4]);
        value.push(kind);
        Self(value)
    }
    fn byte(&mut self, value: u8) {
        self.0.push(value);
    }
    fn number(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }
    fn bytes(&mut self, value: &[u8]) -> io::Result<()> {
        if value.len() > 1024 || self.0.len() + value.len() + 2 > MAX_PACKET + 4 {
            return Err(invalid());
        }
        self.0
            .extend_from_slice(&(value.len() as u16).to_be_bytes());
        self.0.extend_from_slice(value);
        Ok(())
    }
    fn finish(mut self) -> Zeroizing<Vec<u8>> {
        let length = (self.0.len() - 4) as u32;
        self.0[..4].copy_from_slice(&length.to_be_bytes());
        self.0
    }
}

struct Decoder<'a>(&'a [u8]);
impl<'a> Decoder<'a> {
    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < length {
            return Err(invalid());
        }
        let (head, tail) = self.0.split_at(length);
        self.0 = tail;
        Ok(head)
    }
    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn boolean(&mut self) -> io::Result<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid()),
        }
    }
    fn number(&mut self) -> io::Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| invalid())?,
        ))
    }
    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let length = u16::from_be_bytes(self.take(2)?.try_into().map_err(|_| invalid())?) as usize;
        if length > 1024 {
            return Err(invalid());
        }
        self.take(length)
    }
    fn text(&mut self) -> io::Result<String> {
        let text = std::str::from_utf8(self.bytes()?).map_err(|_| invalid())?;
        Ok(plain(text, 256))
    }
    fn finish(self) -> io::Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}

pub(crate) fn encode_command(command: &Command) -> io::Result<Zeroizing<Vec<u8>>> {
    let packet = match command {
        Command::Begin { attempt } => {
            let mut packet = Encoder::new(101);
            packet.number(*attempt);
            packet
        }
        Command::Respond {
            attempt,
            serial,
            secret,
        } => {
            let mut packet = Encoder::new(102);
            packet.number(*attempt);
            packet.number(*serial);
            packet.bytes(secret.as_str().as_bytes())?;
            packet
        }
        _ => return Err(invalid()),
    };
    // A zero generation can never authorize a conversation.
    if packet.0[5..13] == [0; 8] {
        return Err(invalid());
    }
    Ok(packet.finish())
}

pub(crate) fn decode_command(payload: &[u8]) -> io::Result<Command> {
    let mut packet = Decoder(payload);
    let kind = packet.byte()?;
    let attempt = packet.number()?;
    if attempt == 0 {
        return Err(invalid());
    }
    let command = match kind {
        101 => Command::Begin { attempt },
        102 => {
            let serial = packet.number()?;
            if serial == 0 {
                return Err(invalid());
            }
            let secret = Secret::from_bytes(packet.bytes()?).ok_or_else(invalid)?;
            Command::Respond {
                attempt,
                serial,
                secret,
            }
        }
        _ => return Err(invalid()),
    };
    packet.finish()?;
    Ok(command)
}

fn encode_event(event: &Event) -> io::Result<Zeroizing<Vec<u8>>> {
    let packet = match event {
        Event::Ready { uid, user } => {
            let mut packet = Encoder::new(1);
            packet.number(u64::from(*uid));
            packet.bytes(user.as_bytes())?;
            packet
        }
        Event::Challenge {
            attempt,
            serial,
            prompt,
            echo,
        } => {
            let mut packet = Encoder::new(2);
            packet.number(*attempt);
            packet.number(*serial);
            packet.byte(u8::from(*echo));
            packet.bytes(prompt.as_bytes())?;
            packet
        }
        Event::Notice {
            attempt,
            text,
            error,
        } => {
            let mut packet = Encoder::new(3);
            packet.number(*attempt);
            packet.byte(u8::from(*error));
            packet.bytes(text.as_bytes())?;
            packet
        }
        Event::Finished { attempt, result } => {
            let mut packet = Encoder::new(4);
            packet.number(*attempt);
            packet.byte(result.err().map_or(0, failure_code));
            packet
        }
        Event::Fatal(error) => {
            let mut packet = Encoder::new(5);
            packet.byte(failure_code(*error));
            packet
        }
    };
    Ok(packet.finish())
}

pub(crate) fn decode_event(payload: &[u8]) -> io::Result<Event> {
    let mut packet = Decoder(payload);
    let event = match packet.byte()? {
        1 => {
            let uid = u32::try_from(packet.number()?).map_err(|_| invalid())?;
            let user = packet.text()?;
            Event::Ready { uid, user }
        }
        2 => {
            let attempt = packet.number()?;
            let serial = packet.number()?;
            let echo = packet.boolean()?;
            let prompt = packet.text()?;
            Event::Challenge {
                attempt,
                serial,
                prompt,
                echo,
            }
        }
        3 => {
            let attempt = packet.number()?;
            let error = packet.boolean()?;
            let text = packet.text()?;
            Event::Notice {
                attempt,
                text,
                error,
            }
        }
        4 => {
            let attempt = packet.number()?;
            let code = packet.byte()?;
            let result = if code == 0 {
                Ok(())
            } else {
                Err(failure(code)?)
            };
            Event::Finished { attempt, result }
        }
        5 => Event::Fatal(failure(packet.byte()?)?),
        _ => return Err(invalid()),
    };
    packet.finish()?;
    Ok(event)
}

pub(crate) fn write_event(output: &mut impl Write, event: &Event) -> io::Result<()> {
    output.write_all(&encode_event(event)?)?;
    output.flush()
}

pub(crate) fn read_command(input: &mut impl Read) -> io::Result<Command> {
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_PACKET {
        return Err(invalid());
    }
    let mut payload = Zeroizing::new(vec![0; length]);
    input.read_exact(&mut payload)?;
    decode_command(&payload)
}

/// Only the controller receives these packets; none contain responses.
pub(crate) fn next_event(buffer: &mut Vec<u8>) -> io::Result<Option<Event>> {
    if buffer.len() < 4 {
        return Ok(None);
    }
    let length = u32::from_be_bytes(buffer[..4].try_into().map_err(|_| invalid())?) as usize;
    if length == 0 || length > MAX_PACKET {
        return Err(invalid());
    }
    if buffer.len() < length + 4 {
        return Ok(None);
    }
    let event = decode_event(&buffer[4..length + 4])?;
    buffer.drain(..length + 4);
    Ok(Some(event))
}

#[cfg(test)]
#[path = "unit_tests/protocol.rs"]
mod tests;
