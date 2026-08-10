//! Per-key keyboard lighting, for the Acer models that have the hardware.
//!
//! # Can you colour a single key?
//!
//! **It depends on the machine, and on most Predators the answer is no.**
//! That is a hardware fact, not a software limitation, and it is worth stating
//! plainly because vendor marketing blurs it.
//!
//! Acer ships two entirely different keyboard backlights:
//!
//! * **Four-zone.** The keys are wired into four banks; every key in a bank
//!   shares one set of LEDs. There is no per-key addressing to reach — you can
//!   set four colours, and a "per-key" mode is physically impossible. This is
//!   what the Helios 300 PH315-53 has, and it is driven over the gaming WMI
//!   interface ([`crate::rgb`]).
//! * **Per-key RGB.** A separate USB controller — an **ITE 8291 rev 3** — sits
//!   on the internal bus and addresses every key individually. Found on the
//!   Triton 500 SE, Helios 16/18, and later Helios 300 revisions. This module
//!   drives that.
//!
//! So Alien implements both and detects which one is present, rather than
//! offering a per-key colour picker that silently does nothing on two thirds
//! of the machines it runs on.
//!
//! # The ITE 8291r3 protocol
//!
//! The controller enumerates as USB `048d:6004` (also seen as `048d:ce00`) and
//! takes commands as 8-byte HID feature reports on report id 0xCC. A full
//! per-key frame is then pushed as bulk rows over the interrupt endpoint.
//!
//! The keyboard is addressed as a **6 × 21 grid**: six physical rows, up to 21
//! key columns each, with gaps where a row is shorter. Colour data is sent one
//! row at a time, and — this is the part that trips people up — **not as
//! interleaved RGB**. Each row transfer carries all the red values for that
//! row, then all the green, then all the blue.
//!
//! # Testing status
//!
//! Written against the published protocol and the `ite8291r3-ctl` reference,
//! and **not** exercised on hardware: the reference machine is four-zone, so
//! there is no controller here to talk to. Every function therefore refuses to
//! run unless the device is actually present, and
//! [`crate::capability::Capabilities`] reports per-key as unavailable rather
//! than letting a UI offer it. Reports from per-key owners are wanted.

use std::fs;
use std::path::PathBuf;

/// USB ids the ITE per-key controller is known to enumerate as.
pub const ITE_VENDOR: u16 = 0x048d;
pub const ITE_PRODUCTS: &[u16] = &[0x6004, 0xce00, 0x600a, 0x7000];

/// Keyboard matrix. Six rows; columns vary, 21 is the widest.
pub const ROWS: usize = 6;
pub const COLS: usize = 21;

/// A full per-key colour frame.
#[derive(Clone)]
pub struct KeyFrame {
    /// `[row][col]` RGB. Positions with no key are simply never lit.
    pub keys: [[crate::rgb::Colour; COLS]; ROWS],
}

impl Default for KeyFrame {
    fn default() -> Self {
        KeyFrame { keys: [[crate::rgb::Colour::OFF; COLS]; ROWS] }
    }
}

impl KeyFrame {
    pub fn solid(c: crate::rgb::Colour) -> Self {
        KeyFrame { keys: [[c; COLS]; ROWS] }
    }

    pub fn set(&mut self, row: usize, col: usize, c: crate::rgb::Colour) -> bool {
        if row < ROWS && col < COLS {
            self.keys[row][col] = c;
            true
        } else {
            false
        }
    }

    /// Encode one row as the controller expects it.
    ///
    /// Not interleaved RGB: all reds for the row, then all greens, then all
    /// blues. Getting this wrong produces a keyboard that lights up in
    /// convincing but completely wrong colours, which is a miserable thing to
    /// debug by eye.
    pub fn row_payload(&self, row: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(COLS * 3);
        for ch in 0..3 {
            for col in 0..COLS {
                let c = self.keys[row][col];
                out.push(match ch {
                    0 => c.r,
                    1 => c.g,
                    _ => c.b,
                });
            }
        }
        out
    }
}

/// Where a physical key sits in the 6 × 21 matrix.
///
/// Only the keys people actually want to colour individually are named — WASD,
/// the arrows, the function row, the modifiers. A full ANSI map differs
/// per-model and per-layout, and guessing wrong lights the wrong key, which is
/// worse than not offering the name.
pub fn key_position(name: &str) -> Option<(usize, usize)> {
    let n = name.to_ascii_lowercase();
    let pos = match n.as_str() {
        "esc" => (0, 0),
        "f1" => (0, 2), "f2" => (0, 3), "f3" => (0, 4), "f4" => (0, 5),
        "f5" => (0, 6), "f6" => (0, 7), "f7" => (0, 8), "f8" => (0, 9),
        "f9" => (0, 10), "f10" => (0, 11), "f11" => (0, 12), "f12" => (0, 13),

        "1" => (1, 1), "2" => (1, 2), "3" => (1, 3), "4" => (1, 4), "5" => (1, 5),
        "6" => (1, 6), "7" => (1, 7), "8" => (1, 8), "9" => (1, 9), "0" => (1, 10),

        "tab" => (2, 0),
        "q" => (2, 1), "w" => (2, 2), "e" => (2, 3), "r" => (2, 4), "t" => (2, 5),
        "y" => (2, 6), "u" => (2, 7), "i" => (2, 8), "o" => (2, 9), "p" => (2, 10),

        "caps" | "capslock" => (3, 0),
        "a" => (3, 1), "s" => (3, 2), "d" => (3, 3), "f" => (3, 4), "g" => (3, 5),
        "h" => (3, 6), "j" => (3, 7), "k" => (3, 8), "l" => (3, 9),
        "enter" | "return" => (3, 13),

        "lshift" | "shift" => (4, 0),
        "z" => (4, 1), "x" => (4, 2), "c" => (4, 3), "v" => (4, 4), "b" => (4, 5),
        "n" => (4, 6), "m" => (4, 7),
        "up" => (4, 17),

        "lctrl" | "ctrl" => (5, 0),
        "lalt" | "alt" => (5, 2),
        "space" => (5, 5),
        "left" => (5, 16), "down" => (5, 17), "right" => (5, 18),
        _ => return None,
    };
    Some(pos)
}

/// Names `key_position` understands, for help text and completion.
pub fn known_keys() -> Vec<&'static str> {
    vec![
        "esc", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
        "1", "2", "3", "4", "5", "6", "7", "8", "9", "0",
        "tab", "q", "w", "e", "r", "t", "y", "u", "i", "o", "p",
        "caps", "a", "s", "d", "f", "g", "h", "j", "k", "l", "enter",
        "shift", "z", "x", "c", "v", "b", "n", "m",
        "ctrl", "alt", "space", "up", "down", "left", "right",
    ]
}

/// Is a per-key controller attached to this machine?
///
/// Reads sysfs rather than opening the device: detection must be cheap, must
/// work unprivileged, and must not disturb a controller we may not understand.
pub fn detect() -> Option<PathBuf> {
    let devices = fs::read_dir("/sys/bus/usb/devices").ok()?;
    for entry in devices.flatten() {
        let path = entry.path();
        let vid = fs::read_to_string(path.join("idVendor")).ok();
        let pid = fs::read_to_string(path.join("idProduct")).ok();
        let (Some(vid), Some(pid)) = (vid, pid) else { continue };
        // `continue`, not `?`: /sys/bus/usb/devices holds interface entries as
        // well as devices, and one unparsable id must skip that entry rather
        // than abandon the whole scan and report "no controller".
        let (Ok(vid), Ok(pid)) = (
            u16::from_str_radix(vid.trim(), 16),
            u16::from_str_radix(pid.trim(), 16),
        ) else {
            continue;
        };
        if vid == ITE_VENDOR && ITE_PRODUCTS.contains(&pid) {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::Colour;

    #[test]
    fn row_payload_is_planar_not_interleaved() {
        // The single most important property in this file: get it wrong and
        // the keyboard lights in plausible but wrong colours.
        let mut f = KeyFrame::default();
        f.set(0, 0, Colour::new(0x11, 0x22, 0x33));
        f.set(0, 1, Colour::new(0x44, 0x55, 0x66));
        let p = f.row_payload(0);
        assert_eq!(p.len(), COLS * 3);
        assert_eq!(p[0], 0x11, "reds come first");
        assert_eq!(p[1], 0x44);
        assert_eq!(p[COLS], 0x22, "then all greens");
        assert_eq!(p[COLS + 1], 0x55);
        assert_eq!(p[COLS * 2], 0x33, "then all blues");
        assert_eq!(p[COLS * 2 + 1], 0x66);
    }

    #[test]
    fn every_advertised_key_actually_maps() {
        // A name in the help text that does not resolve would send the user
        // hunting for a typo that is ours.
        for k in known_keys() {
            assert!(key_position(k).is_some(), "advertised key {k} has no position");
        }
    }

    #[test]
    fn positions_are_inside_the_matrix() {
        for k in known_keys() {
            let (r, c) = key_position(k).unwrap();
            assert!(r < ROWS && c < COLS, "{k} maps outside the matrix at {r},{c}");
        }
    }

    #[test]
    fn key_names_are_case_insensitive() {
        assert_eq!(key_position("W"), key_position("w"));
        assert_eq!(key_position("Esc"), key_position("esc"));
    }

    #[test]
    fn unknown_keys_are_rejected_rather_than_guessed() {
        assert_eq!(key_position("hyperspace"), None);
        assert_eq!(key_position(""), None);
    }

    #[test]
    fn solid_fills_every_position() {
        let f = KeyFrame::solid(Colour::new(1, 2, 3));
        assert!(f.keys.iter().flatten().all(|c| *c == Colour::new(1, 2, 3)));
    }
}

// ── Sending a frame ─────────────────────────────────────────────────────────

/// Push a full frame to the controller.
///
/// Writes HID output reports to the controller's `hidraw` node directly rather
/// than pulling in a USB crate: this is a handful of `write()` calls, and the
/// dependency list of a tool that gets vendored into six packaging formats is
/// part of its trust story.
///
/// **Not exercised on hardware.** The reference machine is four-zone, so there
/// is no controller here to send to. The encoding is unit-tested and the
/// device is required to exist, so on a four-zone machine this returns an
/// error rather than pretending — see the module docs.
pub fn send(frame: &KeyFrame) -> std::io::Result<()> {
    let node = hidraw_node().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no ITE 8291 per-key controller present",
        )
    })?;

    use std::io::Write;
    let mut f = fs::OpenOptions::new().write(true).open(node)?;

    // Report id, then command. 0x09 selects "user mode" — without it the
    // controller keeps running its own built-in effect and overwrites whatever
    // we send a moment later.
    f.write_all(&[0xCC, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])?;

    for row in 0..ROWS {
        let mut msg = Vec::with_capacity(2 + COLS * 3);
        // 0x10 = "row data", followed by which row.
        msg.push(0xCC);
        msg.push(0x10 | row as u8);
        msg.extend_from_slice(&frame.row_payload(row));
        f.write_all(&msg)?;
    }

    // Latch: without this the rows sit in the controller's buffer unapplied.
    f.write_all(&[0xCC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])?;
    Ok(())
}

/// The hidraw node belonging to the per-key controller.
fn hidraw_node() -> Option<PathBuf> {
    let usb = detect()?;
    // Walk the USB device's interfaces looking for a hidraw child.
    for entry in fs::read_dir(&usb).ok()?.flatten() {
        let hidraw = entry.path().join("hidraw");
        if let Ok(children) = fs::read_dir(&hidraw) {
            for c in children.flatten() {
                return Some(PathBuf::from("/dev").join(c.file_name()));
            }
        }
        // One level deeper: usbN/…/hidraw lives under the HID device.
        if let Ok(sub) = fs::read_dir(entry.path()) {
            for s in sub.flatten() {
                let hidraw = s.path().join("hidraw");
                if let Ok(children) = fs::read_dir(&hidraw) {
                    for c in children.flatten() {
                        return Some(PathBuf::from("/dev").join(c.file_name()));
                    }
                }
            }
        }
    }
    None
}
