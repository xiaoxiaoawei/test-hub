"""Exercise the built binary's HTTP service; native EDA verification is separate."""
import argparse
import copy
import hashlib
import json
import math
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request
import urllib.error


def seed_model_fixtures(root):
    """Use only our own asymmetric solid/mesh, never downloaded product data."""
    fixture_dir = Path(__file__).resolve().parents[1] / "internal/core/testdata"
    data_dir = root / "data"
    model_dir = data_dir / "models"
    model_dir.mkdir(parents=True)
    source = {ext: (fixture_dir / f"model_asymmetric.{ext}").read_bytes()
              for ext in ("step", "obj")}
    uuid = "b" * 32
    for ext, raw in source.items():
        (model_dir / f"{uuid}.{ext}").write_bytes(raw)
    broken_uuid = "c" * 32
    (model_dir / f"{broken_uuid}.step").write_bytes(b"deliberately invalid cached STEP fixture")
    (model_dir / f"{broken_uuid}.obj").write_bytes(source["obj"])
    part = dict(
        id="smoke-model-safe", lcscId="", name="Smoke_Asymmetric", package="SMOKE_2PAD",
        description="Self-authored offline service fixture", manufacturer="Package Studio",
        source="Package Studio original smoke fixture", warnings=[],
        footprint=dict(name="SMOKE_2PAD", pads=[
            dict(number=str(index + 1), x=x, y=0, width=.8, height=.95,
                 shape="rect", type="smd", layer="F.Cu", rotation=0)
            for index, x in enumerate((-.825, .825))], lines=[], circles=[], polygons=[], texts=[],
            bounds=dict(x=-1.225, y=-.475, width=2.45, height=.95)),
        symbol=dict(name="Smoke_Asymmetric", reference="R", units=1,
                    pins=[dict(number=str(index + 1), name=str(index + 1), x=x, y=0,
                               length=2.54, rotation=rotation, electricalType="passive", unit=1)
                          for index, (x, rotation) in enumerate(((-5.08, 0), (5.08, 180)))],
                    rects=[dict(x=-2.54, y=-1.016, width=5.08, height=2.032, unit=1)],
                    lines=[], circles=[], polygons=[],
                    bounds=dict(x=-5.08, y=-1.016, width=10.16, height=2.032)),
        model=dict(uuid=uuid, path="", placementConvention="easyeda-std-kicad-v1",
                   x=1.3, y=-2.4, z=-.7, rotationX=0, rotationY=0, rotationZ=-37,
                   scaleX=1, scaleY=1, scaleZ=1, sourceWidthMm=3.5, sourceHeightMm=2))
    parts = [part]
    for suffix, change in (("unsupported-xy", dict(rotationX=90)),
                           ("unknown-placement", dict(placementConvention="unknown-smoke-convention")),
                           ("invalid-step", dict(uuid=broken_uuid))):
        variant = copy.deepcopy(part)
        variant["id"] = f"smoke-model-{suffix}"
        variant["model"].update(change)
        parts.append(variant)
    missing = copy.deepcopy(part)
    missing["id"] = "smoke-model-no-source"
    del missing["model"]
    parts.append(missing)
    (data_dir / "parts.json").write_text(json.dumps(parts), encoding="utf-8")
    return parts, {file: file.read_bytes() for file in model_dir.iterdir()}


def expect_rejection(request, payload, contains=None):
    try:
        request("/api/export", payload)
    except urllib.error.HTTPError as error:
        assert error.code == 400, f"unexpected export error status: {error.code}"
        message = json.load(error)["error"]
        assert message, "rejected export did not explain the error"
        if contains:
            assert contains in message, message
        return message
    raise AssertionError("unsafe model export unexpectedly succeeded")


def smoke_model_associations(request, root, fixtures, source_cache):
    def export_request(target, part_id="smoke-model-safe", **overrides):
        payload = dict(partIds=[part_id], outputDirectory=str(root / "model-exports" / target),
                       libraryName="SmokeModel", includeSymbol=True, includeFootprint=True,
                       includeModel=True, target=target,
                       nativeConfig=dict(enabled=False, modelAssociationMode="prepare"))
        payload.update(overrides)
        return payload

    # Golden bounds independently calculated from the authored asymmetric mesh:
    # source +37 degrees CCW, then offset (1.3, -2.4, -.7) mm. The second box
    # extends past the first, so rotating its untransformed bounding box is wrong.
    expected_bbox = [.09636995369590351, -2.4, -.7,
                     3.974861280535116, 1.0027160895507303, 2.1]
    for target in ("pads", "cadence"):
        try:
            result = request("/api/export", export_request(target))
        except urllib.error.HTTPError as error:
            raise AssertionError(f"{target} offline model preparation failed: {error.read().decode()}") from error
        assert result["artifactType"] == "intermediate"
        assert result["validationStatus"] == "awaiting-eda"
        assert not result.get("nativeGenerated") and not result.get("runtimeVerified")
        assert result["files"] and all(Path(f).is_file() for f in result["files"])
        associations = result["modelAssociations"]
        assert len(associations) == 1
        model = associations[0]
        assert model["partId"] == "smoke-model-safe" and model["entry"]
        assert model["status"] == "prepared" and not model["verified"] and not model["geometryVerified"]
        assert not model.get("actualBBoxMm"), "prepared models must not claim EDA readback bounds"
        assert len(model["expectedBBoxMm"]) == 6
        assert all(math.isclose(actual, expected, abs_tol=1e-8)
                   for actual, expected in zip(model["expectedBBoxMm"], expected_bbox)), model
        model_file = Path(model["modelFile"])
        if not model_file.is_absolute():
            model_file = Path(result["directory"]) / model_file
        assert model_file.resolve().is_relative_to(Path(result["directory"]).resolve())
        model_bytes = model_file.read_bytes()
        assert hashlib.sha256(model_bytes).hexdigest() == model["modelSha256"]
        assert model_bytes != source_cache[root / "data/models" / (("b" * 32) + ".step")]
        assert request("/api/history")[0]["modelAssociations"] == associations
        if target == "pads":
            plans = [Path(file) for file in result["files"] if Path(file).name == "association-plan.json"]
            assert len(plans) == 1 and json.loads(plans[0].read_text(encoding="utf-8"))["mode"] == "prepared-only"
        else:
            builds = [Path(file) for file in result["files"] if Path(file).name == "build.il"]
            mappings = [Path(file) for file in result["files"] if file.endswith(".step-map.il")]
            assert builds and mappings, "Cadence preparation omitted its separate association script"
            assert all("axlStepSet" not in file.read_text(encoding="utf-8") for file in builds)
            assert all("axlStepSet" in file.read_text(encoding="utf-8") for file in mappings)

        # Missing source is reported explicitly, never as a successful mapping.
        unavailable = request("/api/export", export_request(target, "smoke-model-no-source"))
        unavailable_model = unavailable["modelAssociations"][0]
        assert unavailable_model["status"] == "unavailable"
        assert not unavailable_model["verified"] and not unavailable_model["geometryVerified"]

        for suffix, message in (("unsupported-xy", "X/Y"),
                                ("unknown-placement", "坐标约定"), ("invalid-step", "Part 21")):
            output = root / f"rejected-{target}-{suffix}"
            expect_rejection(request, export_request(target, f"smoke-model-{suffix}",
                                                     outputDirectory=str(output)), message)
            assert not output.exists(), "unsafe model preparation published an export directory"
            failure = request("/api/history")[0]
            assert failure["status"] == "error"
            assert not any(item.get("verified") or item.get("geometryVerified")
                           or item.get("status") == "associated"
                           for item in failure.get("modelAssociations", []))

    # Native model selection is distinct from preparation: a missing modern
    # engine cannot silently fall back or alter an existing Reuse directory.
    reuse = root / "existing-reuse"
    reuse.mkdir()
    sentinel = reuse / "user-library.bin"
    sentinel.write_bytes(b"untouched-user-reuse-library")
    expect_rejection(request, export_request("pads", nativeConfig=dict(
        enabled=False, modelAssociationMode="native", padsModelExe=str(root / "missing-powerpcb.exe"),
        padsReuseDirectory=str(reuse))))
    assert list(reuse.iterdir()) == [sentinel]
    assert sentinel.read_bytes() == b"untouched-user-reuse-library"
    rejected_native = request("/api/history")[0]
    assert rejected_native["status"] == "error"
    assert not any(item.get("verified") or item.get("geometryVerified")
                   or item.get("status") == "associated"
                   for item in rejected_native.get("modelAssociations", []))
    output = root / "rejected-cadence-native-without-library"
    expect_rejection(request, export_request("cadence", outputDirectory=str(output),
        nativeConfig=dict(enabled=False, modelAssociationMode="native")), "启用原生库")
    assert not output.exists()

    # Exports must leave cached raw STEP/OBJ and source placement untouched.
    assert all(file.read_bytes() == original for file, original in source_cache.items())
    current = {part["id"]: part for part in request("/api/bootstrap")["parts"]}
    for part in fixtures:
        assert current[part["id"]].get("model") == part.get("model"), part["id"]
    print("Offline HTTP model smoke passed: PADS/Cadence prepared, unavailable, safe rejection, cache isolation")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("exe", type=Path)
    p.add_argument("--online-search", help="Optional live source check, e.g. STM32F030C8T6 (not run in CI)")
    args = p.parse_args()
    exe = args.exe.resolve()
    with tempfile.TemporaryDirectory(prefix="package-studio-smoke-") as tmp:
        root = Path(tmp)
        fixtures, source_cache = seed_model_fixtures(root)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        environment = os.environ.copy()
        if not args.online_search:
            # No external source is needed for the normal CI smoke. Make an
            # accidental cache miss fail locally instead of contacting EasyEDA.
            environment.update(HTTP_PROXY="http://127.0.0.1:1", HTTPS_PROXY="http://127.0.0.1:1",
                               NO_PROXY="127.0.0.1,localhost", http_proxy="http://127.0.0.1:1",
                               https_proxy="http://127.0.0.1:1", no_proxy="127.0.0.1,localhost")
        process = subprocess.Popen([str(exe), "--serve", f"127.0.0.1:{port}", "--data-dir", str(root / "data")],
                                   env=environment, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
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
            smoke_model_associations(request, root, fixtures, source_cache)
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
