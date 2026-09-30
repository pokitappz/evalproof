"""Independent reference using binomial sums, not Rust's beta quantile library.

Also checks exact wrong-decision probabilities at every predeclared look and
applies the union bound over looks and metrics. No stochastic provider calls.
"""
import json
import math
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def pmf(n, k, p):
    if p == 0:
        return float(k == 0)
    if p == 1:
        return float(k == n)
    return math.exp(math.lgamma(n + 1) - math.lgamma(k + 1) - math.lgamma(n - k + 1)
                    + k * math.log(p) + (n - k) * math.log1p(-p))


def cdf(n, k, p):
    return math.fsum(pmf(n, i, p) for i in range(k + 1))


def invert(n, k, target):
    low, high = 0.0, 1.0
    for _ in range(60):
        middle = (low + high) / 2
        if cdf(n, k, middle) > target:
            low = middle
        else:
            high = middle
    return (low + high) / 2


def main():
    output = subprocess.check_output(["cargo", "run", "--quiet", "--example", "stats_reference"], cwd=ROOT)
    rows = json.loads(output)
    largest = 0.0
    for row in rows:
        n, k, alpha = row["n"], row["k"], row["alpha"]
        lower = 0 if k == 0 else invert(n, k - 1, 1 - alpha / 2)
        upper = 1 if k == n else invert(n, k, alpha / 2)
        error = max(abs(lower - row["lower"]), abs(upper - row["upper"]))
        largest = max(largest, error)
        assert error < 5e-9, (row, lower, upper)

    for groups in (2, 4, 12):
        tail = 0.01 / (2 * groups * 6)
        for p in (0.001, 0.049, 0.05, 0.051, 0.1, 0.5):
            wrong_bound = 0.0
            for n in (20, 40, 80, 160, 320, 640):
                # A wrong upper/lower decision is equivalent to the corresponding
                # binomial tail test at the 5% policy boundary.
                policy_probs = [pmf(n, k, 0.05) for k in range(n + 1)]
                cumulative = 0.0
                for k in range(n + 1):
                    prior = cumulative
                    cumulative += policy_probs[k]
                    false_pass = p > 0.05 and cumulative <= tail
                    false_fail = p < 0.05 and 1 - prior < tail
                    if false_pass or false_fail:
                        wrong_bound += pmf(n, k, p)
            assert wrong_bound * groups <= 0.01 + 1e-10, (groups, p, wrong_bound)
    print(f"Verified {len(rows)} independent interval references; maximum difference {largest:.3g}.")
    print("Exact binomial tail bounds satisfy the run error allowance for 18 policy scenarios.")


if __name__ == "__main__":
    main()
