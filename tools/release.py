"""Package Studio release gate. Uses only the Python standard library."""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import re
import struct
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
REPO = "https://github.com/xiaoxiaoawei/test-hub"
ARCHIVE = "package-studio-windows-x64.zip"


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def version(root=ROOT):
    value = (root / "VERSION").read_text().strip()
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", value):
        raise ValueError("invalid VERSION")
    if any(int(x) > 65535 for x in value.split(".")):
        raise ValueError("version component exceeds Windows limit")
    return value


def notes(root=ROOT):
    v = version(root)
    text = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    match = re.search(r"^## " + re.escape(v) + r"\s*\n(.*?)(?=^## |\Z)", text, re.M | re.S)
    if not match or not match[1].strip():
        raise ValueError("missing current CHANGELOG entry")
    return match[1].strip()


def check(root=ROOT, tag=None):
    v = version(root)
    expected = "package-studio-v" + v
    if tag and tag != expected:
        raise ValueError(f"tag {tag} does not match {expected}")
    if read_json(root / "wails.json")["info"]["productVersion"] != v:
        raise ValueError("wails version mismatch")
    for file in ("frontend/package.json", "frontend/package-lock.json"):
        package = read_json(root / file)
        if package["version"] != v or ("packages" in package and package["packages"][""]["version"] != v):
            raise ValueError(file + " version mismatch")
    source = (root / "internal/service/service.go").read_text(encoding="utf-8")
    if not re.search(r'var Version = "' + re.escape(v) + '"', source):
        raise ValueError("Go service version mismatch")
    resource = read_json(root / "winres/winres.json")["RT_VERSION"]["#1"]["0000"]
    if resource["fixed"]["file_version"] != v + ".0" or resource["fixed"]["product_version"] != v + ".0":
        raise ValueError("PE fixed version mismatch")
    if resource["info"]["0409"]["ProductVersion"] != v:
        raise ValueError("PE string version mismatch")
    notes(root)
    return v


def validate_pe(data):
    if len(data) < 256 or data[:2] != b"MZ":
        raise ValueError("not a Windows executable")
    pos = struct.unpack_from("<I", data, 0x3C)[0]
    if pos < 64 or pos + 100 > len(data) or data[pos:pos + 4] != b"PE\0\0":
        raise ValueError("invalid PE header")
    machine = struct.unpack_from("<H", data, pos + 4)[0]
    flags = struct.unpack_from("<H", data, pos + 22)[0]
    magic = struct.unpack_from("<H", data, pos + 24)[0]
    subsystem = struct.unpack_from("<H", data, pos + 24 + 68)[0]
    if (machine, magic, subsystem) != (0x8664, 0x20B, 2) or flags & 0x2000:
        raise ValueError("requires AMD64 PE32+ GUI executable")


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def package(app, out, root=ROOT, tag=None):
    v = check(root, tag)
    app, out = Path(app), Path(out)
    validate_pe((app / "PackageStudio.exe").read_bytes())
    # Release packages include the optional small AD adapter, never its SDK/runtime.
    if not (app / "adapters/altium/PackageStudio.Altium.dll").is_file():
        raise ValueError("release requires Altium adapter")
    (app / "VERSION").write_text(v + "\n", encoding="utf-8")
    (app / "README.md").write_text((root / "README.md").read_text(encoding="utf-8"), encoding="utf-8")
    out.mkdir(parents=True, exist_ok=True)
    archive = out / ARCHIVE
    files = sorted(x for x in app.rglob("*") if x.is_file())
    if any(x.is_symlink() or x.suffix.lower() in {".pdb", ".tmp"} for x in files):
        raise ValueError("unexpected symlink/debug/temporary file in release")
    if sum(x.stat().st_size for x in files) > 256 * 1024 * 1024:
        raise ValueError("uncompressed package too large")
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for path in files:
            name = "PackageStudio/" + path.relative_to(app).as_posix()
            entry = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = 0o100644 << 16
            z.writestr(entry, path.read_bytes())
    size = archive.stat().st_size
    if not 512 <= size <= 128 * 1024 * 1024:
        raise ValueError("archive size outside updater limits")
    manifest = dict(schema_version=1, product="package-studio", version=v,
        download_url=f"{REPO}/releases/download/package-studio-v{v}/{ARCHIVE}",
        notes_url=f"{REPO}/releases/tag/package-studio-v{v}", sha256=digest(archive), size=size,
        published_at=dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z"),
        release_notes=notes(root))
    (out / "update.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    (out / "release-notes.md").write_text(notes(root), encoding="utf-8")
    (out / "SHA256SUMS.txt").write_text("".join(f"{digest(out / f)}  {f}\n" for f in (ARCHIVE, "update.json")), encoding="utf-8")
    verify(out, root)
    return manifest


def verify(out, root=ROOT):
    out = Path(out)
    m = read_json(out / "update.json")
    v = check(root)
    if m["product"] != "package-studio" or m["schema_version"] != 1 or m["version"] != v:
        raise ValueError("manifest version/product mismatch")
    if m["download_url"] != f"{REPO}/releases/download/package-studio-v{v}/{ARCHIVE}" or m["notes_url"] != f"{REPO}/releases/tag/package-studio-v{v}":
        raise ValueError("manifest repository/tag mismatch")
    archive = out / ARCHIVE
    if archive.stat().st_size != m["size"] or digest(archive) != m["sha256"]:
        raise ValueError("archive hash/size mismatch")
    with zipfile.ZipFile(archive) as z:
        if z.testzip():
            raise ValueError("corrupt ZIP")
        names = z.namelist()
        if len({n.casefold() for n in names}) != len(names) or len(names) > 600:
            raise ValueError("duplicate/too many ZIP members")
        for n in names:
            if not n.startswith("PackageStudio/") or "\\" in n or ":" in n or any(p in {"", ".", ".."} for p in n.split("/")):
                raise ValueError("unsafe ZIP member")
        if z.read("PackageStudio/VERSION").decode().strip() != v:
            raise ValueError("packaged VERSION mismatch")
        validate_pe(z.read("PackageStudio/PackageStudio.exe"))
    for line in (out / "SHA256SUMS.txt").read_text().splitlines():
        hash_value, name = line.split("  ")
        if name not in {ARCHIVE, "update.json"} or digest(out / name) != hash_value:
            raise ValueError("checksum manifest mismatch")
    return m


def compare_assets(expected, published, root=ROOT):
    """Allow a release retry only when its immutable remote assets are identical."""
    expected, published = Path(expected), Path(published)
    verify(expected, root)
    manifest = verify(published, root)
    for name in (ARCHIVE, "update.json", "SHA256SUMS.txt"):
        local, remote = expected / name, published / name
        if local.stat().st_size != remote.stat().st_size or digest(local) != digest(remote):
            raise ValueError("immutable release asset mismatch: " + name)
    return manifest


def promote(manifest, root=ROOT, source_root=None):
    """Write only the product feed in root; validate versions against tag source."""
    root = Path(root)
    source_root = root if source_root is None else Path(source_root)
    m = read_json(manifest)
    if m["version"] != check(source_root) or m["product"] != "package-studio":
        raise ValueError("feed version/product mismatch")
    target = root / "updates/package-studio/stable.json"
    if target.exists():
        previous = read_json(target)
        old, new = tuple(map(int, previous["version"].split("."))), tuple(map(int, m["version"].split(".")))
        if new < old or (new == old and previous != m):
            raise ValueError("stable feed downgrade or immutable version conflict")
        if previous == m:
            return False
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(m, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return True


def main():
    p = argparse.ArgumentParser()
    p.add_argument("command", choices=["check", "package", "verify", "compare-assets", "promote"])
    p.add_argument("--tag")
    p.add_argument("--app", default="build/PackageStudio")
    p.add_argument("--out", default="dist")
    p.add_argument("--manifest", default="dist/update.json")
    p.add_argument("--other", help="Published asset directory for compare-assets")
    p.add_argument("--root", type=Path, help="Destination feed checkout for promote; source validation uses this script checkout")
    args = p.parse_args()
    if args.command == "check":
        print(check(tag=args.tag))
    elif args.command == "package":
        print(json.dumps(package(args.app, args.out, tag=args.tag), ensure_ascii=False))
    elif args.command == "verify":
        print("verified", verify(args.out)["version"])
    elif args.command == "compare-assets":
        if not args.other:
            raise ValueError("compare-assets requires --other")
        print("immutable assets verified", compare_assets(args.out, args.other)["version"])
    else:
        changed = promote(args.manifest, root=args.root or ROOT, source_root=ROOT)
        print("stable feed promoted" if changed else "stable feed already current")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, zipfile.BadZipFile) as error:
        sys.exit(str(error))
