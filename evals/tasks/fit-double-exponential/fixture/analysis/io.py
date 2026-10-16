"""Data loading helpers."""

from __future__ import annotations

import csv
from pathlib import Path

import numpy as np


def load_xy(path: str | Path, x_col: str, y_col: str) -> tuple[np.ndarray, np.ndarray]:
    """Load two numeric columns from a CSV file.

    Parameters
    ----------
    path : str or Path
        CSV file with a header row.
    x_col, y_col : str
        Column names to read.

    Returns
    -------
    x, y : ndarray
        Float arrays in file order.
    """
    xs: list[float] = []
    ys: list[float] = []
    with open(path, newline="") as f:
        for row in csv.DictReader(f):
            xs.append(float(row[x_col]))
            ys.append(float(row[y_col]))
    return np.asarray(xs), np.asarray(ys)
