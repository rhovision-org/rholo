"""The preflight refuses a run whose inputs cannot support an explanation.

Every case is a `not run` coordinate: the claim has not been tested, so a claim fail would be a
false accusation of the code.
"""
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(common)

HEAD = "a" * 40


class Result:
    def __init__(self, returncode=0, stdout=""):
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = ""


def runner(*, docker=0, image=0, label=HEAD, status="", head=HEAD):
    def run(command):
        if command[:2] == ["docker", "info"]:
            return Result(docker)
        if command[:3] == ["docker", "image", "inspect"] and "--format" in command:
            return Result(0 if image == 0 else image, label)
        if command[:3] == ["docker", "image", "inspect"]:
            return Result(image)
        if command[:2] == ["git", "rev-parse"]:
            return Result(0, head)
        if command[:2] == ["git", "status"]:
            return Result(0, status)
        raise AssertionError(f"unexpected command {command}")
    return run


class PreflightTests(unittest.TestCase):
    def test_a_matching_image_and_clean_tree_pass(self):
        self.assertEqual(common.preflight("local", runner=runner(), head=HEAD), [])

    def test_a_missing_image_names_the_build_command(self):
        problems = common.preflight("local", runner=runner(image=1), head=HEAD)
        self.assertTrue(any("missing" in p and "devnet.sh build" in p for p in problems))

    def test_an_unlabelled_image_is_refused(self):
        problems = common.preflight("local", runner=runner(label=""), head=HEAD)
        self.assertTrue(any("revision" in p for p in problems))

    def test_a_stale_image_names_both_revisions(self):
        problems = common.preflight("local", runner=runner(label="b" * 40), head=HEAD)
        self.assertTrue(any("not HEAD" in p for p in problems))

    def test_a_dirty_tree_is_refused_unless_allowed(self):
        problems = common.preflight("local", runner=runner(status=" M casper/src/x.rs"),
                                    head=HEAD)
        self.assertTrue(any("dirty" in p for p in problems))
        with mock.patch.dict(os.environ, {"RELEASE_ALLOW_DIRTY": "1"}):
            self.assertEqual(
                common.preflight("local", runner=runner(status=" M casper/src/x.rs"), head=HEAD), [])

    def test_a_public_run_needs_a_manifest(self):
        problems = common.preflight("public", None, runner=runner(), head=HEAD)
        self.assertTrue(any("--public-manifest" in p for p in problems))

    def test_a_manifest_from_another_revision_is_refused(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "manifest.json"
            path.write_text(json.dumps({"provenance": {"source_sha": "c" * 40}}))
            problems = common.preflight("public", str(path), runner=runner(), head=HEAD)
        self.assertTrue(any("source_sha" in p for p in problems))

    def test_a_matching_manifest_passes(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "manifest.json"
            path.write_text(json.dumps({"provenance": {"source_sha": HEAD}}))
            self.assertEqual(common.preflight("public", str(path), runner=runner(), head=HEAD), [])


if __name__ == "__main__":
    unittest.main()
