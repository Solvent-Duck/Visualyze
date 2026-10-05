#!/usr/bin/env python3
"""Build the calibration-v1 pairwise judgment set from frozen evaluation reports.

Every pair compares two geometrically distinct layouts of the same view. Pairs
from training/development views form the fit split; pairs from holdout-v1 views
form the test split. The rating page never shows the split. A fixed seed sets
presentation order and sides; it never selects or drops layouts.

usage: build_pairs.py NEW_PAIRS_JSON
"""
import hashlib
import itertools
import json
import pathlib
import random
import sys

ROOT = pathlib.Path(__file__).resolve().parents[4]
REPORTS = {
    "fit": ["docs/measurements/quality5-holdout-v1-training-2026-10-05.json",
            "docs/measurements/quality5-holdout-v1-development-2026-10-05.json"],
    "test": ["docs/measurements/quality5-holdout-v1-held-out-2026-10-05.json"],
}
COMPONENTS = ["route_length", "bends", "compactness", "aspect_ratio", "occlusion", "distortion", "congestion"]
SEED = 20261005
REPEATS = 12


def compact(solution):
    boxes = [[id, *(b["bounds"]["min"][a] for a in "xyz"), *(b["bounds"]["max"][a] for a in "xyz"), b["solid"]]
             for id, b in sorted(solution["boxes"].items())]
    routes = [[[p["x"], p["y"], p["z"]] for p in r["points"]] for r in solution["routes"]]
    return {"boxes": boxes, "routes": routes}


def main(out):
    if pathlib.Path(out).exists():
        raise SystemExit("output exists; choose a new path")
    layouts, pairs, sources = {}, [], {}
    for split, paths in REPORTS.items():
        for path in paths:
            data = (ROOT / path).read_bytes()
            sources[path] = hashlib.sha256(data).hexdigest()
            report = json.loads(data)
            if not report["all_valid"]:
                raise SystemExit(f"{path} contains invalid layouts")
            for case in report["evaluation"]["cases"]:
                distinct = {}
                for row in case["results"]:
                    geometry = compact(row["solution"])
                    digest = hashlib.sha256(json.dumps(geometry, sort_keys=True).encode()).hexdigest()
                    if digest in distinct:
                        continue
                    key = f'{case["case"]["id"]}/{row["algorithm"]}'
                    distinct[digest] = key
                    layouts[key] = {"view": case["case"]["id"], "algorithm": row["algorithm"],
                                    "geometry_sha256": digest,
                                    "components": {c: row["score_components"][c] for c in COMPONENTS},
                                    "structural_cost": row["metrics"]["structural_cost"], **geometry}
                for a, b in itertools.combinations(sorted(distinct.values()), 2):
                    pairs.append({"view": case["case"]["id"], "split": split, "a": a, "b": b})
    rng = random.Random(SEED)
    rng.shuffle(pairs)
    for index, pair in enumerate(pairs):
        pair["id"] = f"p{index + 1:03d}"
        if rng.random() < 0.5:
            pair["a"], pair["b"] = pair["b"], pair["a"]
        pair["left"], pair["right"] = pair.pop("a"), pair.pop("b")
    # Repeats show an earlier pair again with sides swapped, in the second half,
    # to measure rater consistency. They are excluded from fitting.
    repeats = [{"id": f"r{i + 1:02d}", "view": p["view"], "split": "repeat", "repeat_of": p["id"],
                "left": p["right"], "right": p["left"]}
               for i, p in enumerate(rng.sample(pairs[: len(pairs) // 2], REPEATS))]
    sequence = pairs[:]
    for repeat in repeats:
        sequence.insert(rng.randrange(len(pairs) // 2 + 1, len(sequence) + 1), repeat)
    result = {"version": 1, "seed": SEED, "components": COMPONENTS, "sources": sources,
              "question": "Which layout makes the architecture's structure easier to read?",
              "counts": {s: sum(p["split"] == s for p in sequence) for s in ("fit", "test", "repeat")},
              "sequence": sequence, "layouts": layouts}
    with open(out, "x") as handle:
        json.dump(result, handle, separators=(",", ":"))
        handle.write("\n")
    print(json.dumps(result["counts"]))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    main(sys.argv[1])
