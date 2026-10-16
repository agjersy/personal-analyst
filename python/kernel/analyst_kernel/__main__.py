"""``python -m analyst_kernel --python P --root R --runs-dir D [--out-dir out] [--sandbox auto]``."""

from __future__ import annotations

import argparse
import logging
import os
import sys
from pathlib import Path

from .errors import RpcError
from .kernel import Kernel
from .rpc import Server
from .sandbox import KINDS


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="analyst_kernel", description="Personal Analyst per-target kernel (JSON-RPC over stdio)")
    parser.add_argument("--python", required=True, help="session interpreter used to run scripts and snippets")
    parser.add_argument("--root", required=True, help="repository root (or mirror) on this target")
    parser.add_argument("--runs-dir", required=True, help="directory holding one subdirectory per run")
    parser.add_argument("--out-dir", default=None, help="output directory name under root (default: project config, else 'out')")
    parser.add_argument("--sandbox", default="auto", choices=KINDS, help="sandbox kind (default: auto)")
    parser.add_argument("--log-level", default=os.environ.get("ANALYST_KERNEL_LOG", "INFO"), help="stderr log level")
    return parser


def main(argv: list[str] | None = None) -> int:
    ns = build_parser().parse_args(argv)
    logging.basicConfig(level=ns.log_level.upper(), stream=sys.stderr, format="kernel %(levelname)s %(name)s: %(message)s")
    root = Path(ns.root).expanduser()
    if not root.is_dir():
        logging.error("root %s is not a directory", root)
        return 2
    try:
        kernel = Kernel(
            python=ns.python,
            root=root,
            runs_dir=Path(ns.runs_dir).expanduser(),
            out_dir=ns.out_dir,
            sandbox=ns.sandbox,
        )
    except RpcError as exc:
        logging.error("%s: %s", exc.message, exc.detail)
        return 3
    server = Server(kernel.dispatcher)
    kernel.set_notify(server.notify)
    try:
        server.serve()
    finally:
        kernel.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
