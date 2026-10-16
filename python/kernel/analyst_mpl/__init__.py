"""Stdlib-only Matplotlib capture backend injected into user script processes.

See docs/PROTOCOL.md section 1.6. Selected with ``MPLBACKEND=module://analyst_mpl``.
It subclasses the Agg backend: ``show()`` saves every open figure as PNG and SVG with a
JSON sidecar and closes it; ``savefig()`` works as usual and also writes the sidecar.
Imports only the standard library and Matplotlib.
"""

from __future__ import annotations

import json
import os
import re
import sys
from datetime import UTC, datetime
from pathlib import Path

from matplotlib import _pylab_helpers
from matplotlib.backend_bases import FigureManagerBase
from matplotlib.backends.backend_agg import FigureCanvasAgg

__all__ = ["FigureCanvas", "FigureManager", "figures_dir", "new_figure_manager", "show", "write_sidecar"]

_FIGURE_EXTS = (".png", ".svg", ".pdf")


def figures_dir() -> Path:
    """``$ANALYST_OUT_DIR/figures`` or ``./out/figures`` when the variable is unset."""
    base = os.environ.get("ANALYST_OUT_DIR")
    root = Path(base) if base else Path.cwd() / "out"
    return root / "figures"


def slugify(text: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return slug[:80]


def _suptitle(fig) -> str | None:
    getter = getattr(fig, "get_suptitle", None)
    text = getter() if callable(getter) else (fig._suptitle.get_text() if getattr(fig, "_suptitle", None) else "")
    return text or None


def figure_title(fig) -> str | None:
    title = _suptitle(fig)
    if title:
        return title
    if fig.axes:
        t = fig.axes[0].get_title()
        if t:
            return t
    return None


def _script_name() -> str:
    argv0 = sys.argv[0] if sys.argv else ""
    root = os.environ.get("ANALYST_ROOT")
    if argv0 and root:
        try:
            return Path(argv0).resolve().relative_to(Path(root).resolve()).as_posix()
        except (ValueError, OSError):
            pass
    return argv0


def sidecar_for(fig) -> dict:
    axes = []
    for ax in fig.axes:
        axes.append(
            {
                "title": ax.get_title() or None,
                "xlabel": ax.get_xlabel() or None,
                "ylabel": ax.get_ylabel() or None,
            }
        )
    w, h = fig.get_size_inches()
    return {
        "run_id": os.environ.get("ANALYST_RUN_ID"),
        "script": _script_name(),
        "args": list(sys.argv[1:]),
        "title": figure_title(fig),
        "axes": axes,
        "created_at": datetime.now(UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z"),
        "width_in": float(w),
        "height_in": float(h),
        "dpi": float(fig.dpi),
    }


def write_sidecar(fig, image_path: Path) -> Path:
    path = Path(image_path).with_suffix(".json")
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(sidecar_for(fig), indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, path)
    return path


def unique_stem(directory: Path, stem: str) -> str:
    """De-duplicate with ``-2``, ``-3`` ... against any figure or sidecar file."""

    def taken(name: str) -> bool:
        return any((directory / f"{name}{ext}").exists() for ext in (*_FIGURE_EXTS, ".json"))

    if not taken(stem):
        return stem
    n = 2
    while taken(f"{stem}-{n}"):
        n += 1
    return f"{stem}-{n}"


_counter = 0


def _next_number(fig) -> int:
    global _counter
    num = getattr(fig, "number", None)
    if isinstance(num, int) and not isinstance(num, bool) and num > 0:
        return num
    _counter += 1
    return _counter


def save_figure(fig) -> list[Path]:
    """Save one figure as PNG and SVG with a sidecar; returns the written image paths."""
    directory = figures_dir()
    directory.mkdir(parents=True, exist_ok=True)
    title = figure_title(fig)
    stem = slugify(title) if title else ""
    if not stem:
        stem = f"fig-{_next_number(fig)}"
    stem = unique_stem(directory, stem)
    written: list[Path] = []
    canvas = fig.canvas
    for ext in (".png", ".svg"):
        target = directory / f"{stem}{ext}"
        FigureCanvasAgg.print_figure(canvas, str(target))
        written.append(target)
    write_sidecar(fig, written[0])
    return written


class FigureCanvas(FigureCanvasAgg):
    """Agg canvas whose ``print_figure`` (used by ``savefig``) also writes the sidecar."""

    def print_figure(self, filename, *args, **kwargs):
        result = super().print_figure(filename, *args, **kwargs)
        if isinstance(filename, (str, os.PathLike)):
            try:
                write_sidecar(self.figure, Path(filename))
            except OSError:
                pass
        return result


class FigureManager(FigureManagerBase):
    def show(self):  # fig.show() on a single figure: save it, keep it open
        save_figure(self.canvas.figure)


FigureCanvas.manager_class = FigureManager


def new_figure_manager(num, *args, **kwargs):
    from matplotlib.figure import Figure

    FigureClass = kwargs.pop("FigureClass", Figure)
    fig = FigureClass(*args, **kwargs)
    return new_figure_manager_given_figure(num, fig)


def new_figure_manager_given_figure(num, figure):
    canvas = FigureCanvas(figure)
    return FigureManager(canvas, num)


def show(*args, **kwargs):
    """Save every open figure (PNG + SVG + sidecar) under the figures directory, then close them."""
    managers = list(_pylab_helpers.Gcf.get_all_fig_managers())
    for manager in managers:
        save_figure(manager.canvas.figure)
    for manager in managers:
        _pylab_helpers.Gcf.destroy(manager)


def draw_if_interactive():
    return None
