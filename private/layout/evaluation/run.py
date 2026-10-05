#!/usr/bin/env python3
"""Hash-checked layout evaluations. Python is development-only; no packages needed.

training MANIFEST NEW_REPORT
freeze MANIFEST TRAINING_REPORT NEW_LOCK
development|held_out MANIFEST LOCK NEW_REPORT
Never overwrites reports, changes a fixture, downloads data, or tunes on held-out results.
"""
import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile
from datetime import datetime, timezone

ROOT = pathlib.Path(__file__).resolve().parents[3]
ROLES = {"training", "development", "held_out"}


def read(path):
    def unique(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError(f"duplicate JSON key: {key}")
            value[key] = item
        return value
    return json.loads(pathlib.Path(path).read_text(), object_pairs_hook=unique)


def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def write_new(path, value):
    with pathlib.Path(path).open("x") as output:
        json.dump(value, output, indent=2, allow_nan=False)
        output.write("\n")


def fixture_path(value):
    relative = pathlib.PurePosixPath(value)
    if relative.is_absolute() or ".." in relative.parts:
        raise ValueError(f"fixture path must stay in the repository: {value}")
    path = (ROOT / value).resolve(strict=True)
    if not path.is_relative_to(ROOT) or not path.is_file():
        raise ValueError(f"fixture is not a repository file: {value}")
    return path


def validate_manifest(manifest):
    if manifest.get("version") != 1 or not manifest.get("cases"):
        raise ValueError("expected version1 manifest with explicit cases")
    ids = set()
    fixture_roles = {}
    for case in manifest["cases"]:
        if case["id"] in ids or case["role"] not in ROLES:
            raise ValueError("duplicate case identity or unknown role")
        ids.add(case["id"])
        if case["format"] not in {"snapshot", "bundle"}:
            raise ValueError("unknown format")
        path = fixture_path(case["path"])
        if digest(path) != case["sha256"]:
            raise ValueError(f"fixture bytes changed: {case['path']}")
        fixture_roles.setdefault(case["sha256"], set()).add(case["role"])
    if any("held_out" in roles and len(roles) > 1 for roles in fixture_roles.values()):
        raise ValueError("one fixture cannot be both held out and used for training/development")


def source_identity():
    paths = {ROOT / "Cargo.lock", ROOT / "Cargo.toml", ROOT / "rust-toolchain.toml",
        ROOT / "public/schema/architecture.schema.json", ROOT / "public/examples/objects.json",
        ROOT / "public/examples/database.json", pathlib.Path(__file__).resolve()}
    paths.update(path for path in (ROOT / ".cargo/config", ROOT / ".cargo/config.toml") if path.is_file())
    for package in ("private/layout", "public/types/rust"):
        paths.add(ROOT / package / "Cargo.toml")
        paths.update((ROOT / package / "src").rglob("*.rs"))
    paths.add(ROOT / "private/layout/examples/evaluate_cases.rs")
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(paths)}


def check_training(training, manifest, sources, manifest_hash):
    expected = [case for case in manifest["cases"] if case["role"] == "training"]
    if (training.get("role") != "training" or not training.get("all_valid")
            or training.get("manifest_sha256") != manifest_hash
            or training.get("sources") != sources
            or training.get("evaluation", {}).get("profile") != manifest["profile"]
            or [case["case"] for case in training.get("evaluation", {}).get("cases", [])] != expected
            or not valid_results(training["evaluation"], manifest["profile"], expected)):
        raise ValueError("training evidence does not match this exact manifest/profile/source or has failures")


def build_executable():
    built = subprocess.run(["cargo", "build", "-p", "visualyze-layout", "--example", "evaluate_cases",
        "--offline", "--locked", "--message-format=json"], cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
    artifacts = [message for line in built.stdout.splitlines() if (message := json.loads(line)).get("reason") == "compiler-artifact"
        and message["target"]["name"] == "evaluate_cases" and "example" in message["target"]["kind"] and message.get("executable")]
    if len(artifacts) != 1:
        raise ValueError("Cargo did not identify exactly one evaluation executable")
    return pathlib.Path(artifacts[0]["executable"]).resolve(strict=True)


def run_evaluation(manifest, role, executable):
    cases = [case for case in manifest["cases"] if case["role"] == role]
    if not cases:
        raise ValueError(f"no {role} cases declared; refusing an empty pass")
    with tempfile.TemporaryDirectory(prefix="visualyze-evaluation-") as directory:
        request = pathlib.Path(directory) / "request.json"
        result = pathlib.Path(directory) / "report.json"
        write_new(request, {"profile": manifest["profile"], "cases": cases})
        subprocess.run([str(executable), str(request), str(result)], cwd=ROOT, check=True)
        return read(result)


def valid_results(evaluation, expected_profile, expected_cases):
    algorithms = expected_profile["algorithms"]
    return (bool(expected_cases) and evaluation.get("profile") == expected_profile
        and [case.get("case") for case in evaluation.get("cases", [])] == expected_cases and all(
        [row.get("algorithm") for row in case.get("results", [])] == algorithms
        and all(row.get("status") == "valid" for row in case["results"])
        for case in evaluation["cases"]
    ))


def main(args):
    if len(args) not in {3, 4} or args[0] not in ROLES | {"freeze"}:
        raise ValueError(__doc__)
    mode, manifest_path = args[:2]
    if len(args) != (3 if mode == "training" else 4):
        raise ValueError(__doc__)
    if pathlib.Path(args[-1]).exists():
        raise ValueError("output already exists; every run requires a new path")
    manifest = read(manifest_path)
    validate_manifest(manifest)
    manifest_hash = digest(manifest_path)
    sources = source_identity()
    if mode == "freeze":
        training = read(args[2])
        check_training(training, manifest, sources, manifest_hash)
        write_new(args[3], {"version": 1, "manifest_sha256": manifest_hash,
            "profile": manifest["profile"], "sources": sources,
            "executable_sha256": training["executable_sha256"],
            "training_report_sha256": digest(args[2]),
            "frozen_at_utc": datetime.now(timezone.utc).isoformat(),
            "scope": "Computational profile freeze after training validity checks. Human calibration and comprehension remain separate gates."})
        print("Frozen exact source/profile/corpus after checked training evidence.")
        return
    lock = None if mode == "training" else read(args[2])
    if lock is not None and (lock.get("version") != 1 or lock.get("manifest_sha256") != manifest_hash
            or lock.get("sources") != sources or lock.get("profile") != manifest["profile"]):
        raise ValueError("frozen source/profile/corpus changed; comparison refused")
    executable = build_executable()
    executable_hash = digest(executable)
    if lock is not None and lock.get("executable_sha256") != executable_hash:
        raise ValueError("compiled executable differs from the training freeze")
    active = json.loads(subprocess.check_output([str(executable), "--profile"], cwd=ROOT))
    if active != manifest["profile"]:
        raise ValueError("active coefficients/budgets differ from declared profile")
    evaluation = run_evaluation(manifest, mode, executable)
    validate_manifest(manifest)
    if sources != source_identity() or manifest_hash != digest(manifest_path) or executable_hash != digest(executable):
        raise ValueError("source/fixture/manifest changed during evaluation; no stable result published")
    result = {"version": 1, "role": mode, "manifest_sha256": manifest_hash,
        "sources": sources, "executable_sha256": executable_hash,
        "executable": str(executable),
        "freeze_sha256": None if lock is None else digest(args[2]),
        "all_valid": valid_results(evaluation, manifest["profile"], [case for case in manifest["cases"] if case["role"] == mode]), "evaluation": evaluation,
        "compiler": subprocess.check_output(["rustc", "-Vv"], cwd=ROOT, text=True).strip(),
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "notes": "Headless CPU observations; includes every declared case/algorithm and every failure. No GPU or human comprehension claim. Initial placement has no previous layout. Historical fixtures remain development cases; a computational freeze alone is not weight calibration."}
    write_new(args[-1], result)
    print(json.dumps({"role": mode, "cases": len(evaluation["cases"]), "all_valid": result["all_valid"], "report": args[-1]}))
    if not result["all_valid"]:
        raise SystemExit(1)


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
