import copy
import json
from pathlib import Path
import shutil
import struct
import tempfile
import unittest

import release


class ReleaseGateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        for name in ("VERSION", "CHANGELOG.md", "README.md", "wails.json", "winres/winres.json", "frontend/package.json", "frontend/package-lock.json", "internal/service/service.go"):
            dst = self.root / name
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(release.ROOT / name, dst)

    def tearDown(self):
        self.temp.cleanup()

    def package_fixture(self, name, payload=b""):
        app, out = self.root / (name + "-app"), self.root / (name + "-assets")
        adapter = app / "adapters/altium/PackageStudio.Altium.dll"
        adapter.parent.mkdir(parents=True)
        adapter.write_bytes(b"test adapter, never executed")
        pe = bytearray(512)
        pe[:2] = b"MZ"
        struct.pack_into("<I", pe, 0x3C, 0x80)
        pe[0x80:0x84] = b"PE\0\0"
        struct.pack_into("<H", pe, 0x84, 0x8664)
        struct.pack_into("<H", pe, 0x80 + 22, 0x0002)
        struct.pack_into("<H", pe, 0x80 + 24, 0x20B)
        struct.pack_into("<H", pe, 0x80 + 24 + 68, 2)
        (app / "PackageStudio.exe").write_bytes(pe + payload)
        release.package(app, out, self.root)
        return out

    def test_source_versions_and_tag_must_agree(self):
        self.assertEqual(release.check(self.root, "package-studio-v" + release.version()), release.version())
        with self.assertRaises(ValueError):
            release.check(self.root, "v" + release.version())
        (self.root / "VERSION").write_text("01.3.0")
        with self.assertRaises(ValueError):
            release.check(self.root)

    def test_mismatched_frontend_is_rejected(self):
        path = self.root / "frontend/package-lock.json"
        data = release.read_json(path)
        data["packages"][""]["version"] = "0.0.1"
        path.write_text(json.dumps(data))
        with self.assertRaises(ValueError):
            release.check(self.root)

    def test_stable_feed_cannot_downgrade_or_rewrite_existing_version(self):
        current = dict(product="package-studio", version=release.version(), sha256="a" * 64)
        source = self.root / "new.json"
        source.write_text(json.dumps(current))
        release.promote(source, self.root)
        release.promote(source, self.root)
        current["sha256"] = "b" * 64
        source.write_text(json.dumps(current))
        with self.assertRaises(ValueError):
            release.promote(source, self.root)
        target = self.root / "updates/package-studio/stable.json"
        target.write_text(json.dumps(dict(product="package-studio", version="99.0.0")))
        with self.assertRaises(ValueError):
            release.promote(source, self.root)

    def test_cli_console_and_wrong_architecture_are_rejected(self):
        for data in (b"MZ" + bytes(512), b"ELF" + bytes(512), b"hello"):
            with self.assertRaises(ValueError):
                release.validate_pe(data)

    def test_identical_published_assets_allow_resume(self):
        expected = self.package_fixture("original")
        published = self.root / "published"
        shutil.copytree(expected, published)
        self.assertEqual(release.compare_assets(expected, published, self.root)["version"], release.version(self.root))

    def test_rebuilt_same_version_archive_cannot_resume_existing_release(self):
        expected = self.package_fixture("original")
        published = self.package_fixture("different", b"changed executable bytes")
        # Both complete packages are independently valid for the same version.
        release.verify(expected, self.root)
        release.verify(published, self.root)
        with self.assertRaisesRegex(ValueError, "immutable release asset mismatch"):
            release.compare_assets(expected, published, self.root)

    def test_changed_manifest_timestamp_is_an_immutable_conflict(self):
        expected = self.package_fixture("original")
        published = self.root / "published"
        shutil.copytree(expected, published)
        manifest = release.read_json(published / "update.json")
        manifest["published_at"] = "2000-01-01T00:00:00Z"
        (published / "update.json").write_text(json.dumps(manifest), encoding="utf-8")
        (published / "SHA256SUMS.txt").write_text("".join(f"{release.digest(published / name)}  {name}\n" for name in (release.ARCHIVE, "update.json")), encoding="utf-8")
        release.verify(published, self.root)
        with self.assertRaisesRegex(ValueError, "immutable release asset mismatch: update.json"):
            release.compare_assets(expected, published, self.root)

    def test_feed_main_version_does_not_replace_tag_source_validation(self):
        feed = self.root / "feed-checkout"
        feed.mkdir()
        (feed / "VERSION").write_text("99.0.0")
        manifest = dict(product="package-studio", version=release.version(self.root), sha256="a" * 64)
        source = self.root / "verified-published.json"
        source.write_text(json.dumps(manifest))
        self.assertTrue(release.promote(source, feed, source_root=self.root))
        self.assertEqual(release.read_json(feed / "updates/package-studio/stable.json"), manifest)
        self.assertEqual((feed / "VERSION").read_text(), "99.0.0")
        self.assertFalse(release.promote(source, feed, source_root=self.root))

    def test_retry_against_newer_remote_feed_refuses_downgrade(self):
        feed = self.root / "feed-checkout"
        target = feed / "updates/package-studio/stable.json"
        target.parent.mkdir(parents=True)
        newer = dict(product="package-studio", version="99.0.0", sha256="b" * 64)
        target.write_text(json.dumps(newer))
        manifest = dict(product="package-studio", version=release.version(self.root), sha256="a" * 64)
        source = self.root / "verified-published.json"
        source.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "downgrade"):
            release.promote(source, feed, source_root=self.root)
        self.assertEqual(release.read_json(target), newer)


if __name__ == "__main__":
    unittest.main()
