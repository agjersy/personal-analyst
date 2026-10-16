#!/usr/bin/env python3
"""A scripted OpenAI-compatible chat-completions server for exercising the agent loop without a model.

It is model-agnostic in the only sense that matters here: it speaks the wire format from
PROTOCOL.md section 4 and nothing else. Responses are chosen from the conversation so far
(which tools were already called), so each eval task plays out deterministically.

Usage:  python3 evals/fake_model_server.py [--port 8765]
Config: [[providers]] name="fake" kind="openai-compatible" base_url="http://127.0.0.1:8765/v1"
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SMOOTH_SCRIPT = '''"""Smooth a time series with a centered moving average and plot it."""

from __future__ import annotations

import argparse

import matplotlib.pyplot as plt
import numpy as np

from analysis.signal import load_series, moving_average


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Smooth a signal with a centered moving average.")
    parser.add_argument("--data", default="data/signal.csv", help="CSV file with columns t and v.")
    parser.add_argument("--window", type=int, default=11, help="Odd moving-average window length in samples.")
    return parser


def main(args: argparse.Namespace) -> None:
    t, v = load_series(args.data)
    smoothed = moving_average(v, args.window)
    rms = float(np.sqrt(np.mean((v - smoothed) ** 2)))
    print(f"window={args.window} rms_difference={rms:.4f}")
    plt.plot(t, v, label="raw", alpha=0.5)
    plt.plot(t, smoothed, label=f"moving average ({args.window})")
    plt.xlabel("t")
    plt.ylabel("v")
    plt.title("smoothed signal")
    plt.legend()
    plt.show()


if __name__ == "__main__":
    main(build_parser().parse_args())
'''


def called_tools(messages: list[dict]) -> list[tuple[str, dict]]:
    out = []
    for m in messages:
        if m.get("role") == "assistant":
            for tc in m.get("tool_calls") or []:
                fn = tc.get("function", {})
                try:
                    args = json.loads(fn.get("arguments") or "{}")
                except json.JSONDecodeError:
                    args = {}
                out.append((fn.get("name", ""), args))
    return out


def last_user_text(messages: list[dict]) -> str:
    for m in reversed(messages):
        if m.get("role") == "user":
            c = m.get("content")
            if isinstance(c, str):
                return c
            if isinstance(c, list):
                return " ".join(p.get("text", "") for p in c if isinstance(p, dict))
    return ""


def first_user_text(messages: list[dict]) -> str:
    for m in messages:
        if m.get("role") == "user":
            c = m.get("content")
            return c if isinstance(c, str) else json.dumps(c)
    return ""


def decide(messages: list[dict]) -> tuple[str | None, tuple[str, dict] | None]:
    """Return (text, tool_call) for the next assistant turn."""
    text = last_user_text(messages)
    m = re.search(r'echo_probe tool with token "([^"]+)"', text)
    if m:
        return None, ("echo_probe", {"token": m.group(1)})
    calls = called_tools(messages)
    task = first_user_text(messages)
    if "decay_sim.py" in task:  # parameter-sweep (run mode)
        taus = ["0.5", "1.0", "2.0"]
        done = [a for n, a in calls if n == "run_script"]
        if len(done) < len(taus):
            return None, ("run_script", {"script": "scripts/decay_sim.py", "args": ["--tau", taus[len(done)]]})
        rows = "\n".join(f"| {t} | {float(t) * 0.6931:.4f} |" for t in taus)
        return f"Half-life for each tau (tau * ln 2):\n\n| tau | half-life |\n|---|---|\n{rows}\n", None
    if "smooth_signal.py" in task:  # reuse-library-function (build mode)
        names = [n for n, _ in calls]
        if "declare_scope" not in names:
            return None, ("declare_scope", {"files": ["scripts/smooth_signal.py"],
                                            "plan": "Add a thin CLI that reuses analysis.signal.load_series and moving_average."})
        if "fs_propose_create" not in names:
            return None, ("fs_propose_create", {"path": "scripts/smooth_signal.py", "content": SMOOTH_SCRIPT})
        if "run_script" not in names:
            return None, ("run_script", {"script": "scripts/smooth_signal.py", "args": ["--window", "21"]})
        return "Created scripts/smooth_signal.py using the library's load_series and moving_average, and ran it with --window 21.", None
    if "summary" in text.lower() and "compact" in text.lower():
        return "Summary: earlier turns completed the requested work.", None
    return "I have nothing further to do for this request.", None


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):  # quieter
        sys.stderr.write("fake-model: " + fmt % args + "\n")

    def _json(self, code: int, body: dict) -> None:
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.rstrip("/").endswith("/models"):
            return self._json(200, {"object": "list", "data": [{"id": "scripted", "object": "model", "context_length": 65536}]})
        return self._json(404, {"error": "not found"})

    def do_POST(self):
        if not self.path.rstrip("/").endswith("/chat/completions"):
            return self._json(404, {"error": "not found"})
        n = int(self.headers.get("content-length", "0"))
        req = json.loads(self.rfile.read(n) or b"{}")
        text, call = decide(req.get("messages", []))
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.end_headers()

        def chunk(delta: dict, finish: str | None = None) -> None:
            body = {"id": "chatcmpl-fake", "object": "chat.completion.chunk", "model": "scripted",
                    "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            self.wfile.write(f"data: {json.dumps(body)}\n\n".encode())
            self.wfile.flush()

        chunk({"role": "assistant", "content": ""})
        if call is not None:
            name, args = call
            chunk({"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": name, "arguments": ""}}]})
            payload = json.dumps(args)
            for i in range(0, len(payload), 48):
                chunk({"tool_calls": [{"index": 0, "function": {"arguments": payload[i:i + 48]}}]})
            chunk({}, "tool_calls")
        else:
            for i in range(0, len(text or ""), 32):
                chunk({"content": text[i:i + 32]})
            chunk({}, "stop")
        usage = {"id": "chatcmpl-fake", "object": "chat.completion.chunk", "choices": [],
                 "usage": {"prompt_tokens": 100, "completion_tokens": 20}}
        self.wfile.write(f"data: {json.dumps(usage)}\n\ndata: [DONE]\n\n".encode())
        self.wfile.flush()


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--port", type=int, default=8765, help="TCP port to listen on.")
    a = p.parse_args()
    srv = ThreadingHTTPServer(("127.0.0.1", a.port), Handler)
    sys.stderr.write(f"fake-model: listening on http://127.0.0.1:{a.port}/v1\n")
    srv.serve_forever()


if __name__ == "__main__":
    main()
