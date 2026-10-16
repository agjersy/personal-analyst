import csv
import numpy as np
import matplotlib.pyplot as plt

rows = list(csv.DictReader(open("/home/someone/projects/measurements.csv")))
x = np.array([float(r["x"]) for r in rows])
y = np.array([float(r["y"]) for r in rows])

def linfit(x, y):
    A = np.vstack([x, np.ones_like(x)]).T
    m, c = np.linalg.lstsq(A, y, rcond=None)[0]
    return m, c

def rmse(y, yhat):
    return np.sqrt(np.mean((y - yhat) ** 2))

m, c = linfit(x, y)
yhat = m * x + c
print("slope", m, "intercept", c, "rmse", rmse(y, yhat))
plt.plot(x, y, ".", label="data")
plt.plot(x, yhat, "-", label="fit")
plt.legend()
plt.title("linear fit")
plt.show()
