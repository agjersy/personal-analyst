"""Loading run data."""

import csv
from pathlib import Path


def load_runs(path):
    """Load a CSV of (t, y) rows into two lists of floats.

    Parameters: path is a CSV file with a header row.
    """
    ts = []
    ys = []
    with Path(path).open(newline="") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            ts.append(float(row["t"]))
            ys.append(float(row["y"]))
    return ts, ys


def count_rows(path):
    """Number of data rows in a CSV file."""
    with Path(path).open(newline="") as fh:
        return sum(1 for _ in csv.DictReader(fh))
