#!/usr/bin/env python3
"""Validate LingoLeaf versions and package a Windows x64 build (Python 3.12+)."""

from __future__ import annotations

import argparse
import hashlib
import io
from pathlib import Path
import re
import struct
import sys
import tomllib
import zipfile


REPOSITORY = Path(__file__).resolve().parents[1]
REQUIRED_DOCUMENTS = (
    "README.md",
    "CHANGELOG.md",
    "docs/releasing.md",
    "docs/build-validation.md",
    "docs/windows-validation.md",
)
OPTIONAL_DOCUMENTS = (
    "docs/images/settings-warm-dark.png",
    "docs/images/appearance.png",
    "docs/images/streaming.png",
    "docs/images/cache.png",
    "docs/images/inline.png",
    "docs/images/selection-icon.png",
)
ZIP_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


class ReleaseError(ValueError):
    """An input does not satisfy the distribution contract."""


def project_version(root: Path, tag: str | None = None) -> str:
    """Check the manifest, lockfile, Windows resources, changelog, and optional tag."""
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    version = manifest["package"]["version"]
    if not isinstance(version, str) or not re.fullmatch(
        r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", version
    ):
        raise ReleaseError("Cargo.toml must use a stable major.minor.patch version")
    if any(int(part) > 65535 for part in version.split(".")):
        raise ReleaseError("Windows version components must not exceed 65535")

    lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
    own_versions = [
        item["version"]
        for item in lock.get("package", [])
        if item.get("name") == manifest["package"]["name"] and "source" not in item
    ]
    if own_versions != [version]:
        raise ReleaseError("Cargo.lock package version does not match Cargo.toml")

    resource = (root / "assets/lingoleaf.rc").read_text(encoding="utf-8")
    numeric = [int(part) for part in version.split(".")] + [0]
    for field in ("FILEVERSION", "PRODUCTVERSION"):
        found = re.findall(rf"^\s*{field}\s+([\d, \t]+)\s*$", resource, re.MULTILINE)
        if len(found) != 1 or [int(part.strip()) for part in found[0].split(",")] != numeric:
            raise ReleaseError(f"assets/lingoleaf.rc {field} does not match {version}")
    for field, expected in (("FileVersion", f"{version}.0"), ("ProductVersion", version)):
        found = re.findall(rf'VALUE\s+"{field}"\s*,\s*"([^"\r\n]+)"', resource)
        if len(found) != 1 or found[0].removesuffix(r"\0") != expected:
            raise ReleaseError(f"assets/lingoleaf.rc {field} does not match {version}")

    changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    if not re.search(rf"^## {re.escape(version)}(?:\s|$)", changelog, re.MULTILINE):
        raise ReleaseError(f"CHANGELOG.md is missing the {version} section")
    if tag is not None and tag != f"v{version}":
        raise ReleaseError(f"Tag {tag!r} must match v{version}")
    return version


def windows_executable(path: Path) -> bytes:
    """Reject missing files and non-x64 PE inputs; this does not execute the program."""
    executable = path.read_bytes()
    if len(executable) < 64 or executable[:2] != b"MZ":
        raise ReleaseError("--exe must be a Windows x64 PE executable")
    pe_offset = struct.unpack_from("<I", executable, 0x3C)[0]
    if (
        pe_offset < 64
        or pe_offset + 24 > len(executable)
        or executable[pe_offset : pe_offset + 4] != b"PE\0\0"
        or struct.unpack_from("<H", executable, pe_offset + 4)[0] != 0x8664
    ):
        raise ReleaseError("--exe must be a Windows x64 PE executable")
    return executable


def document_bytes(root: Path, relative: str) -> bytes:
    path = root / relative
    if not path.resolve().is_relative_to(root.resolve()):
        raise ReleaseError(f"Distribution document escapes the repository: {relative}")
    return path.read_bytes()


def package(root: Path, executable_path: Path, out_dir: Path, tag: str | None = None) -> list[Path]:
    """Write a deterministic, explicit allowlist of runtime files and public documents."""
    version = project_version(root, tag)
    executable = windows_executable(executable_path)
    entries = {name: document_bytes(root, name) for name in REQUIRED_DOCUMENTS}
    for name in OPTIONAL_DOCUMENTS:
        if (root / name).is_file():
            entries[name] = document_bytes(root, name)
    entries["lingoleaf.exe"] = executable
    entries["开始使用.txt"] = (
        f"轻译 LingoLeaf {version}\r\n\r\n"
        "更新前请从系统托盘退出旧版本，再解压并启动 lingoleaf.exe。\r\n"
        "设置保存在当前用户的 %LOCALAPPDATA%\\LingoLeaf，更新不需要复制配置。\r\n"
        "首次运行请填写自己的翻译 API；详细说明见 README.md。\r\n"
        "本包未签名，Windows 实机验证状态见 docs/build-validation.md。\r\n"
    ).encode("utf-8-sig")

    archive = io.BytesIO()
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
        for name, content in sorted(entries.items()):
            info = zipfile.ZipInfo(name, date_time=ZIP_TIMESTAMP)
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            bundle.writestr(info, content, compresslevel=9)

    stem = f"lingoleaf-{version}-windows-x64"
    payloads = {f"{stem}.exe": executable, f"{stem}.zip": archive.getvalue()}
    checksums = "".join(
        f"{hashlib.sha256(content).hexdigest()}  {name}\n"
        for name, content in sorted(payloads.items())
    ).encode("ascii")
    payloads["SHA256SUMS"] = checksums
    out_dir.mkdir(parents=True, exist_ok=True)
    output = []
    for name, content in sorted(payloads.items()):
        path = out_dir / name
        path.write_bytes(content)
        output.append(path)
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Validate versions without writing files")
    parser.add_argument("--tag", help="Require an exact matching vX.Y.Z tag")
    parser.add_argument("--exe", type=Path, help="Path to the built Windows x64 executable")
    parser.add_argument("--out-dir", type=Path, default=Path("dist"), help="Output directory (default: dist)")
    args = parser.parse_args()
    if not args.check and args.exe is None:
        parser.error("--exe is required unless --check is used")
    try:
        if args.check:
            version = project_version(REPOSITORY, args.tag)
            if args.exe is not None:
                windows_executable(args.exe)
            print(f"Version metadata OK: {version}")
        else:
            for path in package(REPOSITORY, args.exe, args.out_dir, args.tag):
                print(path)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Release validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
