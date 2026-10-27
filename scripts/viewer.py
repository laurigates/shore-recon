"""Local web viewer for the export deliverables in work/ (ADR-0015).

Serves scripts/viewer/index.html, an index of every dataset's export outputs at /index.json
(rebuilt on each request, so new runs show up on reload), and the files under work/ at /work/.
The page draws the splat and the mesh in the mesh's model frame, with the raw splat and raw
mesh of the same run beside them.

    viewer.py [--work DIR] [--port N] [--open]

Standard library only; binds to 127.0.0.1.
"""
import argparse
import http.server
import json
import mimetypes
import posixpath
import sys
import urllib.parse
import webbrowser
from pathlib import Path

HERE = Path(__file__).resolve().parent
PAGE_DIR = HERE / "viewer"

# Layer name -> (stage directory, file), for the export output and the raw stage outputs the
# export's report.json points at.
EXPORT_LAYERS = {"splat": "splat.ply", "mesh": "model.glb"}
RAW_LAYERS = {"splat_raw": ("splat", "splat.ply"), "mesh_raw": ("dense", "mesh_textured.glb")}
SHORT = 16  # cache directories are named by the first 16 hex digits of the fingerprint

mimetypes.add_type("model/gltf-binary", ".glb")
mimetypes.add_type("application/octet-stream", ".ply")
mimetypes.add_type("application/octet-stream", ".spz")


def url_of(work, path):
    return "/work/" + urllib.parse.quote(path.relative_to(work).as_posix())


def read_report(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def describe_export(work, dataset, out):
    """One export output directory: its layers, videos and metadata, as URLs under /work/."""
    files = {p.name: p for p in out.iterdir() if p.is_file() and not p.name.startswith(".")}
    sizes = {}

    def add(path):
        if path is None or not path.is_file():
            return None
        url = url_of(work, path)
        sizes[url] = path.stat().st_size
        return url

    layers = {name: add(files.get(f)) for name, f in EXPORT_LAYERS.items()}
    report_path = files.get("report.json")
    report = read_report(report_path) if report_path else None
    stages = {s.get("stage"): s for s in (report or {}).get("stages", []) if isinstance(s, dict)}
    for name, (stage, f) in RAW_LAYERS.items():
        fp = str(stages.get(stage, {}).get("fingerprint", ""))[:SHORT]
        layers[name] = add(work / dataset / stage / fp / f) if fp else None
    post = stages.get("post", {}).get("metrics", {})
    return {
        "id": out.name,
        "modified": max((p.stat().st_mtime for p in [out, *files.values()]), default=0.0),
        "layers": layers,
        "water_plane": add(files.get("water_plane.json")),
        "report": add(report_path),
        "videos": [
            {"name": n[:-4], "url": add(files[n])}
            for n in sorted(files)
            if n.endswith(".mp4") and not n.endswith("-share.mp4")
        ],
        "metrics": {k: post[k] for k in ("gaussians", "mesh_faces") if k in post},
        "sizes": sizes,
    }


def build_index(work):
    """Every dataset under `work` with an export stage, its outputs newest first."""
    work = Path(work)
    datasets = []
    for ds in sorted(p for p in work.iterdir() if p.is_dir()) if work.is_dir() else []:
        export = ds / "export"
        if not export.is_dir():
            continue
        outs = [describe_export(work, ds.name, d) for d in export.iterdir() if d.is_dir()]
        outs = [o for o in outs if any(o["layers"].values()) or o["videos"]]
        if outs:
            outs.sort(key=lambda o: (-o["modified"], o["id"]))
            datasets.append({"name": ds.name, "exports": outs})
    return {"work": str(work), "datasets": datasets}


def resolve(root, url_path):
    """The file `url_path` names under `root`, or None if it leaves `root` or is hidden."""
    parts = [p for p in posixpath.normpath(urllib.parse.unquote(url_path)).split("/") if p]
    if any(p in ("..", ".") or p.startswith(".") for p in parts):
        return None
    path = Path(root, *parts).resolve()
    root = Path(root).resolve()
    if path != root and root not in path.parents:
        return None
    return path


class Handler(http.server.SimpleHTTPRequestHandler):
    work = Path("work")

    def translate_path(self, path):
        url_path = urllib.parse.urlsplit(path).path
        if url_path.startswith("/work/"):
            found = resolve(self.work, url_path[len("/work/"):])
        else:
            found = resolve(PAGE_DIR, url_path)
        # "" fails to open, so the base class answers 404 for a path outside the roots.
        return str(found) if found else ""

    def list_directory(self, path):
        self.send_error(404)
        return None

    def do_GET(self):
        if urllib.parse.urlsplit(self.path).path == "/index.json":
            body = json.dumps(build_index(self.work)).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()

    def log_request(self, code="-", size="-"):
        if not str(int(code) if isinstance(code, int) else code).startswith(("2", "3")):
            super().log_request(code, size)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--work", default="work", help="work root (default: work)")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--open", action="store_true", help="open the page in the browser")
    args = ap.parse_args(argv)
    Handler.work = Path(args.work).resolve()
    index = build_index(Handler.work)
    for ds in index["datasets"]:
        print(f"{ds['name']}: {len(ds['exports'])} export output(s), latest {ds['exports'][0]['id']}")
    if not index["datasets"]:
        print(f"no export outputs under {Handler.work} yet; the page lists them once a run exports")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    url = f"http://127.0.0.1:{server.server_address[1]}/"
    print(f"serving {url} (Ctrl-C stops)")
    if args.open:
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
