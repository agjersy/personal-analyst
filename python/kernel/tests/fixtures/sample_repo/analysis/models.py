"""Decay models."""

import numpy as np


def double_exp(t, A, tau1, B, tau2):
    """Two-component exponential decay.

    Parameters are in seconds (t, tau1, tau2) and arbitrary amplitude units (A, B).

    Math:
        y = A*exp(-t/tau1) + B*exp(-t/tau2)
    """
    first = A * np.exp(-t / tau1)
    second = B * np.exp(-t / tau2)
    return first + second


def damped_cos(t, A, tau, omega):
    """Damped cosine with a deliberately wrong declaration (the code uses sin).

    Math:
        y = A*exp(-t/tau)*cos(omega*t)
    """
    out = np.zeros_like(np.asarray(t, dtype=float))
    out += A * np.exp(-t / tau) * np.sin(omega * t)
    return out


def gaussian(x, mu, sigma):
    return np.exp(-((x - mu) ** 2) / (2 * sigma**2)) / (sigma * np.sqrt(2 * np.pi))


def running_sum(x, n):
    """Cumulative sum of the first n elements with a loop (not derivable).

    LaTeX:
        S_n = \\sum_{i=0}^{n-1} x_i
    """
    total = 0.0
    for i in range(int(n)):
        total += x[i]
    return total


def power_law(x, a, k):
    """Power law with a Math: section that does not parse.

    Math:
        y = a*x^^k
    """
    return a * np.power(x, k)
