#!/usr/bin/env python3
"""Exercise the real terminal application using only the Python standard library.

Run `cargo build` first, then `python3 scripts/smoke_redis_tui.py` (Unix).
Requires DBRUST_TEST_REDIS_URL for a dedicated Redis-compatible test server.
Uses an isolated configuration directory.
"""
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time
from terminal_screen import Screen

root = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix="dbrust-smoke-") as config:
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    process = subprocess.Popen(
        [str(root / "target/debug/dbrust")],
        stdin=slave, stdout=slave, stderr=slave,
        env={**os.environ, "TERM": "xterm-256color", "XDG_CONFIG_HOME": config, "DBRUST_CLIPBOARD": "internal"},
        start_new_session=True,
    )
    os.close(slave)
    output = bytearray()
    screen = Screen()

    def wait_for(text, timeout=8):
        deadline = time.monotonic() + timeout
        marker = text.encode()
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.1)
            if ready:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    break
                output.extend(chunk)
                screen.feed(chunk)
                # Respond to terminal cursor-position queries if requested.
                if b"\x1b[6n" in chunk:
                    os.write(master, b"\x1b[1;1R")
            if text in screen.text:
                output.clear()
                return
            if process.poll() is not None:
                break
        raise AssertionError(f"Did not see {text!r}; process={process.poll()}; screen:\n{screen.text}")

    try:
        url = os.environ["DBRUST_TEST_REDIS_URL"]
        wait_for("Connection URL")
        os.write(master, b"\x1b[200~" + url.encode() + b"\x1b[201~\r")
        wait_for("All types")
        os.write(master, b"\x01\x0b\x1b[200~SET smoke:json '{\"hello\":42}'\nPEXPIRE smoke:json 60000\x1b[201~")
        os.write(master, b"\x1b[15~")
        wait_for("2 rows")
        os.write(master, b"\x1bOS")  # F4
        wait_for("Redis filters")
        os.write(master, b"\x01\x0b\x1b[200~smoke:*\x1b[201~\t\x1b[C\r")
        wait_for("MATCH smoke:*")
        os.write(master, b"\x1b")  # Focus browser
        time.sleep(0.1)
        os.write(master, b"\r")
        wait_for("JSON / Text")
        os.write(master, b"\x1b[20~")  # F9
        wait_for("Find in loaded values")
        os.write(master, b"\x1b[200~hello\x1b[201~\r")
        wait_for("loaded preview only")
        os.write(master, b"\r")
        wait_for("Full value")
        os.write(master, b"\x03")  # Ctrl+C copies full document
        wait_for("Copied")
        os.write(master, b"\x1b")
        time.sleep(0.1)
        os.write(master, b"\t\t\x01\x16")  # Editor, select all, Ctrl+V
        wait_for("Pasted from clipboard")
        os.write(master, b"\x1b[17~")  # F6 namespace tree
        wait_for("Namespaces")
        os.write(master, b"\x11")
        process.wait(timeout=5)
        assert process.returncode == 0
        print("PASS: Redis connection, commands, pattern/type filters, JSON inspector, value search, full value, Ctrl+C/Ctrl+V, tree, clean exit")
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        os.close(master)
