# Build step: bake the OneFormer weights and the processor's class metadata into
# one directory, so the image runs offline (ADR-0006).
#
#   python fetch_model.py <model> <revision> <class-info dataset> <dataset revision> <out dir>
#
# The processor reads <repo_path>/<class_info_file> when that file exists
# locally, and otherwise downloads it from a separate dataset repo
# (shi-labs/oneformer_demo); repo_path is pointed at <out dir> here.
import json
import shutil
import sys
from pathlib import Path

from huggingface_hub import hf_hub_download, snapshot_download

model, revision, class_info_repo, class_info_revision, out = sys.argv[1:6]
out = Path(out)
snapshot_download(
    model,
    revision=revision,
    local_dir=out,
    allow_patterns=["*.json", "merges.txt", "vocab.json", "pytorch_model.bin"],
)
config_path = out / "preprocessor_config.json"
config = json.loads(config_path.read_text())
name = config["class_info_file"]
fetched = hf_hub_download(class_info_repo, name, repo_type="dataset", revision=class_info_revision)
shutil.copy(fetched, out / name)
config["repo_path"] = str(out)
config_path.write_text(json.dumps(config, indent=2) + "\n")
shutil.rmtree(out / ".cache", ignore_errors=True)
print(f"baked {model}@{revision} and {class_info_repo}@{class_info_revision}:{name} into {out}")
