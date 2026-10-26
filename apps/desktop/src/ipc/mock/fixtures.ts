// In-memory repository used by the MockBackend (IPC.md section 6).

export const PROJECT_TOML = `[project]
name = "decay-analysis"
library = "analysis"
scripts = ["scripts/**/*.py"]
out_dir = "out"
data_dirs = ["data"]

[policy]
edits = "review"
runs = "auto"
packages = "ask"
shell = "ask"
network = "deny"
git = "ask"
protected = ["data/**", ".analyst/**"]
max_steps = 40
`;

export const ANALYSIS_INIT = `"""Analysis library for the decay-analysis project."""

from .models import residuals, single_exp

__all__ = ["single_exp", "residuals"]
`;

export const MODELS_PY = `"""Model functions for decay fits."""

import numpy as np


def single_exp(t, A, tau, C=0.0):
    """Single exponential decay with a constant offset.

    Math:
        y = A * exp(-t / tau) + C
    """
    return A * np.exp(-t / tau) + C


def residuals(y, y_fit):
    """Difference between observed and fitted values."""
    return y - y_fit
`;

export const DOUBLE_EXP_SNIPPET = `

def double_exp(t, A, tau1, B, tau2, C=0.0):
    """Sum of two exponential decays with a constant offset.

    Math:
        y = A * exp(-t / tau1) + B * exp(-t / tau2) + C
    """
    return A * np.exp(-t / tau1) + B * np.exp(-t / tau2) + C
`;

export const DECAY_SIM_PY = `"""Simulate a noisy exponential decay and fit it back."""

import argparse

import matplotlib.pyplot as plt
import numpy as np
from scipy.optimize import curve_fit

from analysis.models import residuals, single_exp


def main() -> None:
    parser = argparse.ArgumentParser(description="Simulate and fit a single exponential decay.")
    parser.add_argument("--n", type=int, default=200, help="number of samples")
    parser.add_argument("--tau", type=float, default=2.5, help="true decay constant")
    parser.add_argument("--amplitude", type=float, default=1.0, help="true amplitude")
    parser.add_argument("--noise", type=float, default=0.05, help="gaussian noise sigma")
    parser.add_argument("--seed", type=int, default=0, help="random seed")
    args = parser.parse_args()

    rng = np.random.default_rng(args.seed)
    t = np.linspace(0.0, 5 * args.tau, args.n)
    y = single_exp(t, args.amplitude, args.tau) + rng.normal(0.0, args.noise, args.n)
    popt, _ = curve_fit(single_exp, t, y, p0=(1.0, 1.0, 0.0))
    print(f"tau_fit = {popt[1]:.3f}")

    fig, ax = plt.subplots()
    ax.set_title("Decay simulation")
    ax.plot(t, y, ".", label="data")
    ax.plot(t, single_exp(t, *popt), "-", label="fit")
    ax.set_xlabel("t")
    ax.set_ylabel("y")
    ax.legend()
    plt.show()
    print("residual rms =", float(np.sqrt(np.mean(residuals(y, single_exp(t, *popt)) ** 2))))


if __name__ == "__main__":
    main()
`;

export const BAD_PY = `"""A deliberately flawed script used to exercise the checker."""

import argparse


def main() -> None:
    parser = argparse.ArgumentParser(description="Load a run from a hard-coded location.")
    parser.add_argument("--scale", type=float, default=1.0, help="scale factor")
    args = parser.parse_args()
    data = open("/Users/me/data/run3.csv").read()
    values = np.array([float(x) for x in data.split()]) * args.scale
    print(values.mean())


if __name__ == "__main__":
    main()
`;

export const SAMPLE_CSV = `t,y
0.0,1.012
0.5,0.823
1.0,0.671
1.5,0.544
2.0,0.452
2.5,0.371
3.0,0.299
3.5,0.247
4.0,0.198
4.5,0.166
5.0,0.137
`;

export const NOTES_MD = `# Session notes

Decay fits on the sample runs. The model is

$$ y = A e^{-t/\\tau} + C $$

- \`scripts/decay_sim.py\` simulates a decay and fits it back.
- Residuals should be flat and white if the single exponential is adequate.

## Todo

- Try a double exponential on \`data/run3\`.
`;

export const README_MD = `# decay-analysis

An analysis repo managed by Personal Analyst.
`;

export function fixtureFiles(): Map<string, string> {
  return new Map<string, string>([
    [".analyst/project.toml", PROJECT_TOML],
    ["analysis/__init__.py", ANALYSIS_INIT],
    ["analysis/models.py", MODELS_PY],
    ["scripts/decay_sim.py", DECAY_SIM_PY],
    ["scripts/bad.py", BAD_PY],
    ["data/sample.csv", SAMPLE_CSV],
    ["notes.md", NOTES_MD],
    ["README.md", README_MD],
  ]);
}
