#!/usr/bin/env python3
"""Check that alien-tui's layout fits the terminal it is given.

Why this exists: the first version of the TUI looked fine in an 80-column
terminal and fell apart in a tiled window. The header wrapped mid-word, each
sparkline ran past the right edge and pushed the next row down, and the key
hints broke as "+/- bri / ghtness". None of that is visible from unit tests,
and eyeballing it in a window only tests the one width that window happens to
be.

So: run the real binary in a pty of a chosen size, take the last frame it
drew, strip the SGR escapes, and assert no line is wider than the terminal.

    ./tui-layout-check.py                # a spread of widths
    ./tui-layout-check.py 60 --show      # one width, printed

Needs a working daemon or root, because the TUI renders real telemetry.
"""

import argparse
import fcntl
import os
import pty
import re
import select
import struct
import sys
import termios
import time

BINARY = os.environ.get("ALIEN_TUI", "/usr/bin/alien-tui")
SGR = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
CLEAR = "\x1b[2J\x1b[H"

# Widths worth checking: the minimum the layout clamps to, a tiled half-screen,
# a conventional terminal, and something very wide.
WIDTHS = [38, 60, 80, 100, 160]


def capture(width: int, rows: int = 40, seconds: float = 3.0) -> str:
    """Run the TUI in a pty and return everything it wrote."""
    pid, fd = pty.fork()
    if pid == 0:
        os.execv(BINARY, [os.path.basename(BINARY)])

    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, width, 0, 0))
    out = b""
    deadline = time.time() + seconds
    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.2)
        if ready:
            try:
                out += os.read(fd, 65536)
            except OSError:
                break

    # Ask it to quit rather than killing it, so the terminal-restore path in
    # Drop actually runs — that path is the one that matters most if it breaks.
    try:
        os.write(fd, b"q")
        time.sleep(0.4)
        os.read(fd, 65536)
    except OSError:
        pass
    try:
        os.waitpid(pid, os.WNOHANG)
    except ChildProcessError:
        pass
    return out.decode("utf8", "replace")


def last_frame(raw: str) -> str:
    frames = raw.split(CLEAR)
    return frames[-1] if frames else raw


def check(width: int, show: bool = False) -> int:
    frame = last_frame(capture(width))
    plain = SGR.sub("", frame)
    overflow = []
    # Strip stray CRs before measuring: a pty with OPOST set rewrites "\n" as
    # "\r\n", so a UI that already emits "\r\n" arrives as "\r\r\n" and every
    # line looks one column too wide. That cost a real debugging detour — the
    # app was correct and the harness was lying.
    for i, line in enumerate(plain.split("\n")):
        line = line.strip("\r")
        if len(line) > width:
            overflow.append((i, len(line), line))

    if show:
        print("·" * width + "| <- width")
        for line in (l.strip("\r") for l in plain.split("\n")):
            over = f"   <<< OVERFLOW by {len(line) - width}" if len(line) > width else ""
            print(line + over)

    if not plain.strip():
        print(f"  width {width:>4}: FAIL — the TUI drew nothing (is the daemon running?)")
        return 1
    if overflow:
        print(f"  width {width:>4}: FAIL — {len(overflow)} line(s) overflow")
        for i, n, line in overflow[:3]:
            print(f"      line {i}: {n} cols: {line[:width]}…")
        return 1
    print(f"  width {width:>4}: ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("widths", nargs="*", type=int, help="widths to check")
    ap.add_argument("--show", action="store_true", help="print the frame")
    args = ap.parse_args()

    if not os.path.exists(BINARY):
        print(f"not found: {BINARY} (set ALIEN_TUI)", file=sys.stderr)
        return 2

    failures = sum(check(w, args.show) for w in (args.widths or WIDTHS))
    print("\nlayout:", "PASS" if failures == 0 else f"FAIL ({failures})")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
