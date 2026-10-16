"""Signal processing helpers."""

from __future__ import annotations

import numpy as np


def moving_average(x: np.ndarray, window: int) -> np.ndarray:
    """Centered moving average with edge padding.

    Parameters
    ----------
    x : ndarray
        Input samples.
    window : int
        Window length in samples; must be odd and at least 1.

    Returns
    -------
    ndarray
        Smoothed samples, same length as `x`.
    """
    if window < 1 or window % 2 == 0:
        raise ValueError("window must be an odd positive integer")
    half = window // 2
    padded = np.pad(x, half, mode="edge")
    kernel = np.ones(window) / window
    return np.convolve(padded, kernel, mode="valid")


def load_series(path: str, t_col: str = "t", v_col: str = "v") -> tuple[np.ndarray, np.ndarray]:
    """Load a two-column time series from a CSV file with a header row."""
    data = np.genfromtxt(path, delimiter=",", names=True)
    return np.asarray(data[t_col]), np.asarray(data[v_col])
