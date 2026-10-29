"""Unit tests for datasets.sh: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "datasets.sh"


def datasets(root, configs):
    for name, text in configs.items():
        (root / "configs" / name).write_text(text)
    out = subprocess.run([str(SCRIPT), "configs"], cwd=root, check=True, capture_output=True, text=True)
    return out.stdout.split()


class Datasets(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "configs").mkdir()
        (self.root / "data").mkdir()
        (self.root / "data" / "a.mp4").touch()

    def tearDown(self):
        self._tmp.cleanup()

    def test_lists_configs_whose_input_exists(self):
        got = datasets(self.root, {
            "a.toml": '[dataset]\nname = "a"\ninput = "data/a.mp4"\n',
            "b.toml": '[dataset]\nname = "b"\ninput = "data/missing.mp4"\n',
        })
        self.assertEqual(got, ["a"])

    def test_skips_files_without_a_dataset_table(self):
        got = datasets(self.root, {
            "defaults.toml": '[frames]\nfps = 2.0\n',
            "host.local.toml": '[colmap]\nruntime = "native"\nprogram = "colmap"\n',
        })
        self.assertEqual(got, [])

    def test_accepts_spaced_headers_and_comments(self):
        got = datasets(self.root, {
            "a.toml": '  [ dataset ]  # the clip\nname = "a"\n  input = "data/a.mp4"  # raw\n',
        })
        self.assertEqual(got, ["a"])

    def test_empty_directory_prints_nothing_and_succeeds(self):
        self.assertEqual(datasets(self.root, {}), [])


if __name__ == "__main__":
    unittest.main()
