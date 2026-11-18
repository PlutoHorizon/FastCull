#!/usr/bin/env python3
"""Build the ACR diagnostic reference manifest from the 2026-11-03 record.

This is deliberately *not* a Lightroom acceptance set. It records the old
Photoshop/ACR exports and their historical FastCull candidates so the T01
differential tooling can be exercised and regressions detected. A target-LR
manifest must be collected separately with `raw_reference_eval.py verify
--strict-lr`.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "docs/acr-color-validation-2026-11-03.json"
SCHEMA = "fastcull.raw_reference.manifest.v1"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def default_rois() -> list[dict]:
    return [
        {"name": "all", "role": "general", "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0},
        {"name": "center", "role": "general", "x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5},
    ]


def build(source: Path) -> dict:
    record = json.loads(source.read_text(encoding="utf-8"))
    cases = []
    problems = []
    for row in record["rows"]:
        folder = Path(row["folder"])
        if not folder.is_absolute():
            folder = ROOT / folder
        raw = folder / f"private.{row['format']}"
        reference = folder / "acr.png"
        candidate = folder / "fastcull.png"
        if not raw.is_file():
            problems.append(f"{row['name']}: missing {raw}")
            continue
        if not reference.is_file():
            problems.append(f"{row['name']}: missing {reference}")
            continue
        response = row["acr_response"].split("\t")
        width = int(response[1]) if len(response) > 2 else None
        height = int(response[2]) if len(response) > 2 else None
        entry = {
            "id": row["name"],
            "kind": "acr_diagnostic",
            "format": row["format"],
            "raw": {
                "path_hint": raw.relative_to(ROOT).as_posix(),
                "sha256": sha256(raw),
            },
            "reference": {
                "path": reference.relative_to(ROOT).as_posix(),
                "sha256": sha256(reference),
                "output": {
                    "width": width,
                    "height": height,
                    "bit_depth": 8,
                    "icc": response[3] if len(response) > 3 else "sRGB IEC61966-2.1",
                    "note": "historical comparison export, longest edge 1280",
                },
            },
            "params": row["requested"],
            "effective": row["effective_acr"],
            "rois": default_rois(),
            "historical_metrics": row.get("metrics"),
            "provenance": {
                "photoshop": record.get("photoshop"),
                "acr": record.get("acr"),
                "process_version": record.get("process_version"),
                "date": record.get("date"),
            },
        }
        if candidate.is_file():
            entry["candidate"] = {
                "path": candidate.relative_to(ROOT).as_posix(),
                "sha256": sha256(candidate),
            }
        cases.append(entry)
    return {
        "schema": SCHEMA,
        "target": {
            "kind": "acr_diagnostic",
            "application": "Adobe Photoshop Camera Raw",
            "acr": record.get("acr"),
            "photoshop": record.get("photoshop"),
            "process_version": record.get("process_version"),
            "acceptance": "diagnostic only; not the user's Lightroom Classic",
        },
        "source_record": source.relative_to(ROOT).as_posix(),
        "cases": cases,
        "problems": problems,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=SOURCE)
    parser.add_argument("--out", type=Path,
                        default=ROOT / "docs/acr-reference-manifest-2026-11-03.json")
    args = parser.parse_args()
    manifest = build(args.source)
    args.out.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
                        encoding="utf-8")
    print(json.dumps({
        "out": str(args.out),
        "cases": len(manifest["cases"]),
        "problems": manifest["problems"],
    }, ensure_ascii=False, indent=2))
    return 1 if manifest["problems"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
