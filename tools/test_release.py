"""Distribution contract tests with dummy PE metadata, never a running Windows app."""

import hashlib
from pathlib import Path
import struct
import tempfile
import unittest
import zipfile

import release


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "assets").mkdir()
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "lingoleaf"\nversion = "1.2.3"\n', encoding="utf-8"
        )
        (self.root / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "lingoleaf"\nversion = "1.2.3"\n',
            encoding="utf-8",
        )
        (self.root / "assets/lingoleaf.rc").write_text(
            'FILEVERSION 1,2,3,0\nPRODUCTVERSION 1,2,3,0\n'
            'VALUE "FileVersion", "1.2.3.0\\0"\n'
            'VALUE "ProductVersion", "1.2.3\\0"\n',
            encoding="utf-8",
        )
        for name in release.REQUIRED_DOCUMENTS:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("## 1.2.3\n测试文档\n", encoding="utf-8")
        executable = bytearray(128)
        executable[:2] = b"MZ"
        struct.pack_into("<I", executable, 0x3C, 64)
        executable[64:68] = b"PE\0\0"
        struct.pack_into("<H", executable, 68, 0x8664)
        self.executable = self.root / "dummy.exe"
        self.executable.write_bytes(executable)

    def test_version_and_tag_match(self):
        self.assertEqual(release.project_version(self.root, "v1.2.3"), "1.2.3")
        with self.assertRaises(release.ReleaseError):
            release.project_version(self.root, "v1.2.4")

    def test_resource_versions_must_all_match(self):
        path = self.root / "assets/lingoleaf.rc"
        original = path.read_text(encoding="utf-8")
        for current, wrong in (
            ("FILEVERSION 1,2,3,0", "FILEVERSION 1,2,4,0"),
            ("PRODUCTVERSION 1,2,3,0", "PRODUCTVERSION 1,2,4,0"),
            ('"1.2.3.0\\0"', '"1.2.4.0\\0"'),
            ('"1.2.3\\0"', '"1.2.4\\0"'),
        ):
            with self.subTest(current=current):
                path.write_text(original.replace(current, wrong), encoding="utf-8")
                with self.assertRaises(release.ReleaseError):
                    release.project_version(self.root)

    def test_lockfile_and_changelog_are_required(self):
        lockfile = self.root / "Cargo.lock"
        original = lockfile.read_text(encoding="utf-8")
        lockfile.write_text(original.replace("1.2.3", "1.2.4"), encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            release.project_version(self.root)
        lockfile.write_text(original, encoding="utf-8")
        (self.root / "CHANGELOG.md").write_text("## 1.2.2\n", encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            release.project_version(self.root)

    def test_archive_is_deterministic_and_allowlisted(self):
        for name in ("settings.json", ".env", "cache.json", "ui_smoke.exe"):
            (self.root / name).write_text("must not ship", encoding="utf-8")
        first = self.root / "first"
        second = self.root / "second"
        release.package(self.root, self.executable, first)
        release.package(self.root, self.executable, second)
        self.assertEqual(
            {path.name: path.read_bytes() for path in first.iterdir()},
            {path.name: path.read_bytes() for path in second.iterdir()},
        )
        with zipfile.ZipFile(first / "lingoleaf-1.2.3-windows-x64.zip") as archive:
            self.assertEqual(
                set(archive.namelist()),
                {*release.REQUIRED_DOCUMENTS, "lingoleaf.exe", "开始使用.txt"},
            )
            self.assertEqual(archive.read("lingoleaf.exe"), self.executable.read_bytes())
            for item in archive.infolist():
                self.assertEqual(item.date_time, release.ZIP_TIMESTAMP)
                self.assertEqual(item.external_attr >> 16, 0o100644)
        for line in (first / "SHA256SUMS").read_text(encoding="ascii").splitlines():
            expected, name = line.split("  ")
            self.assertEqual(hashlib.sha256((first / name).read_bytes()).hexdigest(), expected)

    def test_optional_screenshot_preserves_relative_path(self):
        image = self.root / release.OPTIONAL_DOCUMENTS[0]
        image.parent.mkdir(parents=True)
        image.write_bytes(b"dummy image")
        output = self.root / "output"
        release.package(self.root, self.executable, output)
        with zipfile.ZipFile(output / "lingoleaf-1.2.3-windows-x64.zip") as archive:
            self.assertEqual(archive.read(release.OPTIONAL_DOCUMENTS[0]), b"dummy image")

    def test_invalid_executable_does_not_create_distribution(self):
        for payload in (b"not a PE", b"MZ" + bytes(126)):
            with self.subTest(payload=payload[:2]):
                self.executable.write_bytes(payload)
                output = self.root / "output"
                with self.assertRaises(release.ReleaseError):
                    release.package(self.root, self.executable, output)
                self.assertFalse(output.exists())

    def test_non_x64_executable_is_rejected(self):
        executable = bytearray(self.executable.read_bytes())
        struct.pack_into("<H", executable, 68, 0x14C)
        self.executable.write_bytes(executable)
        with self.assertRaises(release.ReleaseError):
            release.windows_executable(self.executable)


if __name__ == "__main__":
    unittest.main()
