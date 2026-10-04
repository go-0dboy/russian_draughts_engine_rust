"""Shared progress and concise CLI failures; protocol stdout stays machine-readable."""

import os
import sys
import threading
import time
import subprocess


class Progress:
    """Heartbeat also runs while SQLite or exporter blocks the main thread."""

    def __init__(self, interval=5):
        self.interval = interval
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.started = time.monotonic()
        self.label = "Подготовка"
        self.n = 0
        self.total = None
        self.stage_started = self.started

    def stage(self, label, total=None):
        with self.lock:
            self.label = label
            self.n = 0
            self.total = total
            self.stage_started = time.monotonic()
        self.show()

    def count(self, n):
        with self.lock:
            self.n = n

    def show(self):
        with self.lock:
            elapsed = time.monotonic() - self.started
            duration = max(time.monotonic() - self.stage_started, 0.001)
            text = f"[{elapsed:.0f} с] {self.label}"
            if self.total is not None:
                fraction = self.n / self.total if self.total else 1
                text += f": {self.n}/{self.total} ({100*fraction:.1f}%)"
                if self.n and self.n < self.total:
                    text += f", остаток ~{duration*(self.total-self.n)/self.n:.0f} с"
            elif self.n:
                text += f": {self.n}"
            if self.n:
                text += f", {self.n/duration:.1f}/с"
            print(text, file=sys.stderr, flush=True)

    def loop(self):
        while not self.stop.wait(self.interval):
            self.show()

    def __enter__(self):
        self.thread = threading.Thread(target=self.loop, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *args):
        self.stop.set()
        self.thread.join()
        self.show()


def run_cli(main):
    try:
        result = main()
        raise SystemExit(result or 0)
    except KeyboardInterrupt:
        print(
            "\nОстановлено пользователем. Завершённые результаты сохранены; см. правила resume инструмента.",
            file=sys.stderr,
        )
        raise SystemExit(130) from None
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        if os.environ.get("RD_DEBUG") == "1":
            raise
        print(f"Ошибка: {error}\nПодробная трассировка: RD_DEBUG=1", file=sys.stderr)
        raise SystemExit(2) from None
