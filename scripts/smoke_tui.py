#!/usr/bin/env python3
"""Exercise the real terminal application using only the Python standard library.

Run `cargo build` first, then `python3 scripts/smoke_tui.py` (Unix).
Uses an isolated config directory and an in-memory SQLite database.
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

root = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix="dbrust-smoke-") as config:
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    process = subprocess.Popen(
        [str(root / "target/debug/dbrust")],
        stdin=slave, stdout=slave, stderr=slave,
        env={**os.environ, "TERM": "xterm-256color", "XDG_CONFIG_HOME": config},
        start_new_session=True,
    )
    os.close(slave)
    output = bytearray()

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
                # Respond to terminal cursor-position queries if requested.
                if b"\x1b[6n" in chunk:
                    os.write(master, b"\x1b[1;1R")
            if marker in output:
                output.clear()
                return
            if process.poll() is not None:
                break
        raise AssertionError(f"Did not see {text!r}; process={process.poll()}; output={bytes(output)[-3000:]!r}")

    try:
        wait_for("Connection URL")
        os.write(master, b"\x1b[200~sqlite::memory:\x1b[201~\r")
        wait_for("Connected.")
        os.write(master, b"\x01\x0b\x1b[200~CREATE TABLE demo (id INTEGER, name TEXT);\nINSERT INTO demo VALUES (42, 'hello'), (43, 'world');\nSELECT * FROM demo;\x1b[201~")
        os.write(master, b"\x1b[15~")  # F5
        wait_for("2 rows")
        os.write(master, b"\x1b[13~")  # F3 refresh
        wait_for("Table list refreshed.")
        history = Path(config, "dbrust", "connections.json")
        assert history.is_file()
        assert "sqlite::memory:" in history.read_text()
        os.write(master, b"\x11")  # Ctrl+Q
        process.wait(timeout=5)
        assert process.returncode == 0
        print("PASS: startup, bracketed paste, SQLite connection, multiline query, F5, F3, history, clean exit")
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        os.close(master)
