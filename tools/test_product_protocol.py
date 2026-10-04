#!/usr/bin/env python3
"""Contract tests for the asynchronous rd-classic/1 product protocol.

Usage:
  python3 tools/test_product_protocol.py [target/release/classic_engine]
"""

from __future__ import annotations

import selectors
import subprocess
import sys
import time
from pathlib import Path

ENGINE = (
    Path(sys.argv[1]) if len(sys.argv) > 1 else Path("target/release/classic_engine")
)


class Client:
    def __init__(self, path: Path):
        self.p = subprocess.Popen(
            [
                str(path),
                "--eval",
                "config/eval_default.txt",
                "--search",
                "config/search_default.txt",
                "--tt",
                "16",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=False,
            bufsize=0,
        )
        assert self.p.stdout and self.p.stdin
        self.sel = selectors.DefaultSelector()
        self.sel.register(self.p.stdout, selectors.EVENT_READ)
        self.buf = bytearray()

    def send(self, line: str) -> None:
        assert self.p.stdin
        self.p.stdin.write((line + "\n").encode())
        self.p.stdin.flush()

    def read(self, timeout: float = 5.0) -> str:
        assert self.p.stdout
        deadline = time.monotonic() + timeout
        while True:
            pos = self.buf.find(b"\n")
            if pos >= 0:
                raw = bytes(self.buf[:pos])
                del self.buf[: pos + 1]
                return raw.rstrip(b"\r").decode("utf-8", "replace")
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not self.sel.select(remaining):
                raise TimeoutError("protocol read timeout")
            data = self.p.stdout.read(4096)
            if not data:
                err = (
                    self.p.stderr.read().decode("utf-8", "replace")
                    if self.p.stderr
                    else ""
                )
                raise RuntimeError(f"engine exited rc={self.p.poll()} stderr={err}")
            self.buf.extend(data)

    def expect_error(self, command: str, prefix: str = "error ") -> str:
        self.send(command)
        line = self.read()
        assert line.startswith(prefix), (command, line)
        return line

    def close(self) -> None:
        if self.p.poll() is None:
            try:
                self.send("quit")
                self.read(2.0)
            except Exception:
                self.p.kill()
        try:
            self.p.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.p.kill()
            self.p.wait()


def is_search_result(line: str) -> bool:
    return line.startswith(
        ("bestmove ", "claim ", "claim_move ", "status ", "adjudication ")
    )


def main() -> int:
    if not ENGINE.exists():
        raise SystemExit(f"missing engine binary: {ENGINE}")
    c = Client(ENGINE)
    try:
        greeting = c.read()
        assert "protocol rd-classic/1" in greeting, greeting

        c.send("protocol")
        assert c.read() == "protocol rd-classic 1"

        # The versioned parser rejects malformed optional fields and leftovers;
        # it never silently substitutes defaults for explicitly bad values.
        for command in (
            "protocol extra",
            "version extra",
            "ping a b",
            "new extra",
            "position not-hex 0 0 white",
            "position 0 0 0 white extra",
            "go nodes 1000 abc",
            "go nodes 1000 0",
            "go nodes 1000 4 extra",
            "go time 10 nope",
            "go depth 0",
            "go depth 1 extra",
            "play 1 2 3,3",
            "play 1 2 - extra",
        ):
            c.expect_error(command)

        c.send("ping after-errors")
        assert c.read() == "pong after-errors"

        c.send("legal")
        legal = c.read()
        parts = legal.split()
        assert parts[0] == "legal" and int(parts[1]) == 7, legal
        from_s, to_s, captures = parts[2].split(":", 2)
        c.send(f"play {from_s} {to_s} {captures}")
        assert c.read() == "ok"

        # CheckedGame must reject mutation after a terminal game even if the
        # supplied move is syntactically valid.
        c.send("position 0 1 0 white")
        assert c.read() == "ok"
        c.send("play 1 2 -")
        assert c.read().startswith("error illegal_move game is already finished")

        c.send("new")
        assert c.read() == "ok"

        # Search remains responsive, accepts repeated stop, and the stop token
        # is scoped to this search only.
        c.send("go time 10000 64")
        c.send("ping during-search")
        c.send("stop")
        c.send("stop")
        seen_pong = False
        stop_replies = 0
        result = None
        for _ in range(12):
            line = c.read(5.0)
            if line == "pong during-search":
                seen_pong = True
            elif line in {"ok stop", "ok idle"}:
                stop_replies += 1
            elif is_search_result(line):
                result = line
            if seen_pong and stop_replies == 2 and result is not None:
                break
        assert seen_pong, "no ping response during active search"
        assert stop_replies == 2, "both stop commands must be acknowledged"
        assert result is not None, "search result did not arrive after stop"
        if result.startswith("bestmove "):
            assert " stop external_stop" in result, result

        # A late/repeated stop from the previous search must not poison this one.
        c.send("go depth 1")
        next_result = c.read(5.0)
        assert is_search_result(next_result), next_result
        if next_result.startswith("bestmove "):
            assert " stop external_stop" not in next_result, next_result

        # An idle stop is explicitly a no-op and also must not affect next go.
        c.send("stop")
        assert c.read() == "ok idle"
        c.send("go depth 1")
        idle_next = c.read(5.0)
        assert is_search_result(idle_next), idle_next
        if idle_next.startswith("bestmove "):
            assert " stop external_stop" not in idle_next, idle_next

        c.send("quit")
        assert c.read(2.0) == "bye"
        c.p.wait(timeout=2)
        print("product protocol: OK")
        return 0
    finally:
        if c.p.poll() is None:
            c.close()


if __name__ == "__main__":
    raise SystemExit(main())
