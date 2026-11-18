#!/usr/bin/env python3
"""Check the real native entropy fallback/cancellation ABI on a private RAW copy.

No camera data is written. DLL allocations are released before each case.
This is an interface/correctness check, not a speed or Lightroom benchmark.
"""
import argparse
import ctypes as c
import hashlib
import json
import os
from pathlib import Path


class EntropyV1(c.Structure):
    _fields_ = [(name, c.c_uint32) for name in ("abi", "kind", "width", "height", "channels")]


class Frame(c.Structure):
    _fields_ = [(name, c.c_uint32) for name in (
        "kind", "width", "height", "channels", "bits", "rendered")] + [
        ("length", c.c_uint64), ("data", c.c_void_p), ("owner", c.c_void_p)]


class Dispatch(c.Structure):
    _fields_ = [(name, c.c_uint32) for name in (
        "abi", "matched", "reason", "callback", "raw_width", "raw_height",
        "active_width", "active_height", "filters", "colors", "dng_version",
        "tiff_samples", "shot_select", "raw_pitch")] + [("decoder", c.c_char * 32)]


Cancel = c.CFUNCTYPE(c.c_int, c.c_void_p)
EntropyFn = c.CFUNCTYPE(c.c_int, c.c_void_p, c.POINTER(c.c_uint16), c.POINTER(EntropyV1))


class Params(c.Structure):
    _fields_ = [("abi", c.c_uint32), ("edge", c.c_uint32),
                ("temperature", c.c_double), ("tint", c.c_double),
                ("exposure", c.c_double), ("cancelled", Cancel), ("context", c.c_void_p)]


def check(dll_path: Path, raw_path: Path) -> dict:
    directory = os.add_dll_directory(str(dll_path.parent)) if os.name == "nt" else None
    try:
        dll = c.CDLL(str(dll_path))
        develop = dll.fc_raw_develop_linear_compute
        path_type = c.c_wchar_p if os.name == "nt" else c.c_char_p
        develop.argtypes = [path_type, c.POINTER(Params), c.POINTER(Frame), c.c_void_p,
                            c.c_void_p, c.c_void_p, c.c_void_p, EntropyFn, c.c_void_p]
        develop.restype = c.c_int
        dll.fc_raw_release.argtypes = [c.c_void_p]
        dll.fc_raw_develop_idle.argtypes = []
        dll.fc_raw_develop_idle.restype = None
        dll.fc_raw_last_unpack_dispatch.argtypes = [c.POINTER(Dispatch)]
        dll.fc_raw_last_unpack_dispatch.restype = c.c_int
        cases = []
        for name, callback_status, expected_reason in (("declined", 0, 5), ("cancelled", -1, 6)):
            dll.fc_raw_develop_idle()
            seen = []

            @Cancel
            def cancel(_):
                return 0

            @EntropyFn
            def entropy(_, pixels, params):
                p = params.contents  # The old host reads exactly the 20-byte ABI v1.
                seen.append({key: getattr(p, key) for key, _ in EntropyV1._fields_})
                return callback_status if p.abi == 1 else 0

            params = Params(1, 0, 0, 0, 0, cancel, None)
            frame, info, error = Frame(), c.create_string_buffer(320), c.create_string_buffer(256)
            path = str(raw_path) if os.name == "nt" else os.fsencode(raw_path)
            status = develop(path, c.byref(params), c.byref(frame), info, error,
                             None, None, entropy, None)
            record = Dispatch()
            available = dll.fc_raw_last_unpack_dispatch(c.byref(record))
            row = {"case": name, "status": status, "error": error.value.decode("utf-8", "replace"),
                   "callback": list(seen), "dispatch_available": available,
                   "dispatch": {key: getattr(record, key) for key in (
                       "abi", "matched", "reason", "callback", "raw_width", "raw_height")},
                   "frame": {key: getattr(frame, key) for key in ("width", "height", "bits")}}
            if frame.data and frame.length:
                row["linear_frame_sha256"] = hashlib.sha256(
                    c.string_at(frame.data, frame.length)).hexdigest()
            if frame.owner:
                dll.fc_raw_release(frame.owner)
            row["ok"] = bool(available and seen and seen[0]["abi"] == 1
                             and record.reason == expected_reason and record.callback == 1
                             and ((status == 0) if name == "declined" else (status != 0)))
            cases.append(row)
            if name == "declined":
                seen.clear()
                reused_frame = Frame()
                reused_status = develop(path, c.byref(params), c.byref(reused_frame), info,
                                        error, None, None, entropy, None)
                reused_record = Dispatch()
                valid = dll.fc_raw_last_unpack_dispatch(c.byref(reused_record))
                reused = {"case": "cached-unpack", "status": reused_status,
                          "callback_count": len(seen), "reason": reused_record.reason,
                          "ok": bool(valid and reused_status == 0 and not seen
                                     and reused_record.reason == 4)}
                if reused_frame.data and reused_frame.length:
                    reused["linear_frame_sha256"] = hashlib.sha256(
                        c.string_at(reused_frame.data, reused_frame.length)).hexdigest()
                    reused["ok"] = bool(reused["ok"] and reused["linear_frame_sha256"]
                                        == row["linear_frame_sha256"])
                if reused_frame.owner:
                    dll.fc_raw_release(reused_frame.owner)
                cases.append(reused)
                missing = raw_path.with_name("missing-dispatch-check.arw")
                if missing.exists():
                    raise ValueError("negative test path unexpectedly exists")
                missing_path = str(missing) if os.name == "nt" else os.fsencode(missing)
                empty = Frame()
                failed = develop(missing_path, c.byref(params), c.byref(empty), info,
                                 error, None, None, entropy, None)
                stale = dll.fc_raw_last_unpack_dispatch(c.byref(Dispatch()))
                cases.append({"case": "early-failure-clears-stale-dispatch", "status": failed,
                              "dispatch_available": stale,
                              "ok": bool(failed != 0 and not stale and not empty.owner)})
        dll.fc_raw_develop_idle()
        return {"schema": "fastcull.native_dispatch.check.v1", "ok": all(x["ok"] for x in cases),
                "dll_sha256": hashlib.sha256(dll_path.read_bytes()).hexdigest(),
                "raw_sha256": hashlib.sha256(raw_path.read_bytes()).hexdigest(), "cases": cases}
    finally:
        if directory:
            directory.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dll", type=Path, required=True)
    parser.add_argument("--raw", type=Path, required=True)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    result = check(args.dll.resolve(), args.raw.resolve())
    text = json.dumps(result, ensure_ascii=False, indent=2)
    if args.report:
        args.report.write_text(text + "\n", encoding="utf-8")
    print(text)
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
