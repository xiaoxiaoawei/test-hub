"""Run the built Windows binary and exercise persisted settings/export dispatch."""
import argparse
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request
import urllib.error


def main():
    p = argparse.ArgumentParser()
    p.add_argument("exe", type=Path)
    p.add_argument("--online-search", help="Optional live source check, e.g. STM32F030C8T6 (not run in CI)")
    args = p.parse_args()
    exe = args.exe.resolve()
    with tempfile.TemporaryDirectory(prefix="package-studio-smoke-") as tmp:
        root = Path(tmp)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        process = subprocess.Popen([str(exe), "--serve", f"127.0.0.1:{port}", "--data-dir", str(root / "data")], creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        def request(path, data=None, method=None):
            req = urllib.request.Request(base + path, data=None if data is None else json.dumps(data).encode(), headers={"Content-Type": "application/json"}, method=method)
            with urllib.request.urlopen(req, timeout=20) as r:
                return json.load(r)
        try:
            for _ in range(100):
                try:
                    boot = request("/api/bootstrap")
                    break
                except OSError:
                    if process.poll() is not None:
                        raise RuntimeError("binary exited before bootstrap")
                    time.sleep(.1)
            else:
                raise RuntimeError("binary did not start")
            expected = (Path(__file__).resolve().parents[1] / "VERSION").read_text().strip()
            assert boot["version"] == expected
            parsed = request("/api/parts/list/parse", {"fileName": "sample.csv", "raw": "Designator,MPN,LCSC Part #\nC12,STM32F103C8T6,C25804\nC34,LM358,c25804"})
            assert parsed["ids"] == ["C25804"] and parsed["duplicates"] == 1
            env = request("/api/native/environment", {})
            assert isinstance(env["pads"], list) and isinstance(env["cadence"], list)
            assert all(not item["runtimeVerified"] for item in env["pads"] + env["cadence"])
            # Exercise the HTTP search route without making CI depend on an external source.
            try:
                request("/api/parts/search", {"query": "", "page": 1})
            except urllib.error.HTTPError as error:
                assert error.code == 400
                assert json.load(error)["error"]
            else:
                raise AssertionError("empty model query was accepted")
            part = boot["parts"][0]
            for target in ("kicad", "pads", "cadence"):
                assert boot["capabilities"][target]
                settings = dict(boot["settings"], exportTarget=target, theme="dark")
                saved = request("/api/settings", settings, "PUT")
                assert saved["exportTarget"] == target
                result = request("/api/export", dict(partIds=[part["id"]], outputDirectory=str(root / "exports"), libraryName="CI库", includeSymbol=True, includeFootprint=True, includeModel=False, target=target))
                assert result["files"]
                assert all(Path(f).is_file() for f in result["files"])
            history = request("/api/history")
            assert len(history) == 3
            assert history[0]["artifactType"] == "intermediate" and not history[0].get("nativeGenerated", False)
            # An explicitly selected missing engine must fail without fallback
            # or mutation of an existing library. This is not native EDA QA.
            existing = root / "existing.olb"
            existing.write_bytes(b"smoke-test-existing-library")
            for target in ("pads", "cadence"):
                native = dict(enabled=True, libraryMode="append", padsRoot=str(root / "missing-pads"),
                              padsSymbolLibrary=str(existing), padsFootprintLibrary=str(existing),
                              captureExe=str(root / "missing-Capture.exe"), allegroExe=str(root / "missing-allegro.exe"),
                              captureLibrary=str(existing), allegroDirectory=str(root / "existing-allegro"))
                try:
                    request("/api/export", dict(partIds=[part["id"]], outputDirectory=str(root / "exports"), libraryName="CI-native", includeSymbol=True, includeFootprint=True, includeModel=False, target=target, nativeConfig=native))
                except urllib.error.HTTPError as error:
                    assert error.code == 400 and json.load(error)["error"]
                else:
                    raise AssertionError("missing EDA engine returned success")
                assert existing.read_bytes() == b"smoke-test-existing-library"
            persisted = json.loads((root / "data/settings.json").read_text(encoding="utf-8"))
            assert persisted["exportTarget"] == "cadence"
            if args.online_search:
                result = request("/api/parts/search", {"query": args.online_search, "page": 1})
                assert result["items"], "live model search returned no candidates"
                match = next((x for x in result["items"] if x["name"].casefold() == args.online_search.casefold()), result["items"][0])
                fetched = request("/api/parts/fetch", {"lcscID": match["lcscId"]})
                assert fetched["footprint"]["pads"] and fetched["symbol"]["pins"]
                print("Live model search and geometry fetch passed:", args.online_search, match["lcscId"])
            print("Windows binary smoke passed:", expected, "KiCad/PADS/Cadence, settings, history")
        finally:
            process.terminate()
            process.wait(timeout=10)


if __name__ == "__main__":
    main()
