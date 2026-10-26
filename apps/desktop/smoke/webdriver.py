#!/usr/bin/env python3
"""Minimal W3C WebDriver client (stdlib only) for driving the Tauri app through tauri-driver.

Usage: smoke/webdriver.py --app <binary> [--port 4444] [--screenshot out.png] <steps...>
Used by smoke/run.sh; also importable.
"""

from __future__ import annotations

import base64
import json
import sys
import time
import urllib.error
import urllib.request


class Driver:
    def __init__(self, base: str):
        self.base = base.rstrip("/")
        self.session: str | None = None

    def _req(self, method: str, path: str, body: dict | None = None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(self.base + path, data=data, method=method, headers={"content-type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.loads(r.read() or b"{}").get("value")
        except urllib.error.HTTPError as e:
            raise RuntimeError(f"{method} {path}: {e.code} {e.read().decode()[:400]}") from None

    def start(self, app: str, args: list[str] | None = None) -> None:
        caps = {"capabilities": {"alwaysMatch": {"tauri:options": {"application": app, "args": args or []}}}}
        v = self._req("POST", "/session", caps)
        self.session = v["sessionId"]

    def quit(self) -> None:
        if self.session:
            self._req("DELETE", f"/session/{self.session}")
            self.session = None

    def _s(self, path: str, method="GET", body=None):
        return self._req(method, f"/session/{self.session}{path}", body)

    def title(self) -> str:
        return self._s("/title")

    def find(self, css: str, timeout: float = 20.0):
        deadline = time.time() + timeout
        last = None
        while time.time() < deadline:
            try:
                v = self._s("/element", "POST", {"using": "css selector", "value": css})
                return next(iter(v.values()))
            except RuntimeError as e:
                last = e
                time.sleep(0.25)
        raise TimeoutError(f"element not found: {css}: {last}")

    def find_all(self, css: str) -> list[str]:
        v = self._s("/elements", "POST", {"using": "css selector", "value": css})
        return [next(iter(e.values())) for e in v]

    def click(self, css: str, timeout: float = 20.0) -> None:
        el = self.find(css, timeout)
        self._s(f"/element/{el}/click", "POST", {})

    def type(self, css: str, text: str) -> None:
        el = self.find(css)
        self._s(f"/element/{el}/clear", "POST", {})
        self._s(f"/element/{el}/value", "POST", {"text": text})

    def text(self, css: str, timeout: float = 20.0) -> str:
        el = self.find(css, timeout)
        return self._s(f"/element/{el}/text")

    def wait_text(self, css: str, needle: str, timeout: float = 60.0) -> str:
        deadline = time.time() + timeout
        last = ""
        while time.time() < deadline:
            try:
                last = self.text(css, 2)
                if needle in last:
                    return last
            except (TimeoutError, RuntimeError):
                pass
            time.sleep(0.5)
        raise TimeoutError(f"{css!r} never contained {needle!r}; last: {last[:200]!r}")

    def wait_count(self, css: str, n: int, timeout: float = 60.0) -> int:
        deadline = time.time() + timeout
        c = 0
        while time.time() < deadline:
            c = len(self.find_all(css))
            if c >= n:
                return c
            time.sleep(0.5)
        raise TimeoutError(f"expected at least {n} of {css!r}, saw {c}")

    def execute(self, script: str, *args):
        return self._s("/execute/sync", "POST", {"script": script, "args": list(args)})

    def screenshot(self, path: str) -> None:
        png = self._s("/screenshot")
        with open(path, "wb") as f:
            f.write(base64.b64decode(png))


if __name__ == "__main__":
    print("import this module from a smoke script", file=sys.stderr)
    sys.exit(2)
