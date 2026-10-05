#!/usr/bin/env python3
"""Render the rating page from pairs.json. The page omits splits, view names and
algorithm names; layouts get opaque keys L001.. in sorted layout-key order.

usage: make_page.py PAIRS_JSON NEW_PAGE_HTML
"""
import json
import pathlib
import sys

DIRECTORY = pathlib.Path(__file__).resolve().parent


def main(pairs_path, out):
    if pathlib.Path(out).exists():
        raise SystemExit("output exists; choose a new path")
    pairs = json.loads(pathlib.Path(pairs_path).read_text())
    opaque = {key: f"L{index + 1:03d}" for index, key in enumerate(sorted(pairs["layouts"]))}
    data = {
        "pairs": [{"id": p["id"], "left": opaque[p["left"]], "right": opaque[p["right"]]} for p in pairs["sequence"]],
        "layouts": {opaque[k]: {"boxes": v["boxes"], "routes": v["routes"]} for k, v in pairs["layouts"].items()},
    }
    template = (DIRECTORY / "rate.template.html").read_text()
    if template.count("/*__DATA__*/null") != 1:
        raise SystemExit("template must contain exactly one data placeholder")
    page = template.replace("/*__DATA__*/null", json.dumps(data, separators=(",", ":")))
    with open(out, "x") as handle:
        handle.write(page)
    print(f"{len(data['pairs'])} pairs, {len(data['layouts'])} layouts, {len(page)} bytes")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    main(*sys.argv[1:])
