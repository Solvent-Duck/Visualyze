# Calibration v1: scoring weights from pairwise judgments

The layout engine chooses among candidates by minimizing a weighted sum of seven
measurements (`private/layout/src/quality.rs`). Before this procedure those
weights were set by hand. This directory fits them to a rater's pairwise
preferences, with a research-based prior on their relative importance.

## Inputs

- `pairs.json` is built by `build_pairs.py` from the frozen holdout-v1
  training, development and held-out reports. Every pair compares two
  geometrically distinct layouts of one view (78 fit pairs from training and
  development views, 39 test pairs from holdout-v1 views). Twelve pairs repeat
  with sides swapped to measure rater consistency. A fixed seed sets order and
  sides only.
- `priors.json` gives relative importance per measurement, with its basis
  (literature or judgment). `fit.py` divides importance by each measurement's
  mean within-pair difference, computed from layouts alone, to get prior
  weights.
- The rating page is `rate.template.html` rendered by `make_page.py`. It hides
  splits, view names and algorithm names. It renders boxes and routes neutrally
  with the native viewer's default camera angle, and both views share one orbit.

## Procedure

1. Rate every pair on the published page (Left / About the same / Right).
2. Export the picks from the page's `picks/<rater>/items` collection to
   `picks-<date>.json` as `{"picks": [{"pair", "choice"}]}`.
3. `python3 fit.py pairs.json picks-<date>.json priors.json fit-<date>.json`
4. The report compares current, prior-only and fitted weights on the held-out
   test pairs (accuracy and log loss), with bootstrap intervals, prior-strength
   sensitivity and repeat consistency.
5. Adopting fitted weights is an engine change: bump the quality profile, rerun
   training, freeze, development and held-out, and record the results. After that
   change, holdout-v1 views are development evidence.

## Limits

One rater's preferences calibrate to that rater. Pairs come from the four
algorithms' final layouts, so the measurements are partly correlated and some
weights are weakly identified; the prior carries those. The fit says nothing
about comprehension tasks such as tracing a path or finding auth coverage.
