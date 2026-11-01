import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "tools/recount-artifact.sh"
PIN = "1b7583649dad33cd56fb4ecbc952224781018698"


class RecountArtifactTests(unittest.TestCase):
    def git(self, directory, *args):
        return subprocess.check_output(["git", "-C", str(directory), *args],
                                       text=True, stderr=subprocess.PIPE).strip()

    def fixture(self, root):
        source = root / "source"
        source.mkdir()
        self.git(source, "init", "-q")
        self.git(source, "config", "user.name", "Fixture")
        self.git(source, "config", "user.email", "fixture@example.invalid")
        (source / "legacy").mkdir()
        (source / "legacy/input").write_bytes(b"abc")
        self.git(source, "add", ".")
        self.git(source, "commit", "-qm", "Historical input")
        historical = self.git(source, "rev-parse", "HEAD")
        shutil.rmtree(source / "legacy")
        (source / "Cargo.toml").write_text('[workspace]\nmembers = []\n')
        (source / "one.rs").write_text("fn main() {}\n")
        (source / "tools").mkdir()
        (source / "tools/recount-artifact.sh").write_text(SCRIPT.read_text().replace(PIN, historical))
        self.git(source, "add", "-A")
        self.git(source, "commit", "-qm", "Current tree")
        checkout = root / "checkout"
        subprocess.run(["git", "clone", "-q", "--depth=1", source.as_uri(), str(checkout)], check=True)
        return checkout, historical

    def run_recount(self, checkout):
        return subprocess.run(["bash", "tools/recount-artifact.sh", "--print", "--no-binary"],
                              cwd=checkout, capture_output=True, text=True, timeout=30,
                              env={**os.environ, "GIT_TERMINAL_PROMPT": "0"})

    def test_shallow_checkout_fetches_the_exact_pinned_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout, historical = self.fixture(Path(directory))
            self.assertNotEqual(subprocess.run(["git", "cat-file", "-e", historical],
                                              cwd=checkout, capture_output=True).returncode, 0)
            result = self.run_recount(checkout)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertRegex(result.stdout, r"legacy files\s+1")
            self.assertRegex(result.stdout, r"legacy bytes\s+3")

    def test_unavailable_tree_fails_before_emitting_zero_counts_or_a_record(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout, _ = self.fixture(Path(directory))
            self.git(checkout, "remote", "set-url", "origin", str(Path(directory) / "missing"))
            result = self.run_recount(checkout)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("cannot fetch legacy tree", result.stderr)
            self.assertNotIn("legacy files", result.stdout)
            self.assertFalse((checkout / "tools/artifact-counts.json").exists())

    def test_available_pin_needs_no_remote(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout, historical = self.fixture(Path(directory))
            self.git(checkout, "fetch", "--depth=1", "origin", historical)
            self.git(checkout, "remote", "remove", "origin")
            result = self.run_recount(checkout)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertRegex(result.stdout, r"legacy bytes\s+3")


if __name__ == "__main__":
    unittest.main()
