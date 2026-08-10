//! Talking to `alien-daemon` over a Unix socket.
//!
//! # The wire format
//!
//! One line per message, ASCII, no framing beyond `\n`:
//!
//! ```text
//! ->  CALL 0e 090000820000000000000000
//! <-  OK 0000000000000000
//!
//! ->  CALL 16 0601
//! <-  ERR misc-setting sub-index 6 writes a byte that persists ...
//! ```
//!
//! Deliberately not JSON or a binary protocol. It is debuggable with
//! `socat - UNIX-CONNECT:/run/alien/alien.sock`, it needs no dependency in a
//! crate that gets vendored into six packaging formats, and the payloads are
//! at most eight bytes — there is nothing here a schema would earn its keep on.
//!
//! Hex is lowercase and unprefixed, with two characters per byte, so an empty
//! payload is an empty field rather than a special case.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::transport::{Transport, TransportError};

/// Default socket path. Overridable with `ALIEN_SOCKET`, mainly so the daemon
/// can be exercised by tests without touching `/run`.
pub const DEFAULT_SOCKET: &str = "/run/alien/alien.sock";

pub fn socket_path() -> PathBuf {
    std::env::var_os("ALIEN_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET))
}

pub struct SocketClient {
    path: PathBuf,
    // One request/response pair at a time. The daemon serialises anyway, but a
    // shared client handed to a GUI's render thread and its poller would
    // otherwise interleave two half-written lines on one stream.
    conn: Mutex<BufReader<UnixStream>>,
}

impl SocketClient {
    pub fn connect() -> Result<Self, TransportError> {
        Self::connect_to(&socket_path())
    }

    pub fn connect_to(path: &Path) -> Result<Self, TransportError> {
        let stream = UnixStream::connect(path)?;
        // Without a timeout a wedged daemon hangs the caller forever, which in
        // a GUI means a frozen window with no explanation.
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(SocketClient {
            path: path.to_path_buf(),
            conn: Mutex::new(BufReader::new(stream)),
        })
    }
}

impl Transport for SocketClient {
    fn call_bytes(&self, function: u32, buf: &[u8]) -> Result<Vec<u8>, TransportError> {
        let line = format!("CALL {:02x} {}\n", function, encode_hex(buf));
        let mut guard = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        guard.get_mut().write_all(line.as_bytes())?;
        guard.get_mut().flush()?;

        let mut resp = String::new();
        let n = guard.read_line(&mut resp)?;
        if n == 0 {
            return Err(TransportError::AcpiFailure(
                "alien-daemon closed the connection".into(),
            ));
        }
        parse_response(resp.trim_end())
    }

    fn describe(&self) -> String {
        format!("alien-daemon at {}", self.path.display())
    }
}

/// Parse one `OK <hex>` / `ERR <message>` reply.
pub fn parse_response(line: &str) -> Result<Vec<u8>, TransportError> {
    match line.split_once(' ') {
        Some(("OK", hex)) => decode_hex(hex.trim())
            .ok_or_else(|| TransportError::AcpiFailure(format!("malformed hex in reply: {hex}"))),
        Some(("ERR", msg)) => Err(TransportError::AcpiFailure(msg.trim().to_string())),
        // "OK" alone: a call whose reply carried no payload.
        None if line.trim() == "OK" => Ok(Vec::new()),
        _ => Err(TransportError::AcpiFailure(format!(
            "unintelligible reply: {line}"
        ))),
    }
}

/// Parse one `CALL <fn-hex> <payload-hex>` request. Used by the daemon.
pub fn parse_request(line: &str) -> Option<(u32, Vec<u8>)> {
    let mut parts = line.trim().split_whitespace();
    if parts.next()? != "CALL" {
        return None;
    }
    let function = u32::from_str_radix(parts.next()?, 16).ok()?;
    // A missing payload field and an empty one mean the same thing.
    let payload = match parts.next() {
        Some(h) => decode_hex(h)?,
        None => Vec::new(),
    };
    Some((function, payload))
}

pub fn encode_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrips_including_empty() {
        for case in [vec![], vec![0x00], vec![0x09, 0x00, 0x82, 0xff]] {
            assert_eq!(decode_hex(&encode_hex(&case)), Some(case));
        }
    }

    #[test]
    fn rejects_odd_length_and_garbage_hex() {
        assert_eq!(decode_hex("abc"), None);
        assert_eq!(decode_hex("zz"), None);
    }

    #[test]
    fn request_roundtrips_through_the_wire_format() {
        let word = 0x820009u64.to_le_bytes();
        let line = format!("CALL {:02x} {}", 14, encode_hex(&word));
        assert_eq!(parse_request(&line), Some((14, word.to_vec())));
    }

    #[test]
    fn request_accepts_a_missing_payload() {
        assert_eq!(parse_request("CALL 05"), Some((5, vec![])));
    }

    #[test]
    fn request_rejects_anything_that_is_not_a_call() {
        assert_eq!(parse_request("QUIT"), None);
        assert_eq!(parse_request("CALL notahexnumber ff"), None);
    }

    #[test]
    fn ok_and_err_replies_are_distinguished() {
        assert_eq!(parse_response("OK 00fa16").unwrap(), vec![0x00, 0xfa, 0x16]);
        assert_eq!(parse_response("OK").unwrap(), Vec::<u8>::new());
        let e = parse_response("ERR nope").unwrap_err();
        assert!(e.to_string().contains("nope"));
    }

    #[test]
    fn an_error_reply_is_never_mistaken_for_data() {
        // The whole point of the two-word format: a daemon refusal must not
        // decode into a plausible-looking response buffer.
        assert!(parse_response("ERR deadbeef").is_err());
    }
}
