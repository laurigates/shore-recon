"""Unit tests for viewer.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import json
import os
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path

import viewer

FP_SPLAT = "a" * 64
FP_DENSE = "b" * 64


def touch(path, data=b"x"):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


def export(work, ds, fp, mtime, report=True):
    out = work / ds / "export" / fp
    for f in ["model.glb", "splat.ply", "water_plane.json", "splat-arc.mp4", "splat-arc-share.mp4"]:
        touch(out / f)
    if report:
        stages = [
            {"stage": "splat", "fingerprint": FP_SPLAT, "metrics": {}},
            {"stage": "dense", "fingerprint": FP_DENSE, "metrics": {}},
            {"stage": "post", "fingerprint": "c" * 64, "metrics": {"gaussians": 7.0, "other": 1.0}},
        ]
        (out / "report.json").write_text(json.dumps({"stages": stages}))
    for p in [*out.iterdir(), out]:
        os.utime(p, (mtime, mtime))
    return out


class BuildIndex(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.work = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_missing_or_empty_work_has_no_datasets(self):
        self.assertEqual(viewer.build_index(self.work / "nope")["datasets"], [])
        self.assertEqual(viewer.build_index(self.work)["datasets"], [])

    def test_lists_datasets_with_exports_newest_first(self):
        export(self.work, "b-site", "1111", 100)
        export(self.work, "a-site", "old", 100)
        export(self.work, "a-site", "new", 200)
        touch(self.work / "c-site" / "frames" / "x" / "images" / "00001.jpg")
        index = viewer.build_index(self.work)
        self.assertEqual([d["name"] for d in index["datasets"]], ["a-site", "b-site"])
        self.assertEqual([e["id"] for e in index["datasets"][0]["exports"]], ["new", "old"])

    def test_describes_layers_videos_and_metrics(self):
        export(self.work, "site", "e1", 100)
        raw_splat = touch(self.work / "site" / "splat" / FP_SPLAT[:16] / "splat.ply", b"12345")
        e = viewer.build_index(self.work)["datasets"][0]["exports"][0]
        self.assertEqual(e["layers"]["splat"], "/work/site/export/e1/splat.ply")
        self.assertEqual(e["layers"]["mesh"], "/work/site/export/e1/model.glb")
        self.assertEqual(e["layers"]["splat_raw"], "/work/site/splat/aaaaaaaaaaaaaaaa/splat.ply")
        self.assertIsNone(e["layers"]["mesh_raw"])  # dense output not present
        self.assertEqual(e["water_plane"], "/work/site/export/e1/water_plane.json")
        self.assertEqual(e["videos"], [{"name": "splat-arc", "url": "/work/site/export/e1/splat-arc.mp4"}])
        self.assertEqual(e["metrics"], {"gaussians": 7.0})
        self.assertEqual(e["sizes"][e["layers"]["splat_raw"]], raw_splat.stat().st_size)

    def test_export_without_report_has_no_raw_layers(self):
        export(self.work, "site", "e1", 100, report=False)
        e = viewer.build_index(self.work)["datasets"][0]["exports"][0]
        self.assertIsNone(e["layers"]["splat_raw"])
        self.assertIsNone(e["report"])
        self.assertEqual(e["metrics"], {})

    def test_skips_export_directories_without_deliverables(self):
        (self.work / "site" / "export" / "partial").mkdir(parents=True)
        self.assertEqual(viewer.build_index(self.work)["datasets"], [])

    def test_urls_are_quoted(self):
        export(self.work, "my site", "e1", 100)
        e = viewer.build_index(self.work)["datasets"][0]["exports"][0]
        self.assertEqual(e["layers"]["mesh"], "/work/my%20site/export/e1/model.glb")


class Resolve(unittest.TestCase):
    def test_stays_inside_root(self):
        with tempfile.TemporaryDirectory() as root:
            self.assertEqual(viewer.resolve(root, "a/b.ply"), Path(root).resolve() / "a" / "b.ply")
            self.assertEqual(viewer.resolve(root, "my%20site/x"), Path(root).resolve() / "my site" / "x")
            for bad in ["../x", "a/../../x", "%2e%2e/x", ".env", "a/.git/config"]:
                self.assertIsNone(viewer.resolve(root, bad), bad)

    def test_symlink_out_of_root_is_refused(self):
        with tempfile.TemporaryDirectory() as root, tempfile.TemporaryDirectory() as other:
            touch(Path(other) / "secret")
            (Path(root) / "link").symlink_to(other)
            self.assertIsNone(viewer.resolve(root, "link/secret"))


class QuietHandler(viewer.Handler):
    def log_message(self, fmt, *args):
        pass


class Serve(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        work = Path(self.tmp.name)
        export(work, "site", "e1", 100)
        QuietHandler.work = work
        self.server = viewer.http.server.ThreadingHTTPServer(("127.0.0.1", 0), QuietHandler)
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.tmp.cleanup()

    def get(self, path):
        with urllib.request.urlopen(self.base + path) as r:
            return r.status, r.read()

    def status(self, path):
        try:
            return self.get(path)[0]
        except urllib.error.HTTPError as e:
            return e.code

    def test_serves_page_index_and_work_files(self):
        status, body = self.get("/")
        self.assertEqual(status, 200)
        self.assertIn(b"shoreline-recon", body)
        index = json.loads(self.get("/index.json")[1])
        url = index["datasets"][0]["exports"][0]["layers"]["mesh"]
        self.assertEqual(self.get(url), (200, b"x"))

    def test_refuses_listings_and_paths_outside_work(self):
        for path in ["/work/", "/work/site/", "/work/../viewer.py", "/work/%2e%2e/viewer.py", "/nope"]:
            self.assertEqual(self.status(path), 404, path)


if __name__ == "__main__":
    unittest.main()
