"""Collect installed dependency license notices into the distribution."""
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
out = root / "build/PackageStudio/licenses"
out.mkdir(parents=True, exist_ok=True)
go = sys.argv[1] if len(sys.argv) > 1 else "go"
modules = subprocess.check_output([go, "list", "-m", "-json", "all"], cwd=root, text=True)
decoder = json.JSONDecoder()
items = []
while modules.strip():
    item, end = decoder.raw_decode(modules.lstrip())
    modules = modules.lstrip()[end:]
    if not item.get("Main") and item.get("Dir"):
        items.append((item["Path"] + "@" + item.get("Version", ""), Path(item["Dir"])))
goroot = subprocess.check_output([go, "env", "GOROOT"], text=True).strip()
items.append(("Go standard library", Path(goroot)))
lock = json.loads((root / "frontend/package-lock.json").read_text())
for path, package in lock["packages"].items():
    if path and not package.get("dev"):
        items.append((path + "@" + package.get("version", ""), root / "frontend" / path))
records = []
for name, folder in items:
    prefix = re.sub(r"[^a-zA-Z0-9._-]", "_", name)
    found = []
    for file in folder.iterdir():
        if file.is_file() and file.name.upper().split(".")[0] in {"LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT"}:
            target = out / (prefix + "-" + file.name)
            shutil.copyfile(file, target)
            found.append("licenses/" + target.name)
    if not found:
        raise RuntimeError("No dependency license found: " + name)
    records.append(name + "\n  " + "\n  ".join(found))
(out.parent / "THIRD-PARTY-NOTICES.txt").write_text("Package Studio dependency notices\n\n" + "\n\n".join(records) + "\n\nAltium adapter notices: adapters/altium/THIRD-PARTY-NOTICES.txt\n", encoding="utf-8")
print("Collected dependency notices:", len(records))
