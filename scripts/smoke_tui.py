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
        wait_for("Connection URL")
        os.write(master, b"\x1b[200~sqlite::memory:\x1b[201~\r")
        wait_for("Connected.")
        os.write(master, b"\x01\x0b\x1b[200~CREATE TABLE demo (id INTEGER, name TEXT);\nINSERT INTO demo VALUES (42, 'hello'), (43, 'world');\nSELECT * FROM demo;\x1b[201~")
        os.write(master, b"\x1b[15~")  # F5
        wait_for("2 rows")
        os.write(master, b"\x1b[Z\x01\x1b[200~SELECT 'SELECT 99 AS copied_answer;' AS query_text;\x1b[201~\x1b[15~")
        wait_for("Cell [1, 1]: SELECT 99 AS copied_answer;")
        os.write(master, b"\x03")  # Ctrl+C copies the selected SQL result cell
        wait_for("Copied")
        os.write(master, b"\x1b[Z\x01\x16")  # SQL editor, select all, Ctrl+V
        wait_for("Pasted from clipboard")
        os.write(master, b"\x1b[15~")
        wait_for("Cell [1, 1]: 99")
        os.write(master, b"\x1b[13~")  # F3 refresh
        wait_for("Table list refreshed.")
        os.write(master, b"\x1b[17~")  # F6: Columns
        wait_for("position")
        os.write(master, b"\x1b[<68;80;24M")  # Shift+wheel down over results
        os.write(master, b"\x1b5")  # Alt+5: DDL
        wait_for("definition")
        os.write(master, b"\x1b[C\x1b[C\r")  # Full DDL cell
        wait_for("Full value")
        os.write(master, b"\x1b")  # Close full-value popup
        history = Path(config, "dbrust", "connections.json")
        assert history.is_file()
        assert "sqlite::memory:" in history.read_text()
        os.write(master, b"\x11")  # Ctrl+Q
        process.wait(timeout=5)
        assert process.returncode == 0
        print("PASS: startup, bracketed paste, SQLite connection, multiline query, SQL Ctrl+C/Ctrl+V and execution, F5, F3, schema tabs, Shift+wheel, full-value popup, history, clean exit")
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        os.close(master)
