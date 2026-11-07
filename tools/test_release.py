"""Distribution contract tests with dummy PE metadata, never a running Windows app."""

import hashlib
import json
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
        executable = bytearray(512)
        executable[:2] = b"MZ"
        struct.pack_into("<I", executable, 0x3C, 64)
        executable[64:68] = b"PE\0\0"
        struct.pack_into("<H", executable, 68, 0x8664)
        struct.pack_into("<H", executable, 84, 240)
        struct.pack_into("<H", executable, 86, 0x0022)
        struct.pack_into("<H", executable, 88, 0x20B)
        struct.pack_into("<H", executable, 156, 2)
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
        with zipfile.ZipFile(first / release.PRIMARY_ARCHIVE) as archive:
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

    def test_stable_download_names_and_legacy_alias_are_identical_and_checked(self):
        output = self.root / "output"
        created = release.package(self.root, self.executable, output, "v1.2.3")
        legacy = "lingoleaf-1.2.3-windows-x64.exe"
        self.assertEqual(
            {path.name for path in created},
            {release.PRIMARY_EXECUTABLE, release.PRIMARY_ARCHIVE, legacy, "update.json", "SHA256SUMS"},
        )
        self.assertEqual((output / release.PRIMARY_EXECUTABLE).read_bytes(), self.executable.read_bytes())
        self.assertEqual((output / legacy).read_bytes(), (output / release.PRIMARY_EXECUTABLE).read_bytes())
        metadata = json.loads((output / "update.json").read_text(encoding="utf-8"))
        # This exact URL is the contract already enforced by 0.5/0.6 clients.
        self.assertEqual(metadata["download_url"], f"{release.UPDATE_REPOSITORY}/releases/download/v1.2.3/{legacy}")
        checksum_files = {
            line.split("  ")[1]
            for line in (output / "SHA256SUMS").read_text(encoding="ascii").splitlines()
        }
        self.assertEqual(checksum_files, {release.PRIMARY_EXECUTABLE, release.PRIMARY_ARCHIVE, legacy, "update.json"})
        self.assertFalse((output / "lingoleaf-1.2.3-windows-x64.zip").exists())

    def test_stable_manifest_asset_is_exactly_scoped_to_its_release(self):
        metadata = json.loads(release.update_manifest("1.2.3", self.executable.read_bytes()))
        stable_url = f"{release.UPDATE_REPOSITORY}/releases/download/v1.2.3/lingoleaf.exe"
        metadata["download_url"] = stable_url
        self.assertEqual(release.validate_update_manifest(metadata), metadata)
        for url in (
            stable_url.replace("v1.2.3/", "v1.2.4/"),
            stable_url + "?download=1",
            stable_url + "#fragment",
            stable_url.replace("lingoleaf.exe", "LingoLeaf.exe"),
            stable_url.replace("lingoleaf.exe", "../lingoleaf.exe"),
            stable_url.replace("lingoleaf.exe", "%6cingoleaf.exe"),
            stable_url.replace("github.com/", "github.com.evil.invalid/"),
            stable_url.replace("/test-hub/", "/other/"),
        ):
            with self.subTest(url=url):
                with self.assertRaises(release.ReleaseError):
                    release.validate_update_manifest({**metadata, "download_url": url})

    def test_optional_screenshot_preserves_relative_path(self):
        image = self.root / release.OPTIONAL_DOCUMENTS[0]
        image.parent.mkdir(parents=True)
        image.write_bytes(b"dummy image")
        output = self.root / "output"
        release.package(self.root, self.executable, output)
        with zipfile.ZipFile(output / release.PRIMARY_ARCHIVE) as archive:
            self.assertEqual(archive.read(release.OPTIONAL_DOCUMENTS[0]), b"dummy image")

    def test_update_manifest_identifies_exact_executable_and_release(self):
        output = self.root / "output"
        release.package(self.root, self.executable, output, "v1.2.3")
        metadata = json.loads((output / "update.json").read_text(encoding="utf-8"))
        executable = (output / "lingoleaf-1.2.3-windows-x64.exe").read_bytes()
        self.assertEqual(set(metadata), release.UPDATE_FIELDS)
        self.assertEqual(metadata["schema_version"], 1)
        self.assertEqual(metadata["version"], "1.2.3")
        self.assertEqual(metadata["size"], len(executable))
        self.assertEqual(metadata["sha256"], hashlib.sha256(executable).hexdigest())
        self.assertEqual(
            metadata["download_url"],
            "https://github.com/xiaoxiaoawei/test-hub/releases/download/v1.2.3/lingoleaf-1.2.3-windows-x64.exe",
        )
        self.assertEqual(
            metadata["notes_url"],
            "https://github.com/xiaoxiaoawei/test-hub/releases/tag/v1.2.3",
        )
        self.assertEqual(release.validate_update_manifest(metadata), metadata)
        checksums = (output / "SHA256SUMS").read_text(encoding="ascii")
        self.assertIn("  update.json\n", checksums)
        with zipfile.ZipFile(output / release.PRIMARY_ARCHIVE) as archive:
            self.assertNotIn("update.json", archive.namelist())

    def test_update_manifest_rejects_url_spoofing_and_version_mismatches(self):
        metadata = json.loads(release.update_manifest("1.2.3", self.executable.read_bytes()))
        for url in (
            "http://github.com/xiaoxiaoawei/test-hub/releases/download/v1.2.3/lingoleaf-1.2.3-windows-x64.exe",
            "https://github.com.example.com/xiaoxiaoawei/test-hub/releases/download/v1.2.3/lingoleaf-1.2.3-windows-x64.exe",
            "https://github.com@evil.example/xiaoxiaoawei/test-hub/releases/download/v1.2.3/lingoleaf-1.2.3-windows-x64.exe",
            "https://github.com/another/repository/releases/download/v1.2.3/lingoleaf-1.2.3-windows-x64.exe",
            "https://github.com/xiaoxiaoawei/test-hub/releases/download/v1.2.4/lingoleaf-1.2.3-windows-x64.exe",
            metadata["download_url"] + "?redirect=https://evil.example",
            metadata["download_url"].replace(".exe", ".zip"),
        ):
            with self.subTest(url=url):
                with self.assertRaises(release.ReleaseError):
                    release.validate_update_manifest({**metadata, "download_url": url})
        with self.assertRaises(release.ReleaseError):
            release.validate_update_manifest({**metadata, "notes_url": metadata["notes_url"] + "/other"})

    def test_update_manifest_rejects_ambiguous_or_unsupported_metadata(self):
        metadata = json.loads(release.update_manifest("1.2.3", self.executable.read_bytes()))
        for key, value in (
            ("download_url", []),
            ("download_url", {}),
            ("download_url", None),
            ("schema_version", True),
            ("schema_version", 2),
            ("version", "1.2.3-beta.1"),
            ("version", "01.2.3"),
            ("version", "65536.2.3"),
            ("sha256", "A" * 64),
            ("sha256", "0" * 63),
            ("size", True),
            ("size", 0),
            ("size", release.MAX_EXECUTABLE_BYTES + 1),
        ):
            with self.subTest(key=key, value=value):
                with self.assertRaises(release.ReleaseError):
                    release.validate_update_manifest({**metadata, key: value})
        with self.assertRaises(release.ReleaseError):
            release.validate_update_manifest({**metadata, "fallback_url": "https://evil.example"})

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

    def test_console_dll_and_non_pe32_plus_inputs_are_rejected(self):
        original = self.executable.read_bytes()
        for offset, value in ((86, 0x2022), (88, 0x10B), (156, 3), (84, 0)):
            with self.subTest(offset=offset, value=value):
                executable = bytearray(original)
                struct.pack_into("<H", executable, offset, value)
                self.executable.write_bytes(executable)
                with self.assertRaises(release.ReleaseError):
                    release.windows_executable(self.executable)

    def test_oversized_executable_is_rejected_before_output(self):
        with self.executable.open("r+b") as executable:
            executable.truncate(release.MAX_EXECUTABLE_BYTES + 1)
        output = self.root / "output"
        with self.assertRaises(release.ReleaseError):
            release.package(self.root, self.executable, output)
        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
