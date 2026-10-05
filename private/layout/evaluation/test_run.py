"""Tests for evidence rejection; these never build or invoke the placement engine."""
import copy
import importlib.util
import json
import pathlib
import tempfile
import types
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("evaluation_run", pathlib.Path(__file__).with_name("run.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class EvidenceChecks(unittest.TestCase):
    def test_invalid_or_missing_candidate_cannot_pass(self):
        case = {"id": "a", "role": "training"}
        valid = {"profile": {"algorithms": ["Floorplan", "Layered"]}, "cases": [
            {"case": case, "results": [{"algorithm": "Floorplan", "status": "valid"}, {"algorithm": "Layered", "status": "valid"}]}]}
        check = lambda result: runner.valid_results(result, valid["profile"], [case])
        self.assertTrue(check(valid))
        failed = copy.deepcopy(valid)
        failed["cases"][0]["results"][1]["status"] = "failed"
        self.assertFalse(check(failed))
        failed["cases"][0]["results"].pop()
        self.assertFalse(check(failed))
        failed["cases"] = []
        self.assertFalse(check(failed))
        missing_case = copy.deepcopy(valid)
        missing_case["cases"][0]["case"] = {"id": "different", "role": "training"}
        self.assertFalse(check(missing_case))
        duplicate_case = copy.deepcopy(valid)
        duplicate_case["cases"] *= 2
        self.assertFalse(check(duplicate_case))
        reduced_algorithms = copy.deepcopy(valid)
        reduced_algorithms["profile"]["algorithms"].pop()
        reduced_algorithms["cases"][0]["results"].pop()
        self.assertFalse(check(reduced_algorithms))

    def test_duplicate_fields_and_existing_reports_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "report.json"
            path.write_text('{"version":1,"version":2}')
            with self.assertRaisesRegex(ValueError, "duplicate"):
                runner.read(path)
            with self.assertRaises(FileExistsError):
                runner.write_new(path, {})

    def test_source_or_corpus_changes_invalidate_training(self):
        case = {"role": "training", "id": "a"}
        manifest = {"profile": {"quality_version": 5, "algorithms": ["Floorplan"]}, "cases": [case]}
        sources = {"solver.rs": "hash"}
        evidence = {"role": "training", "all_valid": True, "manifest_sha256": "manifest",
            "sources": sources, "evaluation": {"profile": manifest["profile"], "cases": [{"case": case,
                "results": [{"algorithm": "Floorplan", "status": "valid"}]}]}}
        runner.check_training(evidence, manifest, sources, "manifest")
        for changed_sources, changed_manifest in [({"solver.rs": "different"}, "manifest"), (sources, "different")]:
            with self.assertRaisesRegex(ValueError, "does not match"):
                runner.check_training(evidence, manifest, changed_sources, changed_manifest)
        with self.assertRaisesRegex(ValueError, "does not match"):
            runner.check_training(evidence, {**manifest, "cases": []}, sources, "manifest")
        evidence["evaluation"]["cases"][0]["results"][0]["status"] = "failed"
        with self.assertRaisesRegex(ValueError, "does not match"):
            runner.check_training(evidence, manifest, sources, "manifest")
        evidence["evaluation"]["cases"][0]["results"] = []
        with self.assertRaisesRegex(ValueError, "does not match"):
            runner.check_training(evidence, manifest, sources, "manifest")

    def test_fixture_path_cannot_escape_repository(self):
        for value in ["../outside", "/tmp/not-a-fixture"]:
            with self.assertRaisesRegex(ValueError, "stay in the repository"):
                runner.fixture_path(value)

    def test_executes_cargo_artifact_when_target_directory_is_redirected(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = pathlib.Path(directory) / "redirected-evaluate-cases"
            executable.write_bytes(b"artifact marker; never executed by this test")
            message = {"reason": "compiler-artifact", "target": {
                "name": "evaluate_cases", "kind": ["example"]}, "executable": str(executable)}
            with mock.patch.object(runner.subprocess, "run", return_value=types.SimpleNamespace(stdout=json.dumps(message))) as build:
                self.assertEqual(runner.build_executable(), executable.resolve())
                self.assertIn("--message-format=json", build.call_args.args[0])
            message["target"]["kind"] = ["lib"]
            with mock.patch.object(runner.subprocess, "run", return_value=types.SimpleNamespace(stdout=json.dumps(message))):
                with self.assertRaisesRegex(ValueError, "exactly one"):
                    runner.build_executable()


if __name__ == "__main__":
    unittest.main()
