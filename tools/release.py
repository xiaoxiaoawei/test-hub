#!/usr/bin/env python3
"""Validate LingoLeaf versions and package a Windows x64 build (Python 3.12+)."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
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
    "docs/images/settings-dark-0.10.0.png",
    "docs/images/markdown-0.10.0.png",
    "docs/images/appearance-0.10.0.png",
    "docs/images/settings-200pct-0.10.0.png",
    "docs/design-0.10.0.md",
)

ZIP_TIMESTAMP = (1980, 1, 1, 0, 0, 0)
UPDATE_REPOSITORY = "https://github.com/xiaoxiaoawei/test-hub"
PRIMARY_EXECUTABLE = "lingoleaf.exe"
PRIMARY_ARCHIVE = "lingoleaf-windows-x64.zip"
MAX_EXECUTABLE_BYTES = 32 * 1024 * 1024
UPDATE_FIELDS = {"schema_version", "version", "download_url", "sha256", "size", "notes_url"}


class ReleaseError(ValueError):
    """An input does not satisfy the distribution contract."""


def stable_version(value: object) -> str:
    """Use the same stable, Windows-version-compatible form as the updater."""
    if not isinstance(value, str) or not re.fullmatch(
        r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", value
    ):
        raise ReleaseError("Version must use stable major.minor.patch")
    if any(int(part) > 65535 for part in value.split(".")):
        raise ReleaseError("Windows version components must not exceed 65535")
    return value


def project_version(root: Path, tag: str | None = None) -> str:
    """Check the manifest, lockfile, Windows resources, changelog, and optional tag."""
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    version = stable_version(manifest["package"]["version"])

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
    """Reject oversized, DLL, console, and non-x64 PE inputs without executing them."""
    if not 64 <= path.stat().st_size <= MAX_EXECUTABLE_BYTES:
        raise ReleaseError("--exe must be between 64 bytes and 32 MiB")
    executable = path.read_bytes()
    if not 64 <= len(executable) <= MAX_EXECUTABLE_BYTES or executable[:2] != b"MZ":
        raise ReleaseError("--exe must be a Windows x64 PE executable")
    pe_offset = struct.unpack_from("<I", executable, 0x3C)[0]
    if (
        pe_offset < 64
        or pe_offset + 24 > len(executable)
        or executable[pe_offset : pe_offset + 4] != b"PE\0\0"
        or struct.unpack_from("<H", executable, pe_offset + 4)[0] != 0x8664
    ):
        raise ReleaseError("--exe must be a Windows x64 PE executable")
    optional = pe_offset + 24
    optional_size = struct.unpack_from("<H", executable, pe_offset + 20)[0]
    characteristics = struct.unpack_from("<H", executable, pe_offset + 22)[0]
    if (
        optional_size < 112
        or optional + optional_size > len(executable)
        or struct.unpack_from("<H", executable, optional)[0] != 0x20B
        or struct.unpack_from("<H", executable, optional + 68)[0] != 2
        or characteristics & 0x2000
        or not characteristics & 0x0002
    ):
        raise ReleaseError("--exe must be a PE32+ Windows GUI executable, not a DLL")
    return executable


def validate_update_manifest(value: object) -> dict:
    """Require one version and exact same-repository URLs; no arbitrary download host."""
    if not isinstance(value, dict) or set(value) != UPDATE_FIELDS:
        raise ReleaseError("Update metadata has missing or unexpected fields")
    if type(value["schema_version"]) is not int or value["schema_version"] != 1:
        raise ReleaseError("Unsupported update metadata schema")
    version = stable_version(value["version"])
    asset = f"lingoleaf-{version}-windows-x64.exe"
    allowed_urls = {
        f"{UPDATE_REPOSITORY}/releases/download/v{version}/{asset}",
        f"{UPDATE_REPOSITORY}/releases/download/v{version}/{PRIMARY_EXECUTABLE}",
    }
    if not isinstance(value["download_url"], str) or value["download_url"] not in allowed_urls:
        raise ReleaseError("Update download URL must match this repository, version and EXE")
    if value["notes_url"] != f"{UPDATE_REPOSITORY}/releases/tag/v{version}":
        raise ReleaseError("Update notes URL must match this repository and version")
    if not isinstance(value["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", value["sha256"]):
        raise ReleaseError("Update SHA-256 must contain 64 lowercase hexadecimal characters")
    if type(value["size"]) is not int or not 64 <= value["size"] <= MAX_EXECUTABLE_BYTES:
        raise ReleaseError("Update executable size must be between 64 bytes and 32 MiB")
    return value


def update_manifest(version: str, executable: bytes) -> bytes:
    """Keep the original six-field URL contract for installed 0.5/0.6 clients.

    The primary user download is lingoleaf.exe. The feed deliberately refers to
    its byte-identical compatibility alias because older clients reject any
    other asset filename before downloading.
    """
    version = stable_version(version)
    metadata = validate_update_manifest({
        "schema_version": 1,
        "version": version,
        "download_url": (
            f"{UPDATE_REPOSITORY}/releases/download/v{version}/"
            f"lingoleaf-{version}-windows-x64.exe"
        ),
        "sha256": hashlib.sha256(executable).hexdigest(),
        "size": len(executable),
        "notes_url": f"{UPDATE_REPOSITORY}/releases/tag/v{version}",
    })
    return (json.dumps(metadata, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8")


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

    compatibility_asset = f"lingoleaf-{version}-windows-x64.exe"
    payloads = {
        PRIMARY_EXECUTABLE: executable,
        PRIMARY_ARCHIVE: archive.getvalue(),
        compatibility_asset: executable,
        "update.json": update_manifest(version, executable),
    }
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
