#!/usr/bin/env python3
"""Managed first-full RAW benchmark (task T00).

One command produces an auditable manifest, merged trace events, per-run
metrics and a summary. It never edits a test original: every run uses a fresh
private copy, and the copy's before/after hash is recorded.

Clocks:
  cold  : the culler's ``open_requested.started_unix_us`` -> ``capture`` event
          for the 100% frame (before PNG encoding).
  reuse : the harness' write of a private IPC request -> ``capture``.

Trace files come from ``FASTCULL_RAW_TRACE_DIR`` (see src/raw_trace.rs). The
harness also accepts runs without a trace by falling back to the historical
smoke files, but such a run is marked ``trace=false`` and cannot pass the
completion validator.

Negative self-test: ``--self-test`` proves that a decode-only endpoint and a
decode+scene endpoint without a presented full frame are rejected.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPORT_SCHEMA = "fastcull.raw_bench.report.v1"
EVENT_SCHEMA = "fastcull.raw_trace.v1"
BASELINE = ROOT / "docs/raw-optimization-baseline-2026-11-11.json"
INSTALLED = [
    "lrplugins/FastCull.lrplugin/fastcull.exe",
    "lrplugins/FastCull.lrplugin/fastcull_raw.dll",
    "lrplugins/FastCull.lrplugin/vcomp140.dll",
]
SOURCE_PATTERNS = [
    "src/**/*.rs",
    "src/**/*.wgsl",
    "native/*",
    "examples/*.rs",
    "tests/*.rs",
    "tests/*.py",
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "scripts/build-native.ps1",
    "scripts/build-macos.py",
    "scripts/release.py",
    "assets/fastcull.ico",
]

# Same controlled baseline as docs/raw-optimization-baseline-2026-11-11.json,
# with +1 EV for the ARW/NEF timing recipe.
RECIPE_XMP = """<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Version="18.7" crs:ProcessVersion="11.0" crs:CameraProfile="Adobe Standard" crs:WhiteBalance="Custom" crs:Temperature="5500" crs:Tint="0" crs:Exposure2012="1" crs:Contrast2012="0" crs:Highlights2012="0" crs:Shadows2012="0" crs:Whites2012="0" crs:Blacks2012="0" crs:Vibrance="0" crs:Saturation="0" crs:Sharpness="0" crs:LuminanceSmoothing="0" crs:ColorNoiseReduction="0" crs:AutoTone="False" crs:HasSettings="True" crs:CropEnabled="False" crs:ToneCurveName2012="Linear" crs:Clarity2012="0" crs:Texture="0" crs:Dehaze="0" crs:LensProfileEnable="0" crs:RemoveChromaticAberration="False"><crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012></rdf:Description></rdf:RDF></x:xmpmeta>"""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def unix_us() -> int:
    return time.time_ns() // 1000


def stamp() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds")


def read_jsonl(path: Path) -> list[dict]:
    events: list[dict] = []
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return events
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and "stage" in event:
            events.append(event)
    return events


def collect_trace(directory: Path) -> tuple[list[dict], list[str]]:
    events: list[dict] = []
    files: list[str] = []
    if not directory.is_dir():
        return events, files
    for path in sorted(directory.glob("raw-trace-*.jsonl")):
        files.append(path.name)
        events.extend(read_jsonl(path))
    events.sort(key=lambda e: (e.get("unix_us", 0), e.get("mono_ms", 0)))
    return events, files


def first(events: list[dict], stage: str, *, after: int | None = None, **fields):
    for event in events:
        if event.get("stage") != stage:
            continue
        if after is not None and event.get("unix_us", 0) < after:
            continue
        if all(event.get(key) == value for key, value in fields.items()):
            return event
    return None


def all_events(events: list[dict], stage: str, *, after: int | None = None, **fields):
    return [
        event
        for event in events
        if event.get("stage") == stage
        and (after is None or event.get("unix_us", 0) >= after)
        and all(event.get(key) == value for key, value in fields.items())
    ]


def ms_between(end: int | None, start: int | None) -> float | None:
    if end is None or start is None or end < start:
        return None
    return round((end - start) / 1000.0, 3)


def validate_run(events: list[dict], *, anchor_us: int | None) -> tuple[bool, list[str]]:
    """A run is complete only with a full scene and a presented final frame."""
    events = sorted(events, key=lambda event: event.get("unix_us", 0))
    problems: list[str] = []
    stages = {event.get("stage") for event in events}
    if anchor_us is None and "open_requested" not in stages:
        problems.append("missing open_requested anchor")
    if "decode_done" not in stages:
        problems.append("missing decode_done")
    full_scenes = [
        event
        for event in events
        if event.get("stage") == "scene_ready" and event.get("full") is True
    ]
    if not full_scenes:
        problems.append("no full-size scene_ready")
    textures = [
        event
        for event in events
        if event.get("stage") in ("texture_upload", "texture_adopt")
    ]
    if not textures:
        problems.append("no GPU texture available")
    captures = [
        event
        for event in events
        if event.get("stage") == "capture" and event.get("name") == "00-loaded.png"
    ]
    if not captures:
        problems.append(
            "no presented 100% frame; a decode-only or scene-only endpoint is not completion"
        )
    elif full_scenes and captures[0].get("unix_us", 0) < full_scenes[0].get("unix_us", 0):
        problems.append("final frame captured before the full scene was ready")
    return not problems, problems


def self_test() -> int:
    anchor = 1_000
    base = [
        {"stage": "open_requested", "unix_us": anchor, "started_unix_us": anchor},
        {"stage": "decode_done", "unix_us": 2_000, "reused": False,
         "unpack_done_unix_us": 2_000},
        {"stage": "scene_ready", "unix_us": 3_000, "full": True, "resident": True},
        {"stage": "texture_adopt", "unix_us": 3_100, "bytes": 0},
        {"stage": "capture", "unix_us": 3_500, "name": "00-loaded.png"},
    ]
    cases = {
        "complete": (base, True),
        "decode-only": (base[:2], False),
        "decode+scene without presented frame": (base[:4], False),
        "frame captured before full scene": (
            [base[0], base[1], {**base[4], "unix_us": 2_500}, base[2], base[3]],
            False,
        ),
    }
    failed = False
    for name, (events, expected) in cases.items():
        ok, problems = validate_run(events, anchor_us=anchor)
        print(f"self-test {name}: {'accepted' if ok else 'rejected'}"
              f"{'' if ok else ' — ' + '; '.join(problems)}")
        if ok != expected:
            failed = True
    if failed:
        print("self-test FAILED: validator accepted a fake endpoint", file=sys.stderr)
        return 1
    print("self-test passed: decode-only and scene-only endpoints are rejected")
    return 0


def source_manifest() -> dict[str, str]:
    manifest: dict[str, str] = {}
    for pattern in SOURCE_PATTERNS:
        for path in sorted(ROOT.glob(pattern)):
            if path.is_file():
                manifest[path.relative_to(ROOT).as_posix()] = sha256(path)
    return manifest


def baseline_drift(manifest: dict[str, str]) -> dict:
    if not BASELINE.is_file():
        return {"baseline": None, "expected_files_missing": [], "drift": {}}
    baseline = json.loads(BASELINE.read_text(encoding="utf-8"))
    expected = dict(baseline.get("production_source_sha256", {}))
    expected.update(baseline.get("extra_build_inputs_sha256", {}))
    missing = sorted(set(expected) - set(manifest))
    drift = {
        path: {"expected": digest, "actual": manifest[path]}
        for path, digest in sorted(expected.items())
        if path in manifest and manifest[path] != digest
    }
    installed = {}
    for relative, meta in baseline.get("installed_files", {}).items():
        path = ROOT / meta["path"]
        actual = sha256(path) if path.is_file() else None
        installed[relative] = {
            "path": meta["path"],
            "expected": meta["sha256"],
            "actual": actual,
            "match": actual == meta["sha256"],
        }
    return {
        "baseline": BASELINE.relative_to(ROOT).as_posix(),
        "expected_files_missing": missing,
        "drift": drift,
        "installed": installed,
    }


def resolve_fixtures(args) -> dict[str, Path]:
    if args.fixtures:
        entries = json.loads(Path(args.fixtures).read_text(encoding="utf-8"))
        fixtures = {}
        for entry in entries:
            fmt = entry["format"].lower()
            if fmt in args.formats:
                fixtures[fmt] = Path(entry["raw"]).resolve()
        return fixtures
    root = args.data_root
    if root is None and os.environ.get("FASTCULL_TEST_DATA_ROOT"):
        root = Path(os.environ["FASTCULL_TEST_DATA_ROOT"])
    if root is None:
        root = ROOT / ".agent-temp/2026-10-23-raw-edit/adobe"
    root = Path(root)
    fixtures = {}
    for fmt in args.formats:
        for candidate in (root / f"probe.{fmt}", root / f"probe.{fmt.upper()}"):
            if candidate.is_file():
                fixtures[fmt] = candidate.resolve()
                break
        else:
            raise SystemExit(
                f"fixture for {fmt} not found under {root}; "
                "use --data-root/FASTCULL_TEST_DATA_ROOT or --fixtures"
            )
    return fixtures


def fixture_manifest(fixtures: dict[str, Path]) -> list[dict]:
    return [
        {
            "format": fmt,
            "path_hint": path.name,
            "sha256": sha256(path),
            "bytes": path.stat().st_size,
        }
        for fmt, path in fixtures.items()
    ]


def environment() -> dict:
    gpu = None
    try:
        result = subprocess.run(
            ["nvidia-smi", "--query-gpu=name,driver_version,memory.total",
             "--format=csv,noheader"],
            capture_output=True, text=True, timeout=10, check=False,
        )
        if result.returncode == 0:
            gpu = [line.strip() for line in result.stdout.splitlines() if line.strip()]
    except (OSError, subprocess.TimeoutExpired):
        pass
    return {
        "os": platform.platform(),
        "machine": platform.machine(),
        "python": sys.version.split()[0],
        "cpu": platform.processor(),
        "gpu": gpu,
        "gpu_backend": "DirectX 12" if os.name == "nt" else "Metal",
        "available_parallelism": os.cpu_count(),
    }


ENV_OPTIONS: dict[str, object] = {}


def prepare_env(run: Path, smoke: Path, drag: int = 0) -> dict[str, str]:
    env = os.environ.copy()
    for key in (
        "FASTCULL_SMOKE_DIR",
        "FASTCULL_SMOKE_SIZE",
        "FASTCULL_RAW_EDIT_SMOKE_PROFILE",
        "FASTCULL_RAW_COMPUTE",
        "FASTCULL_RAW_RESIDENT",
        "FASTCULL_RAW_HSM",
        "FASTCULL_RAW_DEMOSAIC",
        "FASTCULL_RAW_ENTROPY",
        "FASTCULL_RAW_DLL",
        "FASTCULL_RAW_DRAG_SMOKE",
        "FASTCULL_RAW_PREWARM",
        "FASTCULL_RAW_PREWARM_DELAY_MS",
        "FASTCULL_RAW_SMOKE_DELAY_MS",
        "FASTCULL_RAW_AHD_WARMUP",
    ):
        env.pop(key, None)
    temp = run / "tmp"
    trace = run / "trace"
    logs = run / "logs"
    for directory in (temp, trace, logs, smoke):
        directory.mkdir(parents=True, exist_ok=True)
    env.update({
        "FASTCULL_RAW_EDIT_SMOKE_DIR": str(smoke),
        "FASTCULL_RAW_FIRST_100_SMOKE": "1",
        "FASTCULL_RAW_FIRST_DETAIL_SMOKE": "1",
        "FASTCULL_RAW_LATENCY_SMOKE": "1",
        "FASTCULL_RAW_PRELOAD_SMOKE": "1",
        "FASTCULL_RAW_TRACE_DIR": str(trace),
        "FASTCULL_RAW_TRACE_ID": run.name,
        "FASTCULL_LOG_DIR": str(logs),
        "FASTCULL_SETTINGS_FILE": str(run / "settings.txt"),
        "FASTCULL_APPEARANCE_FILE": str(run / "appearance.txt"),
        "TEMP": str(temp),
        "TMP": str(temp),
    })
    if drag:
        env["FASTCULL_RAW_DRAG_SMOKE"] = str(drag)
    if ENV_OPTIONS.get("no_prewarm"):
        env["FASTCULL_RAW_PREWARM"] = "0"
    entropy_mode = ENV_OPTIONS.get("entropy_mode")
    if entropy_mode and entropy_mode != "default":
        env["FASTCULL_RAW_ENTROPY"] = str(entropy_mode)
    ahd_warmup = ENV_OPTIONS.get("ahd_warmup")
    if ahd_warmup and ahd_warmup != "default":
        env["FASTCULL_RAW_AHD_WARMUP"] = "1" if ahd_warmup == "on" else "0"
    if ENV_OPTIONS.get("prewarm_delay_ms") is not None:
        env["FASTCULL_RAW_PREWARM_DELAY_MS"] = str(ENV_OPTIONS["prewarm_delay_ms"])
    if ENV_OPTIONS.get("smoke_delay_ms") is not None:
        env["FASTCULL_RAW_SMOKE_DELAY_MS"] = str(ENV_OPTIONS["smoke_delay_ms"])
    return env


def wait_until(predicate, timeout: float, interval: float = 0.02):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            value = predicate()
        except OSError:
            value = None
        if value:
            return value
        time.sleep(interval)
    return None


def terminate_tree(process: subprocess.Popen) -> None:
    if process.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            capture_output=True, check=False,
        )
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
    else:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()


def terminate_run(run: Path, process: subprocess.Popen) -> None:
    """Kill the whole launch chain that belongs to this run.

    `fastcull --open` immediately spawns `--open-state-owned` and exits, so the
    Popen PID is already gone while the real culler and editor server survive.
    Check this run's recorded process identities, including those reexecs.
    """
    terminate_tree(process)
    if os.name == "nt":
        receipt = cleanup_traced_windows_processes(run)
        (run / "cleanup.json").write_text(
            json.dumps(receipt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        if not receipt["ok"]:
            raise RuntimeError(f"test process cleanup failed; no further run allowed: {receipt}")
    time.sleep(0.3)


def cleanup_traced_windows_processes(run: Path) -> dict:
    """Close only this run's PID+EXE+creation-time identities, including reexecs.

    A run path need not appear in --open-state-owned/--raw-editor-server argv.
    Holding the process handle through termination also avoids PID reuse races.
    """
    import ctypes as c
    from ctypes import wintypes as w

    kernel = c.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
    kernel.OpenProcess.restype = w.HANDLE
    kernel.QueryFullProcessImageNameW.argtypes = [w.HANDLE, w.DWORD, w.LPWSTR, c.POINTER(w.DWORD)]
    kernel.GetProcessTimes.argtypes = [w.HANDLE] + [c.POINTER(w.FILETIME)] * 4
    kernel.GetExitCodeProcess.argtypes = [w.HANDLE, c.POINTER(w.DWORD)]
    kernel.TerminateProcess.argtypes = [w.HANDLE, w.UINT]
    kernel.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
    kernel.CloseHandle.argtypes = [w.HANDLE]
    identities = {}
    for path in (run / "trace").glob("raw-trace-*.jsonl"):
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if (event.get("stage") == "process_start" and event.get("exe")
                    and Path(event["exe"]).name.lower() == "fastcull.exe"
                    and path.name == f"raw-trace-{event.get('pid')}.jsonl"):
                identities[event["pid"]] = event
                break
    rows = []
    for pid, event in identities.items():
        handle = kernel.OpenProcess(0x1000 | 0x100000 | 1, False, pid)
        if not handle:
            error = c.get_last_error()
            rows.append({"pid": pid, "state": "exited" if error == 87 else "access-failed",
                         "ok": error == 87, "error": error})
            continue
        try:
            code = w.DWORD()
            if kernel.GetExitCodeProcess(handle, c.byref(code)) and code.value != 259:
                rows.append({"pid": pid, "state": "exited", "ok": True})
                continue
            name, size = c.create_unicode_buffer(32768), w.DWORD(32768)
            times = [w.FILETIME() for _ in range(4)]
            if (not kernel.QueryFullProcessImageNameW(handle, 0, name, c.byref(size))
                    or not kernel.GetProcessTimes(handle, *[c.byref(t) for t in times])):
                rows.append({"pid": pid, "state": "query-failed", "ok": False})
                continue
            created = ((times[0].dwHighDateTime << 32) | times[0].dwLowDateTime) // 10 - 11644473600000000
            if (os.path.normcase(name.value) != os.path.normcase(event["exe"])
                    or not 0 <= event["unix_us"] - created < 30_000_000):
                rows.append({"pid": pid, "state": "identity-reused-not-touched", "ok": True})
                continue
            code = w.DWORD()
            if not kernel.GetExitCodeProcess(handle, c.byref(code)):
                rows.append({"pid": pid, "state": "exit-query-failed", "ok": False})
                continue
            if code.value != 259:
                rows.append({"pid": pid, "state": "exited", "ok": True})
                continue
            stopped = bool(kernel.TerminateProcess(handle, 0))
            ended = kernel.WaitForSingleObject(handle, 5000) == 0
            rows.append({"pid": pid, "state": ("terminated" if stopped else "exited")
                         if ended else "still-running",
                         "ok": ended})
        finally:
            kernel.CloseHandle(handle)
    return {"ok": all(row["ok"] for row in rows), "processes": rows}


def copy_fixture(source: Path, destination: Path, fmt: str,
                 dng_sidecar: bool = False) -> dict:
    shutil.copyfile(source, destination)
    sidecar = None
    if fmt in ("arw", "nef") or (fmt == "dng" and dng_sidecar):
        sidecar = destination.with_suffix(".xmp")
        sidecar.write_text(RECIPE_XMP, encoding="utf-8")
    before = sha256(destination)
    return {
        "private_copy": destination.name,
        "private_sha256_before": before,
        "sidecar": sidecar.name if sidecar else None,
        "recipe": "adobe-standard-+1ev-sharpness0-nr0" if sidecar else "embedded-adobe-color",
    }


def run_cold(exe: Path, fixture: Path, fmt: str, run: Path, timeout: float,
             drag: int = 0, dng_sidecar: bool = False) -> dict:
    original_before = sha256(fixture)
    photos = run / "photos"
    photos.mkdir(parents=True, exist_ok=True)
    source = photos / f"photo-0.{fmt}"
    copy = copy_fixture(fixture, source, fmt, dng_sidecar)
    smoke = run / "smoke"
    env = prepare_env(run, smoke, drag)
    state = run / "state.txt"
    state.write_text(f"{source.resolve()}|0||0\n", encoding="utf-8")
    started = unix_us()
    process = subprocess.Popen(
        [str(exe), "--open", str(state)],
        cwd=str(run), env=env,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    result_file = smoke / "result.txt"
    error_file = smoke / "error.txt"
    outcome = wait_until(
        lambda: result_file if result_file.is_file() else (error_file if error_file.is_file() else None),
        timeout,
    )
    finished = unix_us()
    terminate_run(run, process)
    events, trace_files = collect_trace(run / "trace")
    open_event = first(events, "open_requested")
    anchor = open_event.get("started_unix_us") if open_event else None
    capture = first(events, "capture", name="00-loaded.png")
    capture_file = smoke / "first-100-capture-us.txt"
    if capture is None and capture_file.is_file():
        try:
            capture = {"stage": "capture", "name": "00-loaded.png",
                       "unix_us": int(capture_file.read_text().strip())}
        except ValueError:
            pass
    if anchor is None and (smoke / "open-start-us.txt").is_file():
        try:
            anchor = int((smoke / "open-start-us.txt").read_text().strip())
        except ValueError:
            pass
    ok, problems = validate_run(events, anchor_us=anchor)
    after = sha256(source)
    return {
        "format": fmt,
        "mode": "cold",
        "status": "ok" if outcome == result_file and ok else "failed",
        "problems": problems if not ok else ([] if outcome == result_file else ["smoke did not finish"]),
        "launch_started_unix_us": started,
        "launch_finished_unix_us": finished,
        "anchor_unix_us": anchor,
        "capture_unix_us": capture.get("unix_us") if capture else None,
        "t_full100_ms": ms_between(capture.get("unix_us") if capture else None, anchor),
        "smoke_result": result_file.read_text(encoding="utf-8", errors="replace").strip()
        if result_file.is_file() else None,
        "smoke_error": error_file.read_text(encoding="utf-8", errors="replace").strip()
        if error_file.is_file() else None,
        "trace_files": trace_files,
        "source_unchanged": sha256(fixture) == original_before,
        "private_sha256_after": after,
        "private_mutated": after != copy["private_sha256_before"],
        "expected_private_mutation": fmt == "dng",
        "copy": copy,
        "events": events,
    }


_FIXTURE_HASH_CACHE: dict[str, str] = {}


def fixture_sha(path: Path) -> str:
    key = str(path)
    if key not in _FIXTURE_HASH_CACHE:
        _FIXTURE_HASH_CACHE[key] = sha256(path)
    return _FIXTURE_HASH_CACHE[key]


def hex_path(path: Path) -> str:
    return path.as_posix().encode("utf-8").hex()


def write_request(directory: Path, source: Path) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    receipt = directory / "receipt.txt"
    request = directory / "request.txt"
    request.write_text(
        "FASTCULL_RAW_WINDOW_V1\n"
        + hex_path(source) + "\n"
        + hex_path(receipt) + "\n-\n-\n",
        encoding="utf-8",
    )
    return request


def run_reuse(exe: Path, fixture: Path, fmt: str, run: Path, timeout: float,
              count: int, drag: int = 0, dng_sidecar: bool = False) -> dict:
    original_before = sha256(fixture)
    env = prepare_env(run, run / "smoke", drag)
    env["FASTCULL_RAW_REUSE_SMOKE"] = "1"
    samples = []
    process = None
    for index in range(count):
        iteration = run / f"iter-{index}"
        photos = iteration / "photos"
        photos.mkdir(parents=True, exist_ok=True)
        source = photos / f"photo-{index}.{fmt}"
        copy = copy_fixture(fixture, source, fmt, dng_sidecar)
        request = write_request(iteration, source)
        submit = unix_us()
        if process is None:
            process = subprocess.Popen(
                [str(exe), "--raw-editor-server", str(request)],
                cwd=str(iteration), env=env,
                stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                text=True, encoding="utf-8",
            )
        else:
            process.stdin.write(request.as_posix().encode("utf-8").hex() + "\n")
            process.stdin.flush()
        smoke = run / "smoke" / source.stem
        result_file = smoke / "result.txt"
        error_file = smoke / "error.txt"
        outcome = wait_until(
            lambda: result_file if result_file.is_file() else (error_file if error_file.is_file() else None),
            timeout,
        )
        closed = wait_until(lambda: (iteration / "closed.txt").is_file(), 30)
        events, trace_files = collect_trace(run / "trace")
        # Segment this iteration: events after the submit, plus its own screenshots.
        segment = [
            event for event in events
            if event.get("unix_us", 0) >= submit - 50_000
        ]
        capture = first(segment, "capture", after=submit, name="00-loaded.png")
        ok, problems = validate_run(segment, anchor_us=submit)
        samples.append({
            "iteration": index,
            "warmup": index == 0,
            "status": "ok" if outcome == result_file and ok else "failed",
            "problems": problems if not ok else ([] if outcome == result_file else ["smoke did not finish"]),
            "submit_unix_us": submit,
            "capture_unix_us": capture.get("unix_us") if capture else None,
            "t_full100_ms": ms_between(capture.get("unix_us") if capture else None, submit),
            "closed": bool(closed),
            "smoke_result": result_file.read_text(encoding="utf-8", errors="replace").strip()
            if result_file.is_file() else None,
            "smoke_error": error_file.read_text(encoding="utf-8", errors="replace").strip()
            if error_file.is_file() else None,
            "events": segment,
            "trace_files": trace_files,
            "source_unchanged": sha256(fixture) == original_before,
            "private_sha256_after": sha256(source),
            "private_mutated": sha256(source) != copy["private_sha256_before"],
            "expected_private_mutation": fmt == "dng",
        })
        if outcome != result_file:
            break
    if process is not None:
        try:
            process.stdin.close()
        except OSError:
            pass
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            pass
        terminate_run(run, process)
    return {"format": fmt, "mode": "reuse", "samples": samples}


def drag_metrics(events: list[dict]) -> dict | None:
    """Replay evidence for T07: display-only drags must not decode."""
    inputs = [
        event for event in events
        if event.get("stage") == "edit_input" and event.get("kind") == "drag"
    ]
    final = first(events, "edit_input", kind="drag-final-wb")
    done = first(events, "drag_done")
    if not inputs and final is None:
        return None
    start = inputs[0].get("unix_us") if inputs else final.get("unix_us")
    end = done.get("unix_us") if done else (
        final.get("unix_us") if final else start)
    decodes = [
        event for event in events
        if event.get("stage") == "decode_done"
        and start <= event.get("unix_us", 0) <= end
    ]
    frames = [
        event for event in events
        if event.get("stage") == "display_frame"
        and start <= event.get("unix_us", 0) <= end
    ]
    last = inputs[-1] if inputs else None
    last_frame = frames[-1] if frames else None
    match = None
    if last and last_frame:
        try:
            exposure = abs(float(last_frame.get("exposure", "nan")) * 1_000_000.
                           - float(last.get("exposure_uev", "nan"))) <= 1_000.0
            basic = ",".join(
                str(int(round(float(value) * 100.)))
                for value in str(last_frame.get("basic", "")).split(",")
                if value
            ) == str(last.get("basic", ""))
            match = bool(exposure and basic)
        except ValueError:
            match = False
    exact_scene = None
    if final:
        exact_scene = next(
            (event for event in events
             if event.get("stage") == "scene_ready"
             and event.get("unix_us", 0) >= final.get("unix_us", 0)
             and event.get("wb_temperature") == 4850),
            None,
        )
    latencies = []
    for input_event in inputs:
        frame = next(
            (event for event in frames
             if event.get("unix_us", 0) > input_event.get("unix_us", 0)),
            None,
        )
        if frame:
            latencies.append(
                (frame["unix_us"] - input_event["unix_us"]) / 1000.0)
    latencies.sort()
    return {
        "inputs": len(inputs),
        "display_frames": len(frames),
        "decode_during_display_drags": len(decodes),
        "last_input_matches_last_display_frame": match,
        "final_wb_input": bool(final),
        "exact_scene_after_final_wb": bool(exact_scene),
        "drag_done": bool(done),
        "frame_latency_ms": {
            "n": len(latencies),
            "median": round(latencies[len(latencies) // 2], 3)
            if latencies else None,
            "max": round(latencies[-1], 3) if latencies else None,
        } if latencies else None,
    }


DISPATCH_FIELDS = (
    "entry", "ok", "decoder", "matched", "bridge_reason", "raw_width", "raw_height",
    "active_width", "active_height", "filters", "colors", "dng_version", "tiff_samples",
    "callback", "backend", "tiles", "samples", "h2d_bytes", "d2h_bytes", "entropy_ms",
    "fallback",
)


def dispatch_summary(events: list[dict]) -> dict | None:
    """Audit what the real edit path used for unpacking (Z03 dispatch evidence)."""
    dispatches = [event for event in events if event.get("stage") == "entropy_dispatch"]
    if not dispatches:
        return None
    first = dispatches[0]
    summary = {"count": len(dispatches)}
    summary.update({field: first.get(field) for field in DISPATCH_FIELDS})
    summary["all"] = [{field: event.get(field) for field in DISPATCH_FIELDS}
                      for event in dispatches]
    return summary


def metrics_from_events(events: list[dict], anchor_us: int | None) -> dict:
    if anchor_us is not None:
        events = [event for event in events if event.get("unix_us", 0) >= anchor_us]
    decode = first(events, "decode_done")
    quick = first(events, "quick_ready")
    full_scene = first(events, "scene_ready", full=True)
    scene = full_scene or first(events, "scene_ready")
    texture = None
    if scene:
        texture = (
            first(events, "texture_adopt", after=scene.get("unix_us"))
            or first(events, "texture_upload", after=scene.get("unix_us"))
            or first(events, "texture_adopt")
            or first(events, "texture_upload")
        )
    capture = first(events, "capture", name="00-loaded.png")
    edit = first(events, "edit_input")
    wb_marker = first(events, "wb_immediate")
    immediate = first(events, "capture", after=edit.get("unix_us") if edit else None,
                      name="01-immediate-wb.png") if edit else None
    edited = first(events, "capture", after=edit.get("unix_us") if edit else None,
                   name="01-edited.png") if edit else None
    next_frame = immediate or edited
    exact = None
    if edit:
        for event in events:
            if (event.get("stage") == "scene_ready"
                    and event.get("unix_us", 0) >= edit.get("unix_us", 0)
                    and event.get("wb_temperature") == 4850):
                exact = event
                break
    final_full = first(events, "capture", name="02-full.png")
    preload = first(events, "capture", name="00-preloaded-100.png")
    preload_switch = first(events, "preload_switch")
    unpack_us = None
    if decode and not decode.get("reused"):
        unpack_us = decode.get("unpack_done_unix_us")
    metrics = {
        "t_quick_ms": ms_between(quick.get("unix_us") if quick else None, anchor_us),
        "t_unpack_ms": ms_between(unpack_us, anchor_us),
        "t_scene_ms": ms_between(
            texture.get("unix_us") if texture else None, anchor_us),
        "t_visible100_ms": ms_between(
            capture.get("unix_us") if capture else None, anchor_us),
        "t_full100_ms": ms_between(
            capture.get("unix_us") if capture else None, anchor_us)
        if full_scene and capture else None,
        # Application scheduling marker (UI request to painted frame), not a
        # physical display latency and not the screenshot readback.
        "t_input_next_frame_ms": wb_marker.get("paint_ms") if wb_marker else None,
        "t_input_next_screenshot_ms": ms_between(
            next_frame.get("unix_us") if next_frame else None,
            edit.get("unix_us") if edit else None),
        "t_input_edited_ms": ms_between(
            edited.get("unix_us") if edited else None,
            edit.get("unix_us") if edit else None),
        "t_wb_immediate_ms": ms_between(
            immediate.get("unix_us") if immediate else None,
            edit.get("unix_us") if edit else None),
        "t_wb_exact_ms": ms_between(
            exact.get("unix_us") if exact else None,
            edit.get("unix_us") if edit else None),
        "t_preloaded_100_ms": ms_between(
            preload.get("unix_us") if preload else None,
            preload_switch.get("unix_us") if preload_switch else None),
        "t_final_full_ms": ms_between(
            final_full.get("unix_us") if final_full else None, anchor_us),
        "decode": {
            "reused": decode.get("reused") if decode else None,
            "open_ms": decode.get("open_ms") if decode else None,
            "unpack_ms": decode.get("unpack_ms") if decode else None,
            "process_ms": decode.get("process_ms") if decode else None,
            "file_bytes": decode.get("file_bytes") if decode else None,
        },
        "scene": {
            "cached": full_scene.get("cached") if full_scene else None,
            "ms": full_scene.get("ms") if full_scene else None,
            "resident": full_scene.get("resident") if full_scene else None,
            "width": full_scene.get("width") if full_scene else None,
            "height": full_scene.get("height") if full_scene else None,
        },
        "capture": bool(capture),
        "full_scene": bool(full_scene),
        "texture": bool(texture),
        "counters_at_scene": {
            key: full_scene.get(key) for key in
            ("io_bytes", "h2d_bytes", "d2h_bytes", "copies")
        } if full_scene else None,
        "drag": drag_metrics(events),
        "dispatch": dispatch_summary(events),
    }
    return metrics


def summarize(values: list[float]) -> dict:
    values = sorted(v for v in values if v is not None)
    if not values:
        return {"n": 0, "median_ms": None, "min_ms": None, "max_ms": None,
                "p95_ms": None, "p95_established": False}
    n = len(values)
    median = values[n // 2] if n % 2 else (values[n // 2 - 1] + values[n // 2]) / 2
    p95 = values[min(n - 1, int(round(0.95 * (n - 1))))]
    return {
        "n": n,
        "median_ms": round(median, 3),
        "min_ms": values[0],
        "max_ms": values[-1],
        "p95_ms": p95,
        "p95_established": n >= 50,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--exe", type=Path,
                        default=ROOT / "lrplugins/FastCull.lrplugin/fastcull.exe")
    parser.add_argument("--data-root", type=Path)
    parser.add_argument("--fixtures", type=Path,
                        help="JSON list of {format, raw} entries")
    parser.add_argument("--out", type=Path)
    parser.add_argument("--formats", default="arw,nef,dng")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--mode", choices=["cold", "reuse", "both"], default="both")
    parser.add_argument("--timeout", type=float, default=240)
    parser.add_argument("--drag", type=int, default=0,
                        help="run FASTCULL_RAW_DRAG_SMOKE with N display-only inputs")
    parser.add_argument("--dng-sidecar", action="store_true",
                        help="write the controlled XMP sidecar for private DNG copies")
    parser.add_argument("--smoke-delay-ms", type=int, default=None,
                        help="wait this long after the photo loads before auto-opening the editor")
    parser.add_argument("--prewarm-delay-ms", type=int, default=None,
                        help="idle delay before prewarming the session editor (default 800)")
    parser.add_argument("--no-prewarm", action="store_true",
                        help="disable the session editor prewarm (cold comparison)")
    parser.add_argument("--entropy-mode", choices=["default", "parallel", "gpu", "cpu"],
                        default="default",
                        help="FASTCULL_RAW_ENTROPY override for the tile-entropy A/B "
                             "(cpu disables the tile path and keeps LibRaw unpack)")
    parser.add_argument("--ahd-warmup", choices=["default", "on", "off"], default="default",
                        help="FASTCULL_RAW_AHD_WARMUP override; separates the GPU AHD shader "
                             "warmup from the session-editor prewarm")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--manifest-only", action="store_true")
    args = parser.parse_args()
    args.formats = [part.strip().lower() for part in args.formats.split(",") if part.strip()]
    ENV_OPTIONS.update({
        "no_prewarm": args.no_prewarm,
        "prewarm_delay_ms": args.prewarm_delay_ms,
        "smoke_delay_ms": args.smoke_delay_ms,
        "entropy_mode": args.entropy_mode,
        "ahd_warmup": args.ahd_warmup,
    })
    if args.self_test:
        return self_test()
    manifest = source_manifest()
    drift = baseline_drift(manifest)
    environment_info = environment()
    if args.manifest_only:
        payload = {
            "schema": REPORT_SCHEMA,
            "event_schema": EVENT_SCHEMA,
            "generated_at": stamp(),
            "source_manifest": manifest,
            "baseline_check": drift,
            "environment": environment_info,
        }
        text = json.dumps(payload, ensure_ascii=False, indent=2)
        print(text)
        return 0
    fixtures = resolve_fixtures(args)
    out = (args.out or ROOT / f".agent-temp/{time.strftime('%Y-%m-%d')}-raw-bench/{time.strftime('%H%M%S')}").resolve()
    out.mkdir(parents=True, exist_ok=True)
    args.exe = args.exe.resolve()
    if not args.exe.is_file():
        print(f"missing exe: {args.exe}", file=sys.stderr)
        return 2
    report = {
        "schema": REPORT_SCHEMA,
        "event_schema": EVENT_SCHEMA,
        "generated_at": stamp(),
        "task": "T00",
        "description": "First-full RAW benchmark: cold culler open and reused editor IPC",
        "exe": {
            "path": str(args.exe),
            "sha256": sha256(args.exe),
            "dll_sha256": sha256(args.exe.with_name("fastcull_raw.dll"))
            if args.exe.with_name("fastcull_raw.dll").is_file() else None,
        },
        "source_manifest": manifest,
        "baseline_check": drift,
        "environment": environment_info,
        "fixtures": fixture_manifest(fixtures),
        "recipe": {
            "arw": "Adobe Standard, +1 EV, sharpness/NR disabled, WB 5500/0",
            "nef": "Adobe Standard, +1 EV, sharpness/NR disabled, WB 5500/0",
            "dng": "embedded Adobe Color +0.75 EV, no extra sidecar",
        },
        "drag_inputs": args.drag,
        "dng_sidecar": args.dng_sidecar,
        "smoke_delay_ms": args.smoke_delay_ms,
        "prewarm_delay_ms": args.prewarm_delay_ms,
        "prewarm": not args.no_prewarm,
        "entropy_mode": args.entropy_mode,
        "ahd_warmup": args.ahd_warmup,
        "runs": [],
        "summary": {},
    }
    for fmt, fixture in fixtures.items():
        for index in range(args.runs):
            if args.mode in ("cold", "both"):
                run = out / "cold" / fmt / f"run-{index}"
                run.mkdir(parents=True, exist_ok=True)
                print(f"cold {fmt} run {index + 1}/{args.runs} ...", flush=True)
                record = run_cold(args.exe, fixture, fmt, run, args.timeout,
                                  args.drag, args.dng_sidecar)
                record["metrics"] = metrics_from_events(record.pop("events"), record["anchor_unix_us"])
                report["runs"].append(record)
                print(f"  t_full100={record['t_full100_ms']} ms "
                      f"status={record['status']}", flush=True)
        if args.mode in ("reuse", "both"):
            run = out / "reuse" / fmt
            run.mkdir(parents=True, exist_ok=True)
            print(f"reuse {fmt}: one warmup + {args.runs} measured opens ...", flush=True)
            record = run_reuse(args.exe, fixture, fmt, run, args.timeout,
                               args.runs + 1, args.drag, args.dng_sidecar)
            for sample in record["samples"]:
                sample["metrics"] = metrics_from_events(
                    sample.pop("events"), sample["submit_unix_us"])
                report["runs"].append({**sample, "format": fmt, "mode": "reuse"})
                print(f"  {sample['iteration']}{' (warmup)' if sample['warmup'] else ''}"
                      f" t_full100={sample['t_full100_ms']} ms "
                      f"status={sample['status']}", flush=True)
    summary = {}
    for mode in ("cold", "reuse"):
        for fmt in fixtures:
            values = [
                run.get("t_full100_ms") for run in report["runs"]
                if run.get("mode") == mode and run.get("format") == fmt
                and not run.get("warmup")
            ]
            summary[f"{mode}/{fmt}"] = summarize(values)
    report["summary"] = summary
    report_path = out / "report.json"
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2),
                           encoding="utf-8")
    print(json.dumps({"report": str(report_path), "summary": summary},
                     ensure_ascii=False, indent=2))
    failures = [run for run in report["runs"] if run.get("status") != "ok"]
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
