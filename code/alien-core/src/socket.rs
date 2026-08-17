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

use crate::performance::{
    GpuMode, GpuModeOptIn, GpuModeState, GpuOffsetRange, GPU_MODE_ACKNOWLEDGEMENT,
};
use crate::transport::{KeyboardTimeoutState, Transport, TransportError};

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
    // `None` means a prior I/O/framing failure made request/reply alignment
    // unknowable. The connection is permanently discarded so a late reply
    // can never satisfy a later request.
    conn: Mutex<Option<BufReader<UnixStream>>>,
}

enum ExchangeFailure {
    /// A complete `ERR ...\n` line was consumed, so the next exchange remains
    /// aligned even though the requested operation failed.
    Synchronized(TransportError),
    /// A write/read/framing failure may have left a partial request or reply.
    /// The stream must be closed and never reused.
    Desynchronized(TransportError),
}

impl SocketClient {
    pub fn connect() -> Result<Self, TransportError> {
        Self::connect_to(&socket_path())
    }

    pub fn connect_to(path: &Path) -> Result<Self, TransportError> {
        Self::connect_to_with_timeout(path, Duration::from_secs(5))
    }

    fn connect_to_with_timeout(path: &Path, timeout: Duration) -> Result<Self, TransportError> {
        let stream = UnixStream::connect(path)?;
        // Without a timeout a wedged daemon hangs the caller forever, which in
        // a GUI means a frozen window with no explanation.
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(SocketClient {
            path: path.to_path_buf(),
            conn: Mutex::new(Some(BufReader::new(stream))),
        })
    }

    fn exchange(&self, line: &str) -> Result<Vec<u8>, TransportError> {
        let mut guard = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let Some(connection) = guard.as_mut() else {
            return Err(TransportError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "alien-daemon connection was discarded after an earlier I/O or protocol failure; reopen Device",
            )));
        };

        let result = Self::exchange_once(connection, line);
        match result {
            Ok(response) => Ok(response),
            Err(ExchangeFailure::Synchronized(error)) => Err(error),
            Err(ExchangeFailure::Desynchronized(error)) => {
                // Dropping the stream is essential: after a timeout the daemon
                // may still finish and send a valid-looking late response.
                // Reusing that stream could pair it with the next request.
                *guard = None;
                Err(error)
            }
        }
    }

    fn exchange_once(
        connection: &mut BufReader<UnixStream>,
        line: &str,
    ) -> Result<Vec<u8>, ExchangeFailure> {
        let desynchronized_io =
            |error: std::io::Error| ExchangeFailure::Desynchronized(TransportError::Io(error));

        connection
            .get_mut()
            .write_all(line.as_bytes())
            .map_err(desynchronized_io)?;
        connection
            .get_mut()
            .write_all(b"\n")
            .map_err(desynchronized_io)?;
        connection.get_mut().flush().map_err(desynchronized_io)?;

        let mut resp = String::new();
        let n = connection.read_line(&mut resp).map_err(desynchronized_io)?;
        if n == 0 {
            return Err(ExchangeFailure::Desynchronized(TransportError::Io(
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "alien-daemon closed the connection",
                ),
            )));
        }
        let line = resp.trim_end();
        match parse_response(line) {
            Ok(response) => Ok(response),
            // A complete daemon error line preserves framing. Malformed OK or
            // any other response is a protocol failure: discard the stream.
            Err(error) if line.starts_with("ERR ") => Err(ExchangeFailure::Synchronized(error)),
            Err(error) => Err(ExchangeFailure::Desynchronized(error)),
        }
    }
}

impl Transport for SocketClient {
    fn call_bytes(&self, function: u32, buf: &[u8]) -> Result<Vec<u8>, TransportError> {
        self.exchange(&format!("CALL {:02x} {}", function, encode_hex(buf)))
    }

    fn describe(&self) -> String {
        format!("alien-daemon at {}", self.path.display())
    }

    fn coolboost(&self) -> Result<bool, TransportError> {
        parse_bool_feature(self.exchange("FEATURE coolboost get")?, "CoolBoost")
    }

    fn set_coolboost(&self, enabled: bool) -> Result<bool, TransportError> {
        parse_bool_feature(
            self.exchange(&format!("FEATURE coolboost set {}", enabled as u8))?,
            "CoolBoost",
        )
    }

    fn keyboard_timeout(&self) -> Result<KeyboardTimeoutState, TransportError> {
        parse_timeout(self.exchange("FEATURE keyboard-timeout get")?)
    }

    fn set_keyboard_timeout(&self, seconds: u8) -> Result<KeyboardTimeoutState, TransportError> {
        parse_timeout(self.exchange(&format!("FEATURE keyboard-timeout set {seconds}"))?)
    }

    fn lcd_overdrive(&self) -> Result<Option<bool>, TransportError> {
        parse_optional_bool_feature(self.exchange("FEATURE lcd-overdrive get")?, "LCD overdrive")
    }

    fn set_lcd_overdrive(&self, enabled: bool) -> Result<Option<bool>, TransportError> {
        parse_optional_bool_feature(
            self.exchange(&format!("FEATURE lcd-overdrive set {}", enabled as u8))?,
            "LCD overdrive",
        )
    }

    fn gpu_mode(&self) -> Result<GpuModeState, TransportError> {
        parse_gpu_mode_state(self.exchange("FEATURE gpu-mode get")?)
    }

    fn set_gpu_mode(
        &self,
        mode: GpuMode,
        _opt_in: GpuModeOptIn,
    ) -> Result<GpuModeState, TransportError> {
        parse_gpu_mode_state(self.exchange(&format!(
            "FEATURE gpu-mode set {} {GPU_MODE_ACKNOWLEDGEMENT}",
            mode.label()
        ))?)
    }
}

fn parse_bool_feature(bytes: Vec<u8>, name: &str) -> Result<bool, TransportError> {
    match bytes.as_slice() {
        [0] => Ok(false),
        [1] => Ok(true),
        _ => Err(TransportError::AcpiFailure(format!(
            "daemon returned malformed {name} state: {}",
            encode_hex(&bytes)
        ))),
    }
}

fn parse_optional_bool_feature(bytes: Vec<u8>, name: &str) -> Result<Option<bool>, TransportError> {
    match bytes.as_slice() {
        [] => Ok(None),
        [0] => Ok(Some(false)),
        [1] => Ok(Some(true)),
        _ => Err(TransportError::AcpiFailure(format!(
            "daemon returned malformed {name} state: {}",
            encode_hex(&bytes)
        ))),
    }
}

fn parse_timeout(bytes: Vec<u8>) -> Result<KeyboardTimeoutState, TransportError> {
    match bytes.as_slice() {
        [brightness, seconds @ (0 | 30)] => Ok(KeyboardTimeoutState {
            brightness: *brightness,
            seconds: *seconds,
        }),
        _ => Err(TransportError::AcpiFailure(format!(
            "daemon returned malformed keyboard-timeout state: {}",
            encode_hex(&bytes)
        ))),
    }
}

/// Stable v1 binary shape used only inside the daemon socket's `OK <hex>`
/// envelope: version, six signed i32 offset/range values, fan table and GPOC.
pub fn encode_gpu_mode_state(state: GpuModeState) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(27);
    bytes.push(1);
    for value in [
        state.graphics.current_mhz,
        state.graphics.min_mhz,
        state.graphics.max_mhz,
        state.memory.current_mhz,
        state.memory.min_mhz,
        state.memory.max_mhz,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.push(state.fan_table);
    bytes.push(state.gpoc);
    bytes
}

fn parse_gpu_mode_state(bytes: Vec<u8>) -> Result<GpuModeState, TransportError> {
    if bytes.len() != 27 || bytes.first() != Some(&1) {
        return Err(TransportError::AcpiFailure(format!(
            "daemon returned malformed GPU-mode state: {}",
            encode_hex(&bytes)
        )));
    }
    let mut at = 1;
    let mut next_i32 = || {
        let value = i32::from_le_bytes(bytes[at..at + 4].try_into().expect("length checked"));
        at += 4;
        value
    };
    let state = GpuModeState {
        graphics: GpuOffsetRange {
            current_mhz: next_i32(),
            min_mhz: next_i32(),
            max_mhz: next_i32(),
        },
        memory: GpuOffsetRange {
            current_mhz: next_i32(),
            min_mhz: next_i32(),
            max_mhz: next_i32(),
        },
        fan_table: bytes[25],
        gpoc: bytes[26],
    };
    if state.fan_table > 4 || state.gpoc > 2 {
        return Err(TransportError::AcpiFailure(format!(
            "daemon returned out-of-domain GPU-mode firmware state: table {}, GPOC {}",
            state.fan_table, state.gpoc
        )));
    }
    for (label, range) in [("graphics", state.graphics), ("memory", state.memory)] {
        if range.min_mhz > range.max_mhz
            || !(range.min_mhz..=range.max_mhz).contains(&range.current_mhz)
        {
            return Err(TransportError::AcpiFailure(format!(
                "daemon returned inconsistent P0 {label} offset state: current {}, range {}..{}",
                range.current_mhz, range.min_mhz, range.max_mhz
            )));
        }
    }
    Ok(state)
}

/// Parse one `OK <hex>` / `ERR <message>` reply.
pub fn parse_response(line: &str) -> Result<Vec<u8>, TransportError> {
    match line.split_once(' ') {
        Some(("OK", hex)) => decode_hex(hex.trim())
            .ok_or_else(|| TransportError::AcpiFailure(format!("malformed hex in reply: {hex}"))),
        Some(("ERR", msg)) => {
            let msg = msg.trim();
            if let Some(tagged) = msg.strip_prefix("FWSTATUS ") {
                let (status, operation) = tagged.split_once(' ').ok_or_else(|| {
                    TransportError::AcpiFailure(format!(
                        "malformed tagged firmware-status reply: {msg}"
                    ))
                })?;
                let status = u8::from_str_radix(status, 16).map_err(|_| {
                    TransportError::AcpiFailure(format!(
                        "malformed tagged firmware-status reply: {msg}"
                    ))
                })?;
                Err(TransportError::FirmwareStatus {
                    operation: operation.to_owned(),
                    status,
                })
            } else {
                Err(TransportError::AcpiFailure(msg.to_string()))
            }
        }
        // "OK" alone: a call whose reply carried no payload.
        None if line.trim() == "OK" => Ok(Vec::new()),
        _ => Err(TransportError::AcpiFailure(format!(
            "unintelligible reply: {line}"
        ))),
    }
}

/// Parse one `CALL <fn-hex> <payload-hex>` request. Used by the daemon.
pub fn parse_request(line: &str) -> Option<(u32, Vec<u8>)> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "CALL" {
        return None;
    }
    let function = u32::from_str_radix(parts.next()?, 16).ok()?;
    // A missing payload field and an empty one mean the same thing.
    let payload = match parts.next() {
        Some(h) => decode_hex(h)?,
        None => Vec::new(),
    };
    if parts.next().is_some() {
        return None;
    }
    Some((function, payload))
}

pub fn encode_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() & 1 == 1 {
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
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_socket_path(label: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "alien-{label}-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

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
        assert_eq!(parse_request("CALL 16 0502 ignored"), None);
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

    #[test]
    fn tagged_firmware_status_survives_the_socket_boundary() {
        let error = parse_response("ERR FWSTATUS e2 keyboard-timeout getter").unwrap_err();
        assert!(matches!(
            error,
            TransportError::FirmwareStatus { status: 0xe2, .. }
        ));
        assert!(parse_response("ERR FWSTATUS nope missing").is_err());
        assert!(parse_response("ERR FWSTATUS e2").is_err());
    }

    #[test]
    fn typed_feature_replies_require_exact_shapes_and_domains() {
        assert!(!parse_bool_feature(vec![0], "test").unwrap());
        assert!(parse_bool_feature(vec![1], "test").unwrap());
        assert!(parse_bool_feature(vec![], "test").is_err());
        assert!(parse_bool_feature(vec![2], "test").is_err());
        assert!(parse_bool_feature(vec![1, 0], "test").is_err());

        assert_eq!(parse_optional_bool_feature(vec![], "test").unwrap(), None);
        assert_eq!(
            parse_optional_bool_feature(vec![1], "test").unwrap(),
            Some(true)
        );
        assert!(parse_optional_bool_feature(vec![0xff], "test").is_err());

        assert_eq!(
            parse_timeout(vec![75, 30]).unwrap(),
            KeyboardTimeoutState {
                brightness: 75,
                seconds: 30,
            }
        );
        assert!(parse_timeout(vec![75, 15]).is_err());
        assert!(parse_timeout(vec![75, 30, 0]).is_err());
    }

    #[test]
    fn gpu_mode_state_roundtrips_and_rejects_bad_domains() {
        let state = GpuModeState {
            graphics: GpuOffsetRange {
                current_mhz: 100,
                min_mhz: -1000,
                max_mhz: 1000,
            },
            memory: GpuOffsetRange {
                current_mhz: 60,
                min_mhz: -2000,
                max_mhz: 6000,
            },
            fan_table: 3,
            gpoc: 2,
        };
        assert_eq!(
            parse_gpu_mode_state(encode_gpu_mode_state(state)).unwrap(),
            state
        );
        assert!(parse_gpu_mode_state(vec![1; 26]).is_err());
        let mut invalid = encode_gpu_mode_state(state);
        invalid[26] = 3;
        assert!(parse_gpu_mode_state(invalid).is_err());
        let mut invalid = state;
        invalid.graphics.current_mhz = 1001;
        assert!(parse_gpu_mode_state(encode_gpu_mode_state(invalid)).is_err());
    }

    #[test]
    fn timeout_discards_stream_so_late_reply_cannot_satisfy_next_request() {
        let path = test_socket_path("late-reply");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();

            // Deliberately finish after the client's short read timeout. This
            // is the exact response that a reused stream could mispair.
            std::thread::sleep(Duration::from_millis(120));
            let _ = stream.write_all(b"OK aa\n");

            let mut second = String::new();
            let second_len = reader.read_line(&mut second).unwrap_or(0);
            (first, second_len, second)
        });

        let client =
            SocketClient::connect_to_with_timeout(&path, Duration::from_millis(20)).unwrap();
        let first_error = client.exchange("FEATURE gpu-mode get").unwrap_err();
        assert!(matches!(
            first_error,
            TransportError::Io(ref error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                )
        ));

        let second_error = client.exchange("FEATURE coolboost get").unwrap_err();
        assert!(matches!(
            second_error,
            TransportError::Io(ref error) if error.kind() == std::io::ErrorKind::BrokenPipe
        ));
        assert!(second_error.to_string().contains("discarded"));

        let (first, second_len, second) = server.join().unwrap();
        assert_eq!(first, "FEATURE gpu-mode get\n");
        assert_eq!(second_len, 0, "discarded client sent a second request");
        assert!(second.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn public_gpu_flag_status_uses_typed_socket_getter_not_raw_call() {
        let path = test_socket_path("typed-gpu-status");
        let listener = UnixListener::bind(&path).unwrap();
        let state = GpuModeState {
            graphics: GpuOffsetRange {
                current_mhz: 0,
                min_mhz: -1000,
                max_mhz: 1000,
            },
            memory: GpuOffsetRange {
                current_mhz: 0,
                min_mhz: -2000,
                max_mhz: 6000,
            },
            fan_table: 1,
            gpoc: 2,
        };
        let response = format!("OK {}\n", encode_hex(&encode_gpu_mode_state(state)));
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            request
        });

        let client = SocketClient::connect_to(&path).unwrap();
        let device = crate::device::Device::with_transport(Box::new(client));
        assert_eq!(
            device.overclock(crate::wmi::OverclockTarget::Gpu).unwrap(),
            2
        );
        assert_eq!(server.join().unwrap(), "FEATURE gpu-mode get\n");
        let _ = std::fs::remove_file(path);
    }
}
