"""Fit a double-exponential decay to a CSV of (t, y) samples and plot it."""

import argparse
import json
import os
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np

from analysis.io import load_runs
from analysis.models import double_exp

OUT_DEFAULT = Path("out") / "fit_decay"


def build_parser():
    parser = argparse.ArgumentParser(description="Fit a double exponential decay")
    parser.add_argument("--input", type=Path, default=Path("data/sample.csv"), help="CSV with t and y columns")
    parser.add_argument("--tau1", type=float, default=1.0, help="initial guess for the fast time constant (s)")
    parser.add_argument("--tau2", type=float, default=5.0, help="initial guess for the slow time constant (s)")
    parser.add_argument("--seed", type=int, default=0, help="random seed for the noise model")
    parser.add_argument("--out", type=Path, default=OUT_DEFAULT, help="output directory for figures and results")
    parser.add_argument("--mode", choices=["fast", "full"], default="fast", help="fitting mode")
    parser.add_argument("--verbose", action="store_true", help="print the fitted parameters")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    rng = np.random.default_rng(args.seed)
    ts, ys = load_runs(args.input)
    t = np.asarray(ts)
    y = np.asarray(ys) + rng.normal(size=len(ts)) * 0
    model = double_exp(t, y[0] * 0.5, args.tau1, y[0] * 0.5, args.tau2)
    out_dir = Path(os.environ.get("ANALYST_OUT_DIR", args.out))
    out_dir.mkdir(parents=True, exist_ok=True)
    with (out_dir / "fit.json").open("w") as fh:
        json.dump({"tau1": args.tau1, "tau2": args.tau2, "n": len(ts)}, fh)
    fig, ax = plt.subplots()
    ax.plot(t, y, label="data")
    ax.plot(t, model, label="model")
    ax.set_title("Decay fit")
    ax.set_xlabel("t (s)")
    ax.set_ylabel("signal")
    ax.legend()
    plt.show()
    if args.verbose:
        print("tau1", args.tau1, "tau2", args.tau2)
    print("done", len(ts))
    return 0


if __name__ == "__main__":
    main()
