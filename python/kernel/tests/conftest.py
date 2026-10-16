from __future__ import annotations

import shutil
import sys
import threading
import time
from pathlib import Path

import pytest

KERNEL_DIR = Path(__file__).resolve().parent.parent
FIXTURE = Path(__file__).resolve().parent / "fixtures" / "sample_repo"
if str(KERNEL_DIR) not in sys.path:
    sys.path.insert(0, str(KERNEL_DIR))

from analyst_kernel.kernel import Kernel  # noqa: E402

SESSION_PYTHON = sys.executable


class Notes:
    """Thread-safe notification sink."""

    def __init__(self) -> None:
        self.items: list[tuple[str, dict]] = []
        self._cv = threading.Condition()

    def __call__(self, method: str, params: dict) -> None:
        with self._cv:
            self.items.append((method, params))
            self._cv.notify_all()

    def of(self, method: str, run_id: str | None = None) -> list[dict]:
        with self._cv:
            return [p for m, p in self.items if m == method and (run_id is None or p.get("run_id") == run_id)]

    def wait_for(self, method: str, run_id: str | None = None, timeout: float = 90.0, count: int = 1) -> list[dict]:
        deadline = time.monotonic() + timeout
        with self._cv:
            while True:
                found = [p for m, p in self.items if m == method and (run_id is None or p.get("run_id") == run_id)]
                if len(found) >= count:
                    return found
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AssertionError(f"timed out waiting for {method} (have {len(found)}): {self.items[-5:]}")
                self._cv.wait(min(remaining, 0.5))


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    dst = tmp_path / "repo"
    shutil.copytree(FIXTURE, dst)
    return dst


@pytest.fixture
def notes() -> Notes:
    return Notes()


@pytest.fixture
def kernel(repo: Path, tmp_path: Path, notes: Notes):
    k = Kernel(SESSION_PYTHON, repo, tmp_path / "runs", sandbox="limits", notify=notes)
    yield k
    k.close()


def wait_until(pred, timeout: float = 30.0, interval: float = 0.1) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if pred():
            return
        time.sleep(interval)
    raise AssertionError("condition not met in time")
