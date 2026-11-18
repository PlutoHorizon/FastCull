"""Contract tests for the T01 reference manifest, strict verification and metrics.

The strict-mode cases come from the 2026-11-12 findings: missing assets, a
missing effective object/key and a fake 16-bit/ICC declaration used to pass
silently. Each negative case must fail with a specific problem in strict mode
and stay a warning in diagnostic mode.
"""
import hashlib
import importlib.util
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/raw_reference_eval.py"
spec = importlib.util.spec_from_file_location("reference_eval", SCRIPT)
reference_eval = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference_eval)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def synthetic_icc(description: str) -> bytes:
    """Minimal ICC with a parseable 'desc' tag; enough to classify, not to convert."""
    text = description.encode("ascii")
    payload = b"desc" + b"\0" * 4 + struct.pack(">I", len(text) + 1) + text + b"\0"
    while len(payload) % 4:
        payload += b"\0"
    entry = b"desc" + struct.pack(">II", 132 + 12, len(payload))
    body = struct.pack(">I", 1) + entry + payload
    header = bytearray(128)
    struct.pack_into(">I", header, 0, 128 + len(body))
    header[36:40] = b"acsp"
    return bytes(header) + body


def write_png(path: Path, *, bit_depth: int = 8, width: int = 60, height: int = 40,
              profile: bytes | None = None) -> None:
    from PIL import Image

    if bit_depth == 16:
        values = (np.arange(width * height, dtype=np.uint16).reshape(height, width) * 977)
        image = Image.fromarray(values)
    else:
        values = (np.arange(width * height, dtype=np.uint16).reshape(height, width) % 256)
        image = Image.fromarray(values.astype(np.uint8))
    image.save(path, icc_profile=profile) if profile else image.save(path)


class ReferenceEvalTests(unittest.TestCase):
    def test_identity_and_white_excess_are_detected(self):
        reference = np.full((8, 8, 3), 128.0, np.float32)
        candidate = reference.copy()
        candidate[0, 0] = 255.0
        reference[0, 0] = 200.0
        metrics = reference_eval.compare_images(
            reference, candidate, reference_eval.default_rois())
        all_roi = next(roi for roi in metrics["rois"] if roi["name"] == "all")
        self.assertGreater(all_roi["excess_white_percent"], 0.0)
        self.assertLess(all_roi["excess_white_percent"], 10.0)
        identical = reference_eval.compare_images(
            reference, reference, reference_eval.default_rois())
        for roi in identical["rois"]:
            self.assertLess(roi["delta_e00_max"], 1e-6)
            self.assertLess(roi["rgb_mae"], 1e-6)

    def test_delta_e00_known_ranges(self):
        white = np.full((1, 3), 255.0, np.float32)
        black = np.zeros((1, 3), np.float32)
        value = reference_eval.delta_e00(
            reference_eval.srgb_to_lab(white), reference_eval.srgb_to_lab(black))[0]
        self.assertGreater(value, 95.0)
        self.assertLess(value, 105.0)
        gray = np.full((1, 3), 128.0, np.float32)
        value = reference_eval.delta_e00(
            reference_eval.srgb_to_lab(gray), reference_eval.srgb_to_lab(gray))[0]
        self.assertLess(value, 1e-6)

    def test_delta_e00_matches_published_vectors(self):
        """Sharma et al. CIEDE2000 verification data (Lab pairs, independent source)."""
        vectors = [
            ((50.0, 2.6772, -79.7751), (50.0, 0.0, -82.7485), 2.0425),
            ((50.0, 3.1571, -77.2803), (50.0, 0.0, -82.7485), 2.8615),
            ((50.0, -1.3802, -84.2814), (50.0, 0.0, -82.7485), 1.0000),
            ((50.0, 2.5, 0.0), (50.0, 0.0, -2.5), 4.3065),
            ((60.2574, -34.0099, 36.2677), (60.4626, -34.1751, 39.4387), 1.2644),
        ]
        for lab1, lab2, expected in vectors:
            value = reference_eval.delta_e00(
                np.array([lab1], dtype=np.float64), np.array([lab2], dtype=np.float64))[0]
            self.assertAlmostEqual(expected, float(value), places=4)

    def test_manifest_tamper_and_effective_parameter_alarm(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / "input.arw"
            raw.write_bytes(b"raw")
            reference = root / "ref.png"
            reference.write_bytes(b"png")
            manifest = {
                "schema": reference_eval.MANIFEST_SCHEMA,
                "cases": [{
                    "id": "case",
                    "kind": "lightroom",
                    "format": "arw",
                    "raw": {"path_hint": str(raw), "sha256": reference_eval.sha256(raw)},
                    "reference": {
                        "path": str(reference),
                        "sha256": reference_eval.sha256(reference),
                        "output": {"icc": "sRGB", "bit_depth": 8, "width": 10, "height": 10},
                    },
                    "params": {"Exposure2012": 1},
                    "effective": {"Exposure2012": "2"},
                    "lightroom": {"version": "14", "build": "1", "process_version": "11.0"},
                }],
            }
            ok, problems, _ = reference_eval.verify_manifest(manifest, strict_lr=True)
            self.assertFalse(ok)
            self.assertTrue(any("requested Exposure2012" in problem for problem in problems))
            reference.write_bytes(b"changed")
            manifest["cases"][0]["effective"] = {"Exposure2012": "1"}
            ok, problems, _ = reference_eval.verify_manifest(manifest, strict_lr=True)
            self.assertFalse(ok)
            self.assertTrue(any("reference hash drifted" in problem for problem in problems))

    def test_manifest_rejects_non_lightroom_in_strict_mode(self):
        manifest = {
            "schema": reference_eval.MANIFEST_SCHEMA,
            "cases": [{
                "id": "diag", "kind": "acr_diagnostic", "format": "arw",
                "raw": {"sha256": "0" * 64},
                "reference": {"sha256": "0" * 64},
                "params": {},
            }],
        }
        ok, problems, _ = reference_eval.verify_manifest(manifest, strict_lr=True)
        self.assertFalse(ok)
        self.assertTrue(any("not 'lightroom'" in problem for problem in problems))


class StrictVerificationTests(unittest.TestCase):
    """The 2026-11-12 negative cases must fail; a complete case must pass."""

    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="fastcull-ref-"))
        self.addCleanup(shutil.rmtree, self.directory, True)
        self.raw = self.directory / "scene.arw"
        self.raw.write_bytes(b"raw-bytes")
        self.xmp = self.directory / "scene.xmp"
        self.xmp.write_bytes(b"<x:xmpmeta/>")
        self.ref16 = self.directory / "ref16.png"
        write_png(self.ref16, bit_depth=16)
        self.ref8 = self.directory / "ref8.png"
        write_png(self.ref8, bit_depth=8)

    def case(self, replace=None, **overrides):
        case = {
            "id": "case",
            "kind": "lightroom",
            "format": "arw",
            "raw": {"path_hint": self.raw.name, "sha256": digest(self.raw)},
            "xmp": {"path_hint": self.xmp.name, "sha256": digest(self.xmp)},
            "lightroom": {"version": "14.5", "build": "20250701",
                          "process_version": "11.0"},
            "params": {"Exposure2012": 0, "CameraProfile": "Adobe Standard"},
            "effective": {"Exposure2012": "0.00", "CameraProfile": "Adobe Standard"},
            "reference": {
                "path": self.ref16.name,
                "sha256": digest(self.ref16),
                "output": {"icc": "sRGB", "bit_depth": 16, "width": 60, "height": 40},
            },
        }
        for key, value in overrides.items():
            if value is None:
                case.pop(key, None)
            elif isinstance(value, dict) and isinstance(case.get(key), dict):
                case[key] = {**case[key], **value}
            else:
                case[key] = value
        for key, value in (replace or {}).items():
            if value is None:
                case.pop(key, None)
            else:
                case[key] = value
        return case

    def manifest(self, case=None, **extra):
        return {"schema": reference_eval.MANIFEST_SCHEMA, "cases": [case or self.case()],
                **extra}

    def verify(self, case=None, strict=True, manifest_path=None, data_root=None,
               **extra):
        report = reference_eval.verify_manifest_report(
            self.manifest(case, **extra), strict_lr=strict, manifest_path=manifest_path,
            data_root=data_root, project_root=self.directory)
        return report

    def test_complete_case_is_accepted(self):
        report = self.verify()
        self.assertEqual([], report["problems"])
        self.assertTrue(report["schema_ok"])
        self.assertTrue(report["metadata_ok"])
        self.assertTrue(report["assets_hashed"])
        self.assertTrue(report["assets_verified"])
        self.assertTrue(report["accepted"])
        self.assertEqual("assets_verified", report["cases"][0]["state"])

    def test_true_16_bit_file_loads_at_16_bit(self):
        rgb, meta = reference_eval.load_rgb(self.ref16)
        self.assertEqual(16, meta["bit_depth"])
        self.assertTrue(meta["precision_preserved"])
        self.assertEqual((40, 60, 3), rgb.shape)

    def test_non_srgb_rgb8_conversion_cannot_claim_16_bit_precision(self):
        profile = {"present": True, "family": "AdobeRGB", "description": "Adobe RGB",
                   "bytes": b"profile"}
        with mock.patch.object(reference_eval, "embedded_icc", return_value=profile), \
             mock.patch.object(reference_eval, "convert_to_srgb", return_value=(
                 np.zeros((40, 60, 3), np.float32), "Pillow ImageCms RGB8")):
            rgb, meta = reference_eval.load_rgb(self.ref16, icc=True)
            case = self.case(reference={"output": {
                "icc": "AdobeRGB", "bit_depth": 16, "width": 60, "height": 40}})
            report = self.verify(case)
        self.assertEqual((40, 60, 3), rgb.shape)
        self.assertFalse(meta["precision_preserved"])
        self.assertEqual(8, meta["bit_depth"])
        self.assertFalse(report["accepted"])

    def test_invalid_profile_named_srgb_is_not_accepted_by_its_name(self):
        profile = {"present": True, "family": "sRGB", "description": "sRGB",
                   "bytes": synthetic_icc("sRGB")}
        with mock.patch.object(reference_eval, "embedded_icc", return_value=profile):
            report = self.verify()
        self.assertFalse(report["accepted"])
        self.assertTrue(any("ICC" in p for p in report["problems"]))

    def test_real_srgb_profile_keeps_16_bit_samples(self):
        from PIL import ImageCms
        profile = {"present": True, "family": "sRGB", "description": "sRGB",
                   "bytes": ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()}
        expected, _ = reference_eval.load_rgb(self.ref16)
        with mock.patch.object(reference_eval, "embedded_icc", return_value=profile):
            rgb, meta = reference_eval.load_rgb(self.ref16, icc=True)
            report = self.verify()
        np.testing.assert_array_equal(expected, rgb)
        self.assertTrue(meta["precision_preserved"])
        self.assertTrue(report["accepted"])

    def test_rgba_load_ignores_alpha_without_reordering_rgb(self):
        import cv2
        for dtype, scale in ((np.uint8, 1), (np.uint16, 257)):
            with self.subTest(dtype=dtype):
                path = self.directory / "rgba.png"
                bgra = np.tile(np.array([30, 20, 10, 255], dtype=dtype) * scale,
                               (40, 60, 1))
                self.assertTrue(cv2.imwrite(str(path), bgra))
                rgb, meta = reference_eval.load_rgb(path, icc=True)
                np.testing.assert_allclose(rgb[0, 0], [10, 20, 30], atol=1e-5)

    def test_candidate_color_errors_fail_comparison(self):
        for family in ("unknown", "conversion-failed", "precision-loss"):
            with self.subTest(family=family):
                self._candidate_color_error(family)

    def _candidate_color_error(self, kind):
        from PIL import Image
        path = self.directory / "candidate.png"
        Image.new("RGB", (60, 40), (10, 20, 30)).save(path)
        ref = self.directory / "reference.png"
        Image.new("RGB", (60, 40), (10, 20, 30)).save(ref)
        case = self.case(reference={"path": ref.name, "sha256": digest(ref),
                                   "output": {"icc": "sRGB", "bit_depth": 8,
                                              "width": 60, "height": 40}})
        if kind == "precision-loss":
            path = self.ref16
        case["candidate"] = {"path": path.name}
        manifest = self.directory / "candidate-manifest.json"
        manifest.write_text(json.dumps(self.manifest(case)), encoding="utf-8")
        original = reference_eval.load_rgb

        def load(candidate, **kwargs):
            rgb, meta = original(candidate, **kwargs)
            if Path(candidate) == path:
                if kind == "precision-loss":
                    meta["bit_depth"] = 8  # A loader can reduce a 16-bit container.
                else:
                    meta["icc"] = {"present": True, "family": None if kind == "unknown"
                                   else "AdobeRGB", "description": "Unknown RGB"}
                    if kind == "conversion-failed":
                        meta["icc_conversion_error"] = "cannot build ICC transform"
            return rgb, meta

        from contextlib import redirect_stdout
        import io
        output = io.StringIO()
        with mock.patch.object(reference_eval, "load_rgb", side_effect=load), \
             mock.patch.object(sys, "argv", [str(SCRIPT), "compare", str(manifest),
                                             "--strict-lr"]), redirect_stdout(output):
            status = reference_eval.main()
        self.assertEqual(1, status)
        payload = json.loads(output.getvalue())
        self.assertEqual([], payload["cases"])
        self.assertIn("precision" if kind == "precision-loss" else "ICC",
                      payload["missing"][0]["reason"])

    def test_strict_rejects_findings_case(self):
        """The exact 2026-11-12 manifest that used to pass with assets missing."""
        case = {
            "id": "missing-assets-and-effective",
            "kind": "lightroom",
            "format": "arw",
            "raw": {"path_hint": "does-not-exist.raw", "sha256": "0" * 64},
            "reference": {
                "path": "does-not-exist.png",
                "sha256": "1" * 64,
                "output": {"icc": "sRGB", "bit_depth": 16, "width": 10, "height": 10},
            },
            "params": {"Exposure2012": 1},
            "effective": {},
            "lightroom": {"version": "14", "build": "1", "process_version": "11.0"},
        }
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertFalse(report["assets_verified"])
        problems = "\n".join(report["problems"])
        self.assertIn("effective is missing requested key Exposure2012", problems)
        self.assertIn("raw file missing", problems)
        self.assertIn("reference file missing", problems)
        self.assertIn("invalid", report["cases"][0]["state"])

    def test_strict_rejects_missing_raw_path(self):
        case = self.case(raw={"path_hint": None, "path": None, "sha256": digest(self.raw)})
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("raw path not given" in problem for problem in report["problems"]))

    def test_strict_rejects_missing_reference_path(self):
        case = self.case(reference={"path": None, "path_hint": None,
                                    "sha256": digest(self.ref16)})
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("reference path not given" in problem
                            for problem in report["problems"]))

    def test_strict_rejects_missing_effective_object(self):
        case = self.case(effective=None)
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("effective object missing" in problem
                            for problem in report["problems"]))

    def test_strict_rejects_missing_effective_key(self):
        case = self.case(replace={"effective": {"Exposure2012": "0.00"}})
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("effective is missing requested key CameraProfile" in problem
                            for problem in report["problems"]))

    def test_strict_rejects_invalid_and_missing_digest(self):
        for value in ("not-a-digest", None):
            case = self.case(raw={"sha256": value})
            report = self.verify(case)
            self.assertFalse(report["accepted"])
            self.assertTrue(any("raw hash" in problem for problem in report["problems"]),
                            report["problems"])

    def test_strict_rejects_dimension_and_bit_depth_lies(self):
        case = self.case(reference={"output": {"width": 100, "height": 99}})
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        problems = "\n".join(report["problems"])
        self.assertIn("width 60 != declared 100", problems)
        self.assertIn("height 40 != declared 99", problems)
        case = self.case(reference={"path": self.ref8.name, "sha256": digest(self.ref8)})
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("8-bit but 16-bit was declared" in problem
                            for problem in report["problems"]), report["problems"])

    def test_strict_rejects_undeclared_non_srgb_icc(self):
        profile = synthetic_icc("Adobe RGB (1998)")
        path = self.directory / "adobe.png"
        write_png(path, bit_depth=8, profile=profile)
        case = self.case(reference={
            "path": path.name, "sha256": digest(path),
            "output": {"icc": "sRGB", "bit_depth": 8, "width": 60, "height": 40},
        })
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("embedded ICC is AdobeRGB" in problem
                            for problem in report["problems"]), report["problems"])

    def test_non_srgb_claim_without_profile_or_conversion_is_rejected(self):
        case = self.case(reference={"output": {"icc": "ProPhoto RGB"}})
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("no embedded profile" in problem for problem in report["problems"]),
                        report["problems"])
        profile = synthetic_icc("Adobe RGB (1998)")
        path = self.directory / "adobe-conv.png"
        write_png(path, bit_depth=8, profile=profile)
        case = self.case(reference={
            "path": path.name, "sha256": digest(path),
            "output": {"icc": "Adobe RGB (1998)", "bit_depth": 8, "width": 60, "height": 40},
        })
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("ICC conversion failed" in problem for problem in report["problems"]),
                        report["problems"])

    @unittest.skipUnless(Path(os.environ.get(
        "FASTCULL_SYSTEM_ADOBE_RGB", r"C:\Windows\System32\spool\drivers\color\AdobeRGB1998.icc"
    )).is_file(), "no system Adobe RGB profile")
    def test_real_adobe_rgb_profile_is_converted_not_assumed(self):
        profile = Path(os.environ.get(
            "FASTCULL_SYSTEM_ADOBE_RGB",
            r"C:\Windows\System32\spool\drivers\color\AdobeRGB1998.icc")).read_bytes()
        path = self.directory / "adobe-real.png"
        write_png(path, bit_depth=8, profile=profile)
        case = self.case(reference={
            "path": path.name, "sha256": digest(path),
            "output": {"icc": "Adobe RGB (1998)", "bit_depth": 8, "width": 60, "height": 40},
        })
        report = self.verify(case)
        self.assertEqual([], report["problems"])
        self.assertTrue(report["accepted"])
        rgb, meta = reference_eval.load_rgb(path, icc=True)
        self.assertIn("icc_conversion", meta)
        self.assertEqual("AdobeRGB", meta["icc"]["family"])

    def test_numeric_equivalence_does_not_alarm(self):
        case = self.case(replace={
            "params": {"Exposure2012": 0, "Tint": 0.0, "Whites2012": "+0"},
            "effective": {"Exposure2012": "0.00", "Tint": "0", "Whites2012": 0},
        })
        report = self.verify(case)
        self.assertEqual([], report["problems"])
        self.assertTrue(report["accepted"])
        case = self.case(replace={"params": {"Exposure2012": 0},
                                  "effective": {"Exposure2012": "0.5"}})
        report = self.verify(case)
        self.assertTrue(any("requested Exposure2012" in problem
                            for problem in report["problems"]))

    def test_strict_requires_config_fingerprint(self):
        case = self.case(xmp=None)
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("no config fingerprint" in problem for problem in report["problems"]))
        case = self.case(xmp={"sha256": "z" * 64})
        report = self.verify(case)
        self.assertFalse(report["accepted"])
        self.assertTrue(any("not a 64-hex digest" in problem for problem in report["problems"]))

    def test_diagnostic_mode_is_metadata_only(self):
        case = self.case(raw={"path_hint": "gone.arw"}, reference={"path": "gone.png"})
        report = self.verify(case, strict=False)
        self.assertTrue(report["accepted"])
        self.assertFalse(report["assets_hashed"])
        self.assertFalse(report["assets_verified"])
        self.assertFalse(report["cases"][0]["assets_verified"])
        self.assertEqual("metadata_only", report["cases"][0]["state"])
        self.assertTrue(any("raw file missing" in warning for warning in report["warnings"]))
        self.assertEqual([], report["problems"])

    def test_data_root_resolves_relative_assets(self):
        root = self.directory / "external"
        root.mkdir()
        shutil.copy(self.raw, root / self.raw.name)
        shutil.copy(self.ref16, root / self.ref16.name)
        manifest_path = self.directory / "manifests" / "manifest.json"
        manifest_path.parent.mkdir()
        report = self.verify(manifest_path=manifest_path, data_root=root)
        self.assertEqual([], report["problems"])
        self.assertTrue(report["accepted"])
        resolved = Path(report["cases"][0]["assets"]["raw"]["resolved"])
        self.assertEqual(root, resolved.parent)


class CompareCliTests(unittest.TestCase):
    """compare must use the same resolution and asset checks as verify."""

    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="fastcull-cmp-"))
        self.addCleanup(shutil.rmtree, self.directory, True)

    def test_compare_uses_data_root_and_rejects_drifted_reference(self):
        data = self.directory / "data"
        data.mkdir()
        reference = data / "ref.png"
        write_png(reference, bit_depth=8)
        candidate_dir = self.directory / "candidates"
        candidate_dir.mkdir()
        write_png(candidate_dir / "case.png", bit_depth=8)
        manifest = {
            "schema": reference_eval.MANIFEST_SCHEMA,
            "cases": [{
                "id": "case", "kind": "lightroom", "format": "arw",
                "raw": {"path_hint": "missing.arw", "sha256": "0" * 64},
                "reference": {
                    "path": "ref.png", "sha256": digest(reference),
                    "output": {"icc": "sRGB", "bit_depth": 8, "width": 60, "height": 40},
                },
                "params": {},
            }],
        }
        manifest_path = self.directory / "manifest.json"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        report_path = self.directory / "report.json"
        result = subprocess.run(
            [sys.executable, "-I", str(SCRIPT), "compare", str(manifest_path),
             "--candidate-dir", str(candidate_dir), "--data-root", str(data),
             "--report", str(report_path)],
            capture_output=True, text=True, check=False)
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)
        report = json.loads(report_path.read_text(encoding="utf-8"))
        self.assertEqual(1, len(report["cases"]))
        self.assertEqual([], report["missing"])
        self.assertIn("rois", report["cases"][0])
        reference.write_bytes(b"changed")
        result = subprocess.run(
            [sys.executable, "-I", str(SCRIPT), "compare", str(manifest_path),
             "--candidate-dir", str(candidate_dir), "--data-root", str(data),
             "--strict-lr"],
            capture_output=True, text=True, check=False)
        self.assertEqual(1, result.returncode, result.stdout + result.stderr)
        self.assertIn("reference hash drifted", result.stdout)

    def test_strict_verify_cli_exit_code(self):
        manifest = {
            "schema": reference_eval.MANIFEST_SCHEMA,
            "cases": [{
                "id": "missing", "kind": "lightroom", "format": "arw",
                "raw": {"path_hint": "gone.arw", "sha256": "0" * 64},
                "reference": {"path": "gone.png", "sha256": "1" * 64,
                              "output": {"icc": "sRGB", "bit_depth": 16,
                                         "width": 10, "height": 10}},
                "params": {"Exposure2012": 1},
                "effective": {},
                "lightroom": {"version": "14", "build": "1", "process_version": "11.0"},
            }],
        }
        manifest_path = self.directory / "manifest.json"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        result = subprocess.run(
            [sys.executable, "-I", str(SCRIPT), "verify", str(manifest_path), "--strict-lr"],
            capture_output=True, text=True, check=False)
        self.assertEqual(1, result.returncode, result.stdout + result.stderr)
        payload = json.loads(result.stdout)
        self.assertFalse(payload["ok"])
        self.assertFalse(payload["verification"]["assets_verified"])
        problems = " ".join(payload["problems"])
        self.assertIn("raw file missing", problems)
        self.assertIn("effective is missing requested key Exposure2012", problems)


if __name__ == "__main__":
    unittest.main()
