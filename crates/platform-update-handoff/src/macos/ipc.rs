use super::{Error, Result, SessionBinding};
use platform_update_witness::{ProcessIdentity, SessionNonce};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    thread,
    time::{Duration, Instant},
};
const MAX_FRAME: usize = 4096;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WireBinding {
    version: u8,
    nonce: String,
    pid: i32,
    start_seconds: u64,
    start_microseconds: u64,
    effective_uid: u32,
}
impl From<&SessionBinding> for WireBinding {
    fn from(binding: &SessionBinding) -> Self {
        Self {
            version: 1,
            nonce: binding.nonce.clone(),
            pid: binding.parent.pid(),
            start_seconds: binding.parent.start_seconds(),
            start_microseconds: binding.parent.start_microseconds(),
            effective_uid: binding.parent.effective_uid(),
        }
    }
}
impl WireBinding {
    pub fn checked(self) -> Result<SessionBinding> {
        if self.version != 1 {
            return Err(Error::Protocol);
        }
        SessionNonce::parse(&self.nonce).map_err(|_| Error::Protocol)?;
        let parent = ProcessIdentity::from_parts(
            self.pid,
            self.start_seconds,
            self.start_microseconds,
            self.effective_uid,
        )
        .map_err(|_| Error::Protocol)?;
        Ok(SessionBinding {
            parent,
            nonce: self.nonce,
        })
    }
    pub fn matches(&self, binding: &SessionBinding) -> bool {
        self.clone().checked().is_ok_and(|b| &b == binding)
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(super) enum Message {
    Hello { binding: WireBinding },
    Registered { binding: WireBinding },
    Drain { binding: WireBinding },
    Accepted { binding: WireBinding },
    Arm { binding: WireBinding },
    Armed { binding: WireBinding },
    Cancel {},
}
pub(super) fn write(pipe: &mut UnixStream, message: &Message, budget: Duration) -> Result<()> {
    let payload = serde_json::to_vec(message).map_err(|_| Error::InvalidFrame)?;
    if payload.len() > MAX_FRAME {
        return Err(Error::InvalidFrame);
    }
    let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
    bytes.extend(payload);
    let deadline = Instant::now() + budget;
    let mut offset = 0;
    while offset < bytes.len() {
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        match pipe.write(&bytes[offset..]) {
            Ok(0) => return Err(Error::EndOfPipe),
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(Error::Io),
        }
    }
    Ok(())
}
#[derive(Default)]
pub(super) struct Reader {
    bytes: Vec<u8>,
}
impl Reader {
    pub fn ensure_empty(&self) -> Result<()> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(Error::InvalidFrame)
        }
    }
    pub fn next(&mut self, pipe: &mut UnixStream) -> Result<Option<Message>> {
        // Partial frames survive WouldBlock and are never reinterpreted as a
        // new header. Malformed/truncated input permanently ends the protocol.
        loop {
            if self.bytes.len() >= 4 {
                let size = u32::from_be_bytes(
                    self.bytes[..4]
                        .try_into()
                        .map_err(|_| Error::InvalidFrame)?,
                ) as usize;
                if size == 0 || size > MAX_FRAME {
                    return Err(Error::InvalidFrame);
                }
                if self.bytes.len() >= size + 4 {
                    let message = serde_json::from_slice(&self.bytes[4..size + 4])
                        .map_err(|_| Error::InvalidFrame)?;
                    self.bytes.drain(..size + 4);
                    return Ok(Some(message));
                }
            }
            let mut chunk = [0; 512];
            match pipe.read(&mut chunk) {
                Ok(0) => {
                    self.ensure_empty()?;
                    return Err(Error::EndOfPipe);
                }
                Ok(count) => {
                    self.bytes.extend_from_slice(&chunk[..count]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Error::Io),
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_strict_frames_reject_unknown_duplicate_truncated_and_oversize() {
        for payload in [
            r#"{"kind":"ReadyForNativeExit"}"#,
            r#"{"kind":"Cancel","extra":true}"#,
            r#"{"kind":"Cancel","kind":"Cancel"}"#,
        ] {
            let (mut writer, mut reader_pipe) = UnixStream::pair().unwrap();
            reader_pipe.set_nonblocking(true).unwrap();
            writer
                .write_all(&(payload.len() as u32).to_be_bytes())
                .unwrap();
            writer.write_all(payload.as_bytes()).unwrap();
            assert!(matches!(
                Reader::default().next(&mut reader_pipe),
                Err(Error::InvalidFrame)
            ));
        }
        for bytes in [vec![0, 0, 16, 1], vec![0, 0, 0, 9, b'{'], vec![0, 0]] {
            let (mut writer, mut pipe) = UnixStream::pair().unwrap();
            pipe.set_nonblocking(true).unwrap();
            writer.write_all(&bytes).unwrap();
            drop(writer);
            assert!(matches!(
                Reader::default().next(&mut pipe),
                Err(Error::InvalidFrame)
            ));
        }
    }
    #[test]
    fn partial_frame_is_preserved_across_poll_boundaries() {
        let (mut writer, mut pipe) = UnixStream::pair().unwrap();
        pipe.set_nonblocking(true).unwrap();
        let payload = br#"{"kind":"Cancel"}"#;
        writer
            .write_all(&(payload.len() as u32).to_be_bytes())
            .unwrap();
        writer.write_all(&payload[..3]).unwrap();
        let mut reader = Reader::default();
        assert!(reader.next(&mut pipe).unwrap().is_none());
        writer.write_all(&payload[3..]).unwrap();
        assert!(matches!(
            reader.next(&mut pipe).unwrap(),
            Some(Message::Cancel {})
        ));
    }
}
