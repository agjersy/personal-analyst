"""Simulate a single-exponential decay and report its half-life."""

from __future__ import annotations

import argparse
import json
import os

import matplotlib.pyplot as plt
import numpy as np

from analysis.models import single_exp


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Simulate a single exponential decay.")
    parser.add_argument("--amplitude", type=float, default=1.0, help="Initial amplitude.")
    parser.add_argument("--tau", type=float, default=1.0, help="Decay time constant.")
    parser.add_argument("--t-max", type=float, default=5.0, help="End time of the simulation.")
    parser.add_argument("--n", type=int, default=200, help="Number of samples.")
    parser.add_argument("--seed", type=int, default=0, help="Random seed for the noise.")
    parser.add_argument("--noise", type=float, default=0.0, help="Gaussian noise standard deviation.")
    return parser


def main(args: argparse.Namespace) -> None:
    rng = np.random.default_rng(args.seed)
    t = np.linspace(0.0, args.t_max, args.n)
    y = single_exp(t, args.amplitude, args.tau) + rng.normal(0.0, args.noise, size=t.shape)
    half_life = args.tau * np.log(2)
    summary = {"tau": args.tau, "amplitude": args.amplitude, "half_life": float(half_life)}
    print(json.dumps(summary))
    out_dir = os.environ.get("ANALYST_OUT_DIR", "out")
    os.makedirs(out_dir, exist_ok=True)
    with open(os.path.join(out_dir, "summary.json"), "w") as f:
        json.dump(summary, f)
    plt.plot(t, y, label=f"tau={args.tau}")
    plt.xlabel("t")
    plt.ylabel("y")
    plt.title("decay")
    plt.legend()
    plt.show()


if __name__ == "__main__":
    main(build_parser().parse_args())
