#!/usr/bin/env python3
"""Fit layout scoring weights to pairwise judgments with a research prior.

Model: P(left preferred) = sigmoid(sum_k w_k * (c_k(right) - c_k(left))), with
w_k = exp(theta_k) > 0. A "same" judgment is a 0.5 soft label. The prior is a
normal distribution on the centered log-weights around the centered log prior
weights, so it constrains the weights' relative shape and leaves their overall
scale to the data. Weights are fitted on the fit split only; the test split
(holdout-v1 views) and the repeated pairs are never used for fitting.

Reported weights are rescaled so the median structural cost over all layouts
equals the median under the current weights, which keeps the existing balance
against movement_weight in reorganization search.

usage: fit.py PAIRS_JSON PICKS_JSON PRIORS_JSON NEW_REPORT_JSON
No packages required. Never overwrites a report.
"""
import json
import math
import pathlib
import random
import statistics
import sys

CURRENT = {"route_length": 1.0, "bends": 0.3, "compactness": 2.0, "aspect_ratio": 1.5,
           "occlusion": 2.0, "distortion": 0.25, "congestion": 1.0}
BOOTSTRAP = 200
SEED = 20261005


def sigmoid(x):
    return 1 / (1 + math.exp(-x)) if x >= 0 else math.exp(x) / (1 + math.exp(x))


def observations(pairs, picks, split):
    rows = []
    for pair in pairs["sequence"]:
        if pair["split"] != split or pair["id"] not in picks:
            continue
        choice = picks[pair["id"]]
        label = {"left": 1.0, "right": 0.0, "same": 0.5}[choice]
        left, right = (pairs["layouts"][pair[side]]["components"] for side in ("left", "right"))
        rows.append(([right[c] - left[c] for c in pairs["components"]], label))
    return rows


def prior_weights(pairs, priors):
    names = pairs["components"]
    spread = {}
    for c in names:
        diffs = [abs(pairs["layouts"][p["left"]]["components"][c] - pairs["layouts"][p["right"]]["components"][c])
                 for p in pairs["sequence"] if p["split"] != "repeat"]
        spread[c] = statistics.mean(diffs)
        if spread[c] <= 0:
            raise ValueError(f"component {c} never differs; it cannot be weighted")
    return {c: priors["importance"][c] / spread[c] for c in names}, spread


def fit(rows, names, prior, sd):
    """MAP estimate by Newton's method on theta = log(w)."""
    k = len(names)
    mu = [math.log(prior[c]) for c in names]
    mu_bar = sum(mu) / k
    theta = mu[:]  # start at the prior; its scale is adjusted by the data
    def objective(t):
        w = [math.exp(v) for v in t]
        nll = 0.0
        for x, y in rows:
            z = sum(wi * xi for wi, xi in zip(w, x))
            p = min(max(sigmoid(z), 1e-12), 1 - 1e-12)
            nll -= y * math.log(p) + (1 - y) * math.log(1 - p)
        t_bar = sum(t) / k
        penalty = sum(((t[i] - t_bar) - (mu[i] - mu_bar)) ** 2 for i in range(k)) / (2 * sd * sd)
        return nll + penalty
    def gradient(t):
        w = [math.exp(v) for v in t]
        g = [0.0] * k
        for x, y in rows:
            z = sum(wi * xi for wi, xi in zip(w, x))
            r = sigmoid(z) - y
            for i in range(k):
                g[i] += r * x[i] * w[i]
        t_bar = sum(t) / k
        d = [(t[i] - t_bar) - (mu[i] - mu_bar) for i in range(k)]
        d_bar = sum(d) / k
        for i in range(k):
            g[i] += (d[i] - d_bar) / (sd * sd)
        return g
    value = objective(theta)
    for _ in range(5000):
        g = gradient(theta)
        if max(abs(v) for v in g) < 1e-7:
            break
        step = 1.0
        while step > 1e-12:
            trial = [theta[i] - step * g[i] for i in range(k)]
            trial_value = objective(trial)
            if trial_value < value - 1e-4 * step * sum(v * v for v in g):
                theta, value = trial, trial_value
                break
            step *= 0.5
        else:
            break
    return {c: math.exp(theta[i]) for i, c in enumerate(names)}


def accuracy(rows, weights, names):
    decided = [(x, y) for x, y in rows if y != 0.5]
    if not decided:
        return None
    correct = 0.0
    for x, y in decided:
        z = sum(weights[c] * x[i] for i, c in enumerate(names))
        correct += 0.5 if z == 0 else float((z > 0) == (y == 1.0))
    return correct / len(decided)


def scaled_log_loss(fit_rows, test_rows, weights, names):
    """Mean test log loss after fitting one scale factor on the fit split."""
    def loss(rows, s):
        total = 0.0
        for x, y in rows:
            p = min(max(sigmoid(s * sum(weights[c] * x[i] for i, c in enumerate(names))), 1e-12), 1 - 1e-12)
            total -= y * math.log(p) + (1 - y) * math.log(1 - p)
        return total / len(rows)
    best = min((loss(fit_rows, math.exp(e / 20)), math.exp(e / 20)) for e in range(-200, 201))[1]
    return loss(test_rows, best) if test_rows else None


def rescale(weights, pairs, names):
    def median_cost(w):
        return statistics.median(sum(w[c] * l["components"][c] for c in names) for l in pairs["layouts"].values())
    factor = median_cost(CURRENT) / median_cost(weights)
    return {c: weights[c] * factor for c in names}


def main(pairs_path, picks_path, priors_path, out):
    if pathlib.Path(out).exists():
        raise SystemExit("output exists; choose a new path")
    pairs = json.loads(pathlib.Path(pairs_path).read_text())
    priors = json.loads(pathlib.Path(priors_path).read_text())
    raw = json.loads(pathlib.Path(picks_path).read_text())
    picks = {p["pair"]: p["choice"] for p in raw["picks"]}
    names = pairs["components"]
    fit_rows = observations(pairs, picks, "fit")
    test_rows = observations(pairs, picks, "test")
    if len(fit_rows) < 20:
        raise SystemExit(f"only {len(fit_rows)} fit judgments; need at least 20")
    prior, spread = prior_weights(pairs, priors)
    sd = priors["prior_log_sd"]
    fitted = fit(fit_rows, names, prior, sd)

    rng = random.Random(SEED)
    boot = {c: [] for c in names}
    for _ in range(BOOTSTRAP):
        sample = [rng.choice(fit_rows) for _ in fit_rows]
        b = rescale(fit(sample, names, prior, sd), pairs, names)
        for c in names:
            boot[c].append(b[c])
    interval = {c: [sorted(v)[int(0.05 * BOOTSTRAP)], sorted(v)[int(0.95 * BOOTSTRAP) - 1]] for c, v in boot.items()}

    sensitivity = {}
    for alt in (0.5, 2.0):
        w = rescale(fit(fit_rows, names, prior, alt), pairs, names)
        sensitivity[str(alt)] = {"weights": w, "test_accuracy": accuracy(test_rows, w, names)}

    repeats = [(p, picks.get(p["repeat_of"]), picks.get(p["id"])) for p in pairs["sequence"] if p["split"] == "repeat"]
    flip = {"left": "right", "right": "left", "same": "same"}
    answered = [(a, b) for _, a, b in repeats if a and b]
    consistency = sum(flip[a] == b for a, b in answered) / len(answered) if answered else None

    candidates = {"current": CURRENT, "prior": rescale(prior, pairs, names), "fitted": rescale(fitted, pairs, names)}
    report = {
        "version": 1, "pairs": pairs_path, "picks": picks_path, "priors": priors_path,
        "judgments": {"fit": len(fit_rows), "test": len(test_rows),
                      "fit_same": sum(y == 0.5 for _, y in fit_rows), "test_same": sum(y == 0.5 for _, y in test_rows),
                      "repeats_answered": len(answered), "repeat_consistency": consistency},
        "component_spread_mean_abs_diff": spread,
        "weights": candidates,
        "fitted_90pct_bootstrap_interval": interval,
        "share_of_typical_difference": {name: {c: w[c] * spread[c] / sum(w[k] * spread[k] for k in names) for c in names}
                                        for name, w in candidates.items()},
        "fit_accuracy": {name: accuracy(fit_rows, w, names) for name, w in candidates.items()},
        "test_accuracy": {name: accuracy(test_rows, w, names) for name, w in candidates.items()},
        "test_log_loss": {name: scaled_log_loss(fit_rows, test_rows, w, names) for name, w in candidates.items()},
        "prior_sd_sensitivity": sensitivity,
        "notes": "Accuracy counts decided (non-'same') judgments; a zero cost difference scores 0.5. Test rows are holdout-v1 views never used for fitting. One rater's preferences are not a population estimate.",
    }
    with open(out, "x") as handle:
        json.dump(report, handle, indent=2)
        handle.write("\n")
    print(json.dumps({k: report[k] for k in ("judgments", "test_accuracy")}, indent=2))


if __name__ == "__main__":
    if len(sys.argv) != 5:
        raise SystemExit(__doc__)
    main(*sys.argv[1:])
