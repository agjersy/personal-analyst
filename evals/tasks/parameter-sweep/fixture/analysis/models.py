"""Simple decay models."""

from __future__ import annotations

import numpy as np


def single_exp(t: np.ndarray, amplitude: float, tau: float) -> np.ndarray:
    """Single exponential decay.

    Math:
        y = amplitude*exp(-t/tau)
    """
    return amplitude * np.exp(-t / tau)
