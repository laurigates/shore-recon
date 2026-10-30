import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import clean_remote


class TestRemoteConfigs(unittest.TestCase):
    def test_finds_remote(self):
        tools = {
            "colmap": {"runtime": "apptainer", "remote": {"host": "h", "root": "/r"}},
            "ffmpeg": {"runtime": "native"},
        }
        rc = clean_remote.remote_configs(tools)
        self.assertEqual(len(rc), 1)
        self.assertEqual(rc[0]["host"], "h")


class TestReferencedDigests(unittest.TestCase):
    def test_extracts(self):
        tools = {"x": {"image": "img@sha256:abc123"}}
        self.assertEqual(clean_remote.referenced_digests(tools), {"sha256:abc123"})


class TestRelToLocal(unittest.TestCase):
    def test_maps(self):
        self.assertEqual(clean_remote.rel_to_local("/r/Users/x/work", "/r"), Path("/Users/x/work"))
    def test_outside(self):
        self.assertIsNone(clean_remote.rel_to_local("/other/x", "/r"))


class TestTopmostMissing(unittest.TestCase):
    def test_filters_under_work_and_missing(self):
        with tempfile.TemporaryDirectory() as td:
            work = Path(td) / "work"
            work.mkdir()
            (work / "keep").mkdir()
            remote_dirs = [
                "/r" + str(work / "keep"),          # exists -> not candidate
                "/r" + str(work / "gone"),           # missing -> candidate
                "/r" + str(work / "gone" / "sub"),   # child of candidate -> pruned
                "/r/etc/passwd",                     # not under work -> ignored
            ]
            res = clean_remote.topmost_missing(remote_dirs, "/r", work)
            self.assertEqual(res, ["/r" + str(work / "gone")])


class TestPruneCacheCandidates(unittest.TestCase):
    def test_filters_by_digest(self):
        files = ["/c/a.sif", "/c/b.sif"]
        digests = {"sha256:deadbeef"}
        # neither references digest -> both candidates (heuristic)
        res = clean_remote.prune_cache_candidates(files, digests)
        self.assertEqual(res, files)

    def test_retains_referenced_digest(self):
        files = ["/c/deadbeef1234.sif", "/c/other.sif"]
        digests = {"sha256:deadbeef"}
        # the entry whose name contains the referenced digest is kept;
        # the other is a candidate.
        res = clean_remote.prune_cache_candidates(files, digests)
        self.assertEqual(res, ["/c/other.sif"])
        # inversion check: the retained entry must not appear in candidates
        self.assertNotIn("/c/deadbeef1234.sif", res)


if __name__ == "__main__":
    unittest.main()
