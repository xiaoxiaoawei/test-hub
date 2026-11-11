"""Run the built Windows binary and exercise persisted settings/export dispatch."""
import argparse
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request


def main():
    p = argparse.ArgumentParser()
    p.add_argument("exe", type=Path)
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
            persisted = json.loads((root / "data/settings.json").read_text(encoding="utf-8"))
            assert persisted["exportTarget"] == "cadence"
            print("Windows binary smoke passed:", expected, "KiCad/PADS/Cadence, settings, history")
        finally:
            process.terminate()
            process.wait(timeout=10)


if __name__ == "__main__":
    main()
