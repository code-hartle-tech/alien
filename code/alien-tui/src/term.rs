//! Minimal raw-mode terminal handling.
//!
//! Enough to run a full-screen TUI: raw mode, alternate screen, hidden cursor,
//! 24-bit colour, and a window size. No dependency, because the alternative is
//! pulling a terminal library plus its platform shims into a crate that gets
//! vendored into six packaging formats.
//!
//! # The part that actually matters
//!
//! Restoring the terminal. A TUI that exits without putting the terminal back
//! leaves the user with no echo and no cursor in a shell that looks broken —
//! and they cannot see what they type to fix it. So restoration happens in
//! `Drop`, which runs on normal exit *and* on unwind, and the original
//! `termios` is captured before anything is changed.

use std::io::{Stdout, Write};
use std::mem::MaybeUninit;

pub const CLEAR_HOME: &str = "\x1b[2J\x1b[H";

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub fn fg(c: Rgb, s: &str) -> String {
    format!("\x1b[38;2;{};{};{}m{s}\x1b[0m", c.0, c.1, c.2)
}

pub fn bold(s: &str) -> String {
    format!("\x1b[1m{s}\x1b[0m")
}

// ── libc surface, declared rather than depended on ──────────────────────────

const STDIN_FD: i32 = 0;
const TCSANOW: i32 = 0;
const TIOCGWINSZ: u64 = 0x5413;

// termios is 60 bytes on Linux/glibc (c_iflag..c_ospeed). Treating it as an
// opaque byte blob keeps us honest: we never interpret the fields we do not
// need, we only clear specific flag bits at known offsets.
#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

#[repr(C)]
struct WinSize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

extern "C" {
    fn tcgetattr(fd: i32, t: *mut Termios) -> i32;
    fn tcsetattr(fd: i32, opt: i32, t: *const Termios) -> i32;
    fn ioctl(fd: i32, req: u64, ...) -> i32;
    fn isatty(fd: i32) -> i32;
}

// Flag bits we touch, from <termios.h>.
const OPOST: u32 = 0o000001;
const ICANON: u32 = 0o000002;
const ECHO: u32 = 0o000010;
const ISIG: u32 = 0o000001;
const IXON: u32 = 0o002000;
const ICRNL: u32 = 0o000400;
const VMIN: usize = 6;
const VTIME: usize = 5;

pub struct Terminal {
    pub out: Stdout,
    pub width: u16,
    original: Termios,
}

impl Terminal {
    pub fn enter() -> std::io::Result<Terminal> {
        // SAFETY: isatty on a fixed fd, no memory involved.
        if unsafe { isatty(STDIN_FD) } != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "stdin is not a terminal — alien-tui needs one (try `alien status` instead)",
            ));
        }

        let mut original = MaybeUninit::<Termios>::uninit();
        // SAFETY: tcgetattr fills the struct; we only read it after success.
        if unsafe { tcgetattr(STDIN_FD, original.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: tcgetattr returned 0, so the value is initialised.
        let original = unsafe { original.assume_init() };

        let mut raw = original;
        raw.c_lflag &= !(ICANON | ECHO | ISIG);
        raw.c_iflag &= !(IXON | ICRNL);
        // Clear OPOST too, which the first version forgot. With output
        // post-processing on, ONLCR rewrites every "\n" as "\r\n" — and this
        // UI already emits "\r\n", so every line went out as "\r\r\n". Harmless
        // on a real terminal, but it is one more column on the wire than the
        // layout thinks it is writing, which is exactly the kind of
        // off-by-one that makes a width check disagree with the code.
        raw.c_oflag &= !OPOST;
        // VMIN 0 + VTIME 1: read() returns after 100 ms with whatever arrived,
        // including nothing. That is what lets one thread poll the keyboard and
        // still redraw on a timer without a second thread or a poll() dance.
        raw.c_cc[VMIN] = 0;
        raw.c_cc[VTIME] = 1;

        // SAFETY: raw is a fully initialised Termios derived from a valid one.
        if unsafe { tcsetattr(STDIN_FD, TCSANOW, &raw) } != 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut out = std::io::stdout();
        // Alternate screen, then hide the cursor. Leaving the main screen
        // untouched means quitting restores the user's scrollback intact.
        write!(out, "\x1b[?1049h\x1b[?25l")?;
        out.flush()?;

        Ok(Terminal { out, width: window_width(), original })
    }

    /// Re-read the window size. Cheap (one ioctl) and called every frame,
    /// because a tiling window manager resizes this window after it opens —
    /// the size captured at startup is usually not the size it ends up.
    pub fn refresh_size(&mut self) {
        self.width = window_width();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Show cursor, leave the alternate screen, restore the original modes.
        // Order matters: the escape codes must go out before stdin flips back,
        // or a shell prompt can land on the alternate screen.
        let _ = write!(self.out, "\x1b[?25h\x1b[?1049l");
        let _ = self.out.flush();
        // SAFETY: restoring the exact struct captured in enter().
        unsafe {
            tcsetattr(STDIN_FD, TCSANOW, &self.original);
        }
    }
}

fn window_width() -> u16 {
    let mut ws = WinSize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ writes a WinSize through the pointer we supply.
    let rc = unsafe { ioctl(STDIN_FD, TIOCGWINSZ, &mut ws as *mut WinSize) };
    if rc == 0 && ws.ws_col > 0 {
        ws.ws_col
    } else {
        // A sane default beats a zero-width layout when the ioctl is
        // unavailable, as it is under some CI and container ptys.
        80
    }
}
