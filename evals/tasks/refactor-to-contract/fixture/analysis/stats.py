"""Basic statistics helpers."""

from __future__ import annotations

import numpy as np


def rmse(y: np.ndarray, yhat: np.ndarray) -> float:
    """Root mean square error between observations and predictions.

    Math:
        rmse = sqrt(mean((y - yhat)**2))
    """
    return float(np.sqrt(np.mean((y - yhat) ** 2)))
