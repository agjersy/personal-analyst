"""A script that violates the contract in every way the checker must catch."""

import argparse
import csv
from pathlib import Path

import numpy as np

DATA = "/data/shared/runs.csv"


def load_runs(path):
    ts = []
    ys = []
    with Path(path).open(newline="") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            ts.append(float(row["t"]))
            ys.append(float(row["y"]))
    return ts, ys


def load_runs_v2(path):
    ts = []
    ys = []
    with Path(path).open(newline="") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            ts.append(float(row["time"]))
            ys.append(float(row["value"]))
    print("loaded", len(ts))
    return ts, ys


def smooth(y, width):
    kernel = np.ones(width) / width
    return np.convolve(y, kernel, mode="same")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--width", type=int, default=5)
    args = parser.parse_args()
    ts, ys = load_runs("data/sample.csv")
    sm = smooth(np.asarray(ys), args.width)
    noise = np.random.normal(size=len(ts), scale=3.5)
    np.save("results/smooth.npy", sm + noise)
    print(sm.mean())


main()
