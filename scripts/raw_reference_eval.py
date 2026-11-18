#!/usr/bin/env python3
"""Reference/candidate differential evaluation for the RAW color matrix (T01).

Two jobs:

1. ``verify`` checks a reference manifest: every recorded hash, effective
   parameter set and target identity. A changed config/WB/curve or a replaced
   reference file must raise an alarm. Strict mode (``--strict-lr``) additionally
   requires that every asset resolves, exists, is readable, hashes correctly and
   decodes at the declared size, bit depth and ICC space.
2. ``compare`` computes ROI RGB/CIEDE2000/highlight metrics between a manifest's
   references and candidate exports. It is deliberately independent of the
   application: Lightroom/ACR is only an offline reference.

Manifest schema: ``fastcull.raw_reference.manifest.v1``. See
docs/diagnostics/RAW-REFERENCE-PROTOCOL-2026-11-11.md.

Verification states are reported separately so a schema pass, a metadata pass,
an asset-hash pass, a fully verified asset set and a real Lightroom strict
acceptance cannot be confused: ``schema_ok`` -> ``metadata_ok`` ->
``assets_hashed`` -> ``assets_verified`` -> ``accepted``. A diagnostic run that
is allowed to miss files is reported as ``metadata_only``.

Asset paths resolve against, in order: an explicit ``--data-root`` (or the
manifest-level ``data_root``), the manifest's own directory, then the project
root. Project-relative manifests from 2026-11-11 keep working; external test
data roots need not live inside the repository and no author-machine absolute
path is hard-coded.

A 16-bit TIFF/PNG is read through OpenCV when available; otherwise Pillow is
used (8-bit only, recorded as a limitation and rejected in strict mode). A
non-sRGB ICC profile is converted explicitly through Pillow/ImageCms; if the
profile cannot be parsed the case is rejected instead of the samples being
silently treated as sRGB. ImageCms RGB8 conversion is diagnostic-only for
16-bit input: it records the resulting 8-bit precision, which strict mode
rejects. References and candidates share the same color/precision checks.
No per-image fit or tone matching is applied.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import struct
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
MANIFEST_SCHEMA = "fastcull.raw_reference.manifest.v1"
REPORT_SCHEMA = "fastcull.raw_reference.eval.v1"
D50 = np.array([0.96422, 1.0, 0.82521])
SRGB_TO_XYZ_D65 = np.array([
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.0721750],
    [0.0193339, 0.1191920, 0.9503041],
])
BRADFORD_D65_TO_D50 = np.array([
    [1.0478112, 0.0228866, -0.0501270],
    [0.0295424, 0.9904844, -0.0170491],
    [-0.0092345, 0.0150436, 0.7521316],
])
REQUIRED_CASE_FIELDS = ("id", "kind", "format", "raw", "reference", "params")
DIGEST_PATTERN = re.compile(r"^[0-9a-fA-F]{64}$")
FINGERPRINT_FIELDS = ("settings_sha256", "xmp_sha256", "fingerprint", "config_sha256")
STATE_ASSETS_VERIFIED = "assets_verified"
STATE_ASSETS_HASHED = "assets_hashed"
STATE_METADATA_ONLY = "metadata_only"
STATE_INVALID = "invalid"
_ICC_FAMILIES = (
    ("srgb", "sRGB"),
    ("iec61966", "sRGB"),
    ("adobe rgb", "AdobeRGB"),
    ("adobergb", "AdobeRGB"),
    ("prophoto", "ProPhoto"),
    ("romm rgb", "ProPhoto"),
    ("display p3", "DisplayP3"),
    ("aces", "ACES"),
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def valid_digest(value) -> bool:
    return isinstance(value, str) and bool(DIGEST_PATTERN.match(value))


def canonical(value):
    """Normalise a requested/effective value so 0, 0.00 and "+0" compare equal."""
    if isinstance(value, bool):
        return ("bool", value)
    if isinstance(value, (int, float)):
        return ("num", round(float(value), 9))
    if isinstance(value, str):
        text = value.strip()
        try:
            return ("num", round(float(text.lstrip("+")), 9))
        except ValueError:
            pass
        lowered = text.lower()
        if lowered == "true":
            return ("bool", True)
        if lowered == "false":
            return ("bool", False)
        return ("str", " ".join(text.split()))
    if isinstance(value, dict):
        return ("map", {str(key): canonical(item) for key, item in sorted(value.items())})
    if isinstance(value, (list, tuple)):
        return ("list", [canonical(item) for item in value])
    return ("other", repr(value))


def same_value(left, right) -> bool:
    """Compare requested/effective values without formatting noise (0 vs 0.00, +20)."""
    return canonical(left) == canonical(right)


# --------------------------------------------------------------------------
# Asset path resolution

class Resolver:
    """Resolve manifest asset paths against a data root, the manifest and the repo."""

    def __init__(self, *, data_root=None, manifest_path: Path | None = None,
                 project_root: Path = ROOT):
        self.bases: list[Path] = []
        if data_root is not None:
            self.bases.append(Path(data_root))
        if manifest_path is not None:
            self.bases.append(Path(manifest_path).resolve().parent)
        self.bases.append(Path(project_root))
        self.project_root = Path(project_root)

    @classmethod
    def for_manifest(cls, manifest: dict, *, manifest_path: Path | None = None,
                     data_root=None, project_root: Path = ROOT) -> "Resolver":
        declared = manifest.get("data_root")
        root = Path(data_root) if data_root else None
        if root is None and isinstance(declared, str) and declared.strip():
            root = Path(declared)
            if not root.is_absolute() and manifest_path is not None:
                root = Path(manifest_path).resolve().parent / root
        return cls(data_root=root, manifest_path=manifest_path, project_root=project_root)

    def resolve(self, value, *, note: str = "asset") -> tuple[Path | None, str | None]:
        if value is None or (isinstance(value, str) and not value.strip()):
            return None, f"{note} path not given"
        if not isinstance(value, str):
            return None, f"{note} path is not a string: {value!r}"
        raw = Path(value).expanduser()
        candidates = [raw] if raw.is_absolute() else [base / raw for base in self.bases]
        for candidate in candidates:
            if candidate.is_file():
                return candidate, None
        tried = ", ".join(str(candidate) for candidate in candidates)
        return None, f"{note} file missing; tried {tried}"


def asset_paths(entry: dict) -> tuple[object, object]:
    """Return (path_value, sha256_value) for a manifest asset entry."""
    if not isinstance(entry, dict):
        return None, None
    return entry.get("path") or entry.get("path_hint"), entry.get("sha256")


# --------------------------------------------------------------------------
# Image container probing (header level, independent of the pixel loader)

def _tiff_tag_values(path: Path) -> list[dict]:
    """Read the first few TIFF IFDs and return {tag: (type, values, raw_bytes)}."""
    directories = []
    try:
        with path.open("rb") as handle:
            header = handle.read(8)
            if len(header) < 8:
                return []
            if header[:2] == b"II":
                endian = "<"
            elif header[:2] == b"MM":
                endian = ">"
            else:
                return []
            if struct.unpack(endian + "H", header[2:4])[0] not in (42, 43):
                return []
            offset = struct.unpack(endian + "I", header[4:8])[0]
            for _ in range(8):
                if offset <= 0 or offset > 1 << 31:
                    break
                handle.seek(offset)
                raw = handle.read(2)
                if len(raw) < 2:
                    break
                entries = struct.unpack(endian + "H", raw)[0]
                if entries > 4096:
                    break
                directory = handle.read(entries * 12 + 4)
                if len(directory) < entries * 12:
                    break
                tags: dict = {}
                for index in range(entries):
                    at = index * 12
                    tag, kind = struct.unpack_from(endian + "HH", directory, at)
                    if tag not in (256, 257, 258, 259, 277, 34675):
                        continue
                    count = struct.unpack_from(endian + "I", directory, at + 4)[0]
                    size = {1: 1, 2: 1, 3: 2, 4: 4, 7: 1}.get(kind, 0)
                    if not size or count == 0 or count > 4 << 20:
                        continue
                    if count * size <= 4:
                        payload = directory[at + 8:at + 8 + count * size]
                    else:
                        at_payload = struct.unpack_from(endian + "I", directory, at + 8)[0]
                        handle.seek(at_payload)
                        payload = handle.read(count * size)
                    if len(payload) < count * size:
                        continue
                    if kind in (1, 7):
                        values = payload
                    elif kind == 3:
                        values = list(struct.unpack(endian + f"{count}H", payload))
                    else:
                        values = list(struct.unpack(endian + f"{count}I", payload))
                    tags[tag] = values
                directories.append(tags)
                next_offset = struct.unpack_from(endian + "I", directory, entries * 12)[0]
                if next_offset == 0:
                    break
                offset = next_offset
    except (OSError, struct.error):
        return directories
    return directories


def probe_image_header(path: Path) -> dict:
    """Return file-level width/height/bit depth/colour type read from the header."""
    result: dict = {"format": None, "width": None, "height": None,
                    "bit_depth": None, "channels": None}
    suffix = path.suffix.lower()
    try:
        if suffix == ".png":
            with path.open("rb") as handle:
                header = handle.read(33)
            if header[:8] == b"\x89PNG\r\n\x1a\n" and header[12:16] == b"IHDR":
                width, height, depth, colour = struct.unpack(">IIBB", header[16:26])
                channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}.get(colour)
                result.update({"format": "png", "width": width, "height": height,
                               "bit_depth": depth, "channels": channels})
            return result
        if suffix in (".tif", ".tiff"):
            for tags in _tiff_tag_values(path):
                width = tags.get(256)
                height = tags.get(257)
                bits = tags.get(258)
                samples = tags.get(277)
                if not width or not height:
                    continue
                result.update({
                    "format": "tiff",
                    "width": int(width[0]),
                    "height": int(height[0]),
                    "bit_depth": int(max(bits)) if bits else None,
                    "channels": int(samples[0]) if samples else None,
                })
                return result
            return result
        if suffix in (".jpg", ".jpeg"):
            with path.open("rb") as handle:
                data = handle.read(1 << 20)
            at = 2
            while at + 4 < len(data):
                if data[at] != 0xFF:
                    at += 1
                    continue
                marker = data[at + 1]
                if marker in (0xD8, 0xD9) or 0xD0 <= marker <= 0xD7:
                    at += 2
                    continue
                length = struct.unpack_from(">H", data, at + 2)[0]
                if marker in (0xC0, 0xC1, 0xC2, 0xC3):
                    height, width = struct.unpack_from(">HH", data, at + 5)
                    result.update({"format": "jpeg", "width": width, "height": height,
                                   "bit_depth": data[at + 4], "channels": data[at + 9]})
                    return result
                at += 2 + length
            return result
        from PIL import Image  # type: ignore

        with Image.open(path) as image:
            result.update({"format": image.format, "width": image.width,
                           "height": image.height,
                           "bit_depth": 16 if image.mode in ("I;16", "I;16B", "I;16L") else 8})
    except Exception:
        return result
    return result


# --------------------------------------------------------------------------
# ICC profile handling

def icc_family(name: str | None) -> str | None:
    if not name:
        return None
    lowered = name.lower()
    for key, family in _ICC_FAMILIES:
        if key in lowered:
            return family
    return None


def icc_description(profile: bytes) -> str | None:
    """Read the 'desc' (ICC v2) or 'mluc' (ICC v4) tag description."""
    if len(profile) < 132:
        return None
    try:
        count = struct.unpack_from(">I", profile, 128)[0]
    except struct.error:
        return None
    if count > 4096:
        return None
    for index in range(count):
        at = 132 + index * 12
        if at + 12 > len(profile):
            return None
        if profile[at:at + 4] not in (b"desc", b"mluc"):
            continue
        offset, size = struct.unpack_from(">II", profile, at + 4)
        if offset + size > len(profile) or size < 8:
            continue
        tag = profile[offset:offset + size]
        if tag[:4] == b"desc" and len(tag) >= 12:
            length = struct.unpack_from(">I", tag, 8)[0]
            text = tag[12:12 + min(length, size - 12)].split(b"\0", 1)[0]
            return text.decode("ascii", "replace").strip() or None
        if tag[:4] == b"mluc" and len(tag) >= 16:
            records = struct.unpack_from(">II", tag, 8)
            for record in range(min(records[0], 16)):
                at_record = 16 + record * records[1]
                if at_record + 12 > len(tag):
                    break
                length, text_offset = struct.unpack_from(">II", tag, at_record + 4)
                if text_offset + length > len(tag):
                    continue
                text = tag[text_offset:text_offset + length].decode("utf-16-be", "replace")
                if text.strip():
                    return text.strip()
    return None


def embedded_icc(path: Path) -> dict:
    """Return {'present', 'bytes', 'family', 'description'} for an image file."""
    result = {"present": False, "bytes": None, "family": None, "description": None}
    profile = None
    try:
        from PIL import Image  # type: ignore

        with Image.open(path) as image:
            profile = image.info.get("icc_profile")
    except Exception:
        profile = None
    if not profile and path.suffix.lower() in (".tif", ".tiff"):
        for tags in _tiff_tag_values(path):
            candidate = tags.get(34675)
            if candidate:
                profile = candidate
                break
    if not profile:
        return result
    result["present"] = True
    result["bytes"] = profile
    description = icc_description(profile)
    result["description"] = description
    result["family"] = icc_family(description)
    return result


def declared_icc_family(value) -> tuple[str | None, str]:
    """Map a manifest ICC declaration to a family, keeping the raw text."""
    if isinstance(value, dict):
        text = str(value.get("name") or value.get("family") or value.get("path") or "")
    elif value is None:
        text = ""
    else:
        text = str(value)
    text = text.strip()
    family = icc_family(text)
    if family:
        return family, text
    if text and Path(text).suffix.lower() in (".icc", ".icm"):
        profile_path = Path(text)
        if profile_path.is_file():
            profile = embedded_icc(profile_path)
            if profile["family"]:
                return profile["family"], text
    return (None, text) if text else (None, "")


def srgb_transform(profile_bytes: bytes):
    """Validate the ICC payload, including profiles whose description says sRGB."""
    from PIL import ImageCms  # type: ignore

    source = ImageCms.ImageCmsProfile(io.BytesIO(profile_bytes))
    target = ImageCms.createProfile("sRGB")
    return ImageCms.buildTransform(source, target, "RGB", "RGB",
                                   renderingIntent=ImageCms.Intent.RELATIVE_COLORIMETRIC)


def convert_to_srgb(rgb: np.ndarray, profile_bytes: bytes) -> tuple[np.ndarray | None, str]:
    """Convert 0..255 RGB samples from an embedded profile to sRGB via ImageCms."""
    try:
        from PIL import Image, ImageCms  # type: ignore

        transform = srgb_transform(profile_bytes)
        image = Image.fromarray(np.clip(rgb, 0, 255).astype(np.uint8), mode="RGB")
        converted = ImageCms.applyTransform(image, transform)
        return np.asarray(converted, dtype=np.float32), "Pillow ImageCms RGB8"
    except Exception as error:  # pragma: no cover - depends on the installed lcms
        return None, f"{type(error).__name__}: {error}"


# --------------------------------------------------------------------------
# Pixel loading

def load_rgb(path: Path, *, icc: bool = False) -> tuple[np.ndarray, dict]:
    """Return HxWx3 float32 code values in 0..255 plus loader metadata.

    With ``icc=True`` a non-sRGB embedded profile is converted explicitly to
    sRGB and the conversion is recorded; values are never silently assumed to
    be sRGB when the file declares another space.
    """
    meta: dict = {"path": str(path)}
    profile = embedded_icc(path) if icc else None
    try:
        import cv2  # type: ignore
    except ImportError:  # pragma: no cover - fallback on machines without OpenCV
        cv2 = None
    if cv2 is not None:
        data = cv2.imread(str(path), cv2.IMREAD_UNCHANGED)
        if data is None:
            raise ValueError(f"cannot read image: {path}")
        if data.dtype not in (np.uint8, np.uint16):
            raise ValueError(f"unsupported image dtype {data.dtype} for {path}")
        bit_depth = 16 if data.dtype == np.uint16 else 8
        scale = 255.0 / 65535.0 if bit_depth == 16 else 1.0
        if data.ndim == 2:
            rgb = np.repeat((data.astype(np.float32) * scale)[:, :, None], 3, axis=2)
        elif data.ndim == 3 and data.shape[2] in (3, 4):
            # OpenCV returns BGR/BGRA. Discard alpha before reordering; reversing
            # all four channels would incorrectly turn BGRA into ABG.
            rgb = data[:, :, :3][:, :, ::-1].astype(np.float32) * scale
        else:
            raise ValueError(f"unsupported image shape {data.shape} for {path}")
        if rgb.shape[2] < 3:
            raise ValueError(f"unsupported image shape {data.shape} for {path}")
        rgb = rgb[:, :, :3]
        meta.update({"loader": "opencv", "bit_depth": bit_depth,
                     "precision_preserved": True, "shape": list(data.shape)})
    else:
        from PIL import Image  # type: ignore

        with Image.open(path) as handle:
            mode = handle.mode
            if mode in ("I;16", "I;16B", "I;16L", "I;16N"):
                array = np.asarray(handle, dtype=np.uint16).astype(np.float32)
                rgb = np.repeat((array * (255.0 / 65535.0))[:, :, None], 3, axis=2)
                bit_depth = 16
                preserved = True
            elif mode in ("L", "P", "RGB", "RGBA"):
                array = np.asarray(handle.convert("RGB"), dtype=np.float32)
                rgb = array
                bit_depth = 8
                preserved = True
            else:
                array = np.asarray(handle.convert("RGB"), dtype=np.float32)
                rgb = array
                bit_depth = 16
                preserved = False
        meta.update({"loader": "pillow", "bit_depth": bit_depth,
                     "precision_preserved": preserved})
        if not preserved:
            meta["limitation"] = f"Pillow reduced mode {mode} to 8-bit RGB"
    if profile is not None:
        meta["icc"] = {"present": bool(profile["present"]), "family": profile["family"],
                       "description": profile["description"]}
        if profile["present"] and profile["family"] == "sRGB":
            try:
                srgb_transform(profile["bytes"])
            except Exception as error:
                meta["icc_conversion_error"] = f"invalid sRGB ICC: {error}"
        if profile["present"] and profile["family"] not in (None, "sRGB"):
            source_depth = meta["bit_depth"]
            converted, how = convert_to_srgb(rgb, profile["bytes"])
            if converted is None:
                meta["icc_conversion_error"] = how
            else:
                rgb = converted
                meta["source_bit_depth"] = source_depth
                meta["bit_depth"] = 8
                meta["precision_preserved"] = bool(
                    meta["precision_preserved"] and source_depth <= 8)
                meta["icc_conversion"] = {
                    "from": profile["family"] or profile["description"],
                    "how": how,
                    "precision": "lcms input is 8-bit; 16-bit samples are scaled first",
                }
    return rgb, meta


def color_problems(meta: dict, *, strict_precision: bool = False,
                   minimum_bit_depth: int = 0) -> list[str]:
    """The same color/precision contract for references and candidates."""
    problems = []
    profile = meta.get("icc") or {}
    if meta.get("icc_conversion_error"):
        problems.append(f"ICC conversion failed: {meta['icc_conversion_error']}")
    elif profile.get("present"):
        family = profile.get("family")
        if family is None:
            problems.append(f"ICC profile is unrecognised: {profile.get('description')!r}")
        elif family != "sRGB" and not meta.get("icc_conversion"):
            problems.append(f"ICC {family} has not been converted to sRGB")
    if strict_precision and (not meta.get("precision_preserved")
                             or meta.get("bit_depth", 0) < minimum_bit_depth):
        problems.append("pixel precision was reduced; strict evaluation requires "
                        f"at least {minimum_bit_depth}-bit preserved samples")
    return problems


def resize_to(rgb: np.ndarray, width: int, height: int) -> np.ndarray:
    if rgb.shape[1] == width and rgb.shape[0] == height:
        return rgb
    try:
        import cv2  # type: ignore

        return cv2.resize(rgb, (width, height), interpolation=cv2.INTER_AREA)
    except ImportError:  # pragma: no cover
        from PIL import Image  # type: ignore

        image = Image.fromarray(np.clip(rgb, 0, 255).astype(np.uint8))
        return np.asarray(image.resize((width, height), Image.Resampling.LANCZOS),
                          dtype=np.float32)


# --------------------------------------------------------------------------
# Metrics

def srgb_to_lab(rgb: np.ndarray) -> np.ndarray:
    shape = rgb.shape
    c = np.clip(rgb.reshape(-1, 3), 0.0, 255.0) / 255.0
    linear = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    xyz65 = linear @ SRGB_TO_XYZ_D65.T
    xyz50 = xyz65 @ BRADFORD_D65_TO_D50.T
    xyz = xyz50 / D50
    epsilon = (6.0 / 29.0) ** 3
    threshold = 6.0 / 29.0
    f = np.where(xyz > epsilon, np.cbrt(xyz), xyz / (3 * threshold**2) + 4.0 / 29.0)
    lab = np.stack([
        116.0 * f[:, 1] - 16.0,
        500.0 * (f[:, 0] - f[:, 1]),
        200.0 * (f[:, 1] - f[:, 2]),
    ], axis=1)
    return lab.reshape(shape)


def delta_e00(lab1: np.ndarray, lab2: np.ndarray) -> np.ndarray:
    """CIEDE2000 on Nx3 Lab arrays; returns N."""
    l1, a1, b1 = lab1[:, 0], lab1[:, 1], lab1[:, 2]
    l2, a2, b2 = lab2[:, 0], lab2[:, 1], lab2[:, 2]
    c1 = np.hypot(a1, b1)
    c2 = np.hypot(a2, b2)
    c_bar = (c1 + c2) / 2.0
    g = 0.5 * (1.0 - np.sqrt(c_bar**7 / (c_bar**7 + 25.0**7)))
    a1p = (1.0 + g) * a1
    a2p = (1.0 + g) * a2
    c1p = np.hypot(a1p, b1)
    c2p = np.hypot(a2p, b2)
    h1p = np.degrees(np.arctan2(b1, a1p)) % 360.0
    h2p = np.degrees(np.arctan2(b2, a2p)) % 360.0
    dlp = l2 - l1
    dcp = c2p - c1p
    dh = h2p - h1p
    dh = np.where(dh > 180.0, dh - 360.0, np.where(dh < -180.0, dh + 360.0, dh))
    dh = np.where((c1p * c2p) == 0.0, 0.0, dh)
    dhp = 2.0 * np.sqrt(c1p * c2p) * np.sin(np.radians(dh) / 2.0)
    l_bar = (l1 + l2) / 2.0
    c_bar_p = (c1p + c2p) / 2.0
    h_sum = h1p + h2p
    h_diff = np.abs(h1p - h2p)
    h_bar = np.where(
        (c1p * c2p) == 0.0, h_sum,
        np.where(h_diff <= 180.0, h_sum / 2.0,
                 np.where(h_sum < 360.0, (h_sum + 360.0) / 2.0,
                          (h_sum - 360.0) / 2.0)),
    )
    t = (1.0
         - 0.17 * np.cos(np.radians(h_bar - 30.0))
         + 0.24 * np.cos(np.radians(2.0 * h_bar))
         + 0.32 * np.cos(np.radians(3.0 * h_bar + 6.0))
         - 0.20 * np.cos(np.radians(4.0 * h_bar - 63.0)))
    sl = 1.0 + (0.015 * (l_bar - 50.0) ** 2) / np.sqrt(20.0 + (l_bar - 50.0) ** 2)
    sc = 1.0 + 0.045 * c_bar_p
    sh = 1.0 + 0.015 * c_bar_p * t
    rt = -2.0 * np.sqrt(c_bar_p**7 / (c_bar_p**7 + 25.0**7)) * np.sin(
        np.radians(60.0 * np.exp(-(((h_bar - 275.0) / 25.0) ** 2))))
    return np.sqrt((dlp / sl) ** 2 + (dcp / sc) ** 2 + (dhp / sh) ** 2
                   + rt * (dcp / sc) * (dhp / sh))


def roi_slice(roi: dict, width: int, height: int) -> tuple[slice, slice]:
    if all(key in roi for key in ("x", "y", "w", "h")) and roi.get("unit", "fraction") == "pixel":
        x, y, w, h = (int(roi[key]) for key in ("x", "y", "w", "h"))
    else:
        x = int(round(float(roi.get("x", 0.0)) * width))
        y = int(round(float(roi.get("y", 0.0)) * height))
        w = int(round(float(roi.get("w", 1.0)) * width))
        h = int(round(float(roi.get("h", 1.0)) * height))
    x = max(0, min(width - 1, x))
    y = max(0, min(height - 1, y))
    w = max(1, min(width - x, w))
    h = max(1, min(height - y, h))
    return slice(y, y + h), slice(x, x + w)


def compare_images(reference: np.ndarray, candidate: np.ndarray, rois: list[dict]) -> dict:
    lab_ref = srgb_to_lab(reference)
    lab_cand = srgb_to_lab(candidate)
    result = {"size": [reference.shape[1], reference.shape[0]], "rois": []}
    for roi in rois:
        ys, xs = roi_slice(roi, reference.shape[1], reference.shape[0])
        a = reference[ys, xs].reshape(-1, 3)
        b = candidate[ys, xs].reshape(-1, 3)
        de = delta_e00(lab_ref[ys, xs].reshape(-1, 3), lab_cand[ys, xs].reshape(-1, 3))
        white_candidate = np.all(b >= 254.0, axis=1)
        white_reference = np.min(a, axis=1) >= 250.0
        result["rois"].append({
            "name": roi.get("name", "all"),
            "role": roi.get("role", "general"),
            "pixels": int(a.shape[0]),
            "delta_e00_mean": float(np.mean(de)),
            "delta_e00_p95": float(np.percentile(de, 95)),
            "delta_e00_max": float(np.max(de)),
            "rgb_mae": float(np.mean(np.abs(a - b))),
            "white_candidate_percent": float(np.mean(white_candidate) * 100.0),
            "white_reference_percent": float(np.mean(white_reference) * 100.0),
            "excess_white_percent": float(np.mean(white_candidate & ~white_reference) * 100.0),
            "neutral_chroma_candidate": float(np.mean(np.hypot(
                lab_cand[ys, xs, 1], lab_cand[ys, xs, 2]))),
            "neutral_chroma_reference": float(np.mean(np.hypot(
                lab_ref[ys, xs, 1], lab_ref[ys, xs, 2]))),
            "median_lstar_candidate": float(np.median(lab_cand[ys, xs, 0])),
            "median_lstar_reference": float(np.median(lab_ref[ys, xs, 0])),
        })
    return result


def default_rois() -> list[dict]:
    return [
        {"name": "all", "role": "general", "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0},
        {"name": "center", "role": "general", "x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5},
    ]


# --------------------------------------------------------------------------
# Verification

def _verify_asset(ident: str, entry: dict, label: str, resolver: Resolver,
                  *, strict: bool) -> tuple[dict, list[str], list[str]]:
    """Verify one raw/xmp asset: digest validity, resolution, readability, hash."""
    problems: list[str] = []
    warnings: list[str] = []
    record: dict = {"label": label, "resolved": None, "hashed": False, "verified": False}
    entry = entry if isinstance(entry, dict) else {}
    declared_hash = entry.get("sha256")
    if declared_hash is None:
        problems.append(f"{ident}: {label} hash missing")
    elif not valid_digest(declared_hash):
        problems.append(f"{ident}: {label} hash is not a 64-hex digest: {declared_hash!r}")
    path, reason = resolver.resolve(asset_paths(entry)[0], note=label)
    if path is None:
        (problems if strict else warnings).append(f"{ident}: {reason}")
        record["reason"] = reason
        return record, problems, warnings
    record["resolved"] = str(path)
    try:
        with path.open("rb") as handle:
            handle.read(1)
    except OSError as error:
        message = f"{ident}: {label} unreadable: {error}"
        (problems if strict else warnings).append(message)
        record["reason"] = message
        return record, problems, warnings
    if valid_digest(declared_hash):
        actual = sha256(path)
        record["sha256_actual"] = actual
        if actual != declared_hash.lower():
            problems.append(f"{ident}: {label} hash drifted ({actual})")
            return record, problems, warnings
    record["hashed"] = True
    record["verified"] = True
    return record, problems, warnings


def _verify_reference(ident: str, case: dict, resolver: Resolver,
                      *, strict: bool) -> tuple[dict, list[str], list[str]]:
    reference = case.get("reference") or {}
    record, problems, warnings = _verify_asset(ident, reference, "reference", resolver,
                                               strict=strict)

    def flag(message: str) -> None:
        (problems if strict else warnings).append(message)

    output = reference.get("output") or {}
    declared: dict = {}
    for field in ("width", "height", "bit_depth", "icc"):
        value = output.get(field)
        if value in (None, "", 0):
            flag(f"{ident}: reference.output.{field} missing")
        else:
            declared[field] = value
    record["declared_output"] = declared
    if not record["hashed"]:
        return record, problems, warnings

    path = Path(record["resolved"])
    header = probe_image_header(path)
    record["header"] = header
    try:
        rgb, meta = load_rgb(path, icc=True)
    except Exception as error:
        problems.append(f"{ident}: reference undecodable: {type(error).__name__}: {error}")
        record["verified"] = False
        return record, problems, warnings
    profile = meta.get("icc") or {}
    problems.extend(f"{ident}: reference {problem}" for problem in color_problems(
        meta, strict_precision=strict, minimum_bit_depth=header.get("bit_depth") or 0))
    actual = {
        "width": int(rgb.shape[1]),
        "height": int(rgb.shape[0]),
        "bit_depth": int(meta.get("bit_depth", 8)),
        "loader": meta.get("loader"),
        "precision_preserved": bool(meta.get("precision_preserved", False)),
        "icc_present": bool(profile.get("present")),
        "icc_family": profile.get("family"),
        "icc_description": profile.get("description"),
    }
    if meta.get("icc_conversion"):
        actual["icc_conversion"] = meta["icc_conversion"]
        warnings.append(
            f"{ident}: reference ICC converted to sRGB ({meta['icc_conversion']['how']})")
    record["actual"] = actual

    file_depth = header.get("bit_depth")
    if declared.get("width") is not None and int(declared["width"]) != actual["width"]:
        flag(f"{ident}: reference width {actual['width']} != declared {declared['width']}")
    if declared.get("height") is not None and int(declared["height"]) != actual["height"]:
        flag(f"{ident}: reference height {actual['height']} != declared {declared['height']}")
    if file_depth and actual["width"] and declared.get("width") is not None:
        header_width, header_height = header.get("width"), header.get("height")
        if (header_width, header_height) != (actual["width"], actual["height"]):
            flag(f"{ident}: header size {header_width}x{header_height} differs from decoded"
                 f" {actual['width']}x{actual['height']}")
    if declared.get("bit_depth") is not None:
        try:
            bit_depth = int(declared["bit_depth"])
        except (TypeError, ValueError):
            flag(f"{ident}: reference.output.bit_depth is not a number:"
                 f" {declared['bit_depth']!r}")
            bit_depth = 0
        if bit_depth and bit_depth not in (8, 16):
            flag(f"{ident}: reference.output.bit_depth must be 8 or 16, got {bit_depth}")
        elif bit_depth:
            if file_depth and file_depth < bit_depth:
                flag(f"{ident}: reference file is {file_depth}-bit but {bit_depth}-bit was"
                     " declared")
            elif actual["bit_depth"] < bit_depth or not actual["precision_preserved"]:
                flag(f"{ident}: reference declared {bit_depth}-bit but loaded as"
                     f" {actual['bit_depth']}-bit ({actual['loader']})")
    if declared.get("icc") is not None:
        family, text = declared_icc_family(declared["icc"])
        if family is None:
            flag(f"{ident}: cannot resolve declared ICC {text!r} (use a recognised family"
                 " name or a profile path)")
        elif profile.get("present") and profile.get("family") != family:
            flag(f"{ident}: embedded ICC is {profile.get('family')}"
                 f" ({profile.get('description')!r}) but {family} was declared")
        elif meta.get("icc_conversion_error"):
            problems.append(f"{ident}: ICC conversion failed: {meta['icc_conversion_error']}")
        elif not profile.get("present"):
            if actual["icc_present"]:
                flag(f"{ident}: embedded ICC profile could not be classified;"
                     f" {family} was declared")
            elif family != "sRGB":
                flag(f"{ident}: declared ICC {family} but no embedded profile was found;"
                     " non-sRGB samples cannot be verified")
    record["verified"] = not any(problem.startswith(f"{ident}:") for problem in problems)
    return record, problems, warnings


def _verify_case(case: dict, resolver: Resolver, *, strict: bool) -> dict:
    ident = case.get("id", "<missing>")
    schema_problems: list[str] = []
    metadata_problems: list[str] = []
    asset_problems: list[str] = []
    warnings: list[str] = []
    for field in REQUIRED_CASE_FIELDS:
        if field not in case:
            schema_problems.append(f"{ident}: missing {field}")
    if strict and case.get("kind") != "lightroom":
        schema_problems.append(f"{ident}: kind is {case.get('kind')!r}, not 'lightroom'")
    raw = case.get("raw") if isinstance(case.get("raw"), dict) else {}
    reference = case.get("reference") if isinstance(case.get("reference"), dict) else {}
    xmp = case.get("xmp") if isinstance(case.get("xmp"), dict) else {}
    params = case.get("params")
    if not isinstance(params, dict):
        metadata_problems.append(f"{ident}: params must be an object")
        params = {}
    effective = case.get("effective")
    if not isinstance(effective, dict):
        if strict:
            metadata_problems.append(f"{ident}: effective object missing or not an object")
        effective = {}
    for key, value in params.items():
        if key not in effective:
            metadata_problems.append(f"{ident}: effective is missing requested key {key}")
        elif not same_value(value, effective[key]):
            metadata_problems.append(
                f"{ident}: requested {key}={value!r} but effective {effective[key]!r}")
    if case.get("kind") == "lightroom":
        lightroom = case.get("lightroom") if isinstance(case.get("lightroom"), dict) else {}
        for field in ("version", "build", "process_version"):
            if not lightroom.get(field):
                metadata_problems.append(f"{ident}: lightroom.{field} missing")
    if strict:
        fingerprint = next((case[field] for field in FINGERPRINT_FIELDS if case.get(field)), None)
        if fingerprint is None and isinstance(case.get("config"), dict):
            fingerprint = case["config"].get("sha256") or case["config"].get("fingerprint")
        if fingerprint is None:
            fingerprint = xmp.get("sha256")
        if fingerprint is None:
            metadata_problems.append(
                f"{ident}: no config fingerprint (settings_sha256/xmp_sha256/xmp.sha256)")
        elif not valid_digest(fingerprint):
            metadata_problems.append(f"{ident}: config fingerprint is not a 64-hex digest")
        declared_format = str(case.get("format") or "").lower().lstrip(".")
        raw_path = asset_paths(raw)[0]
        suffix = Path(str(raw_path)).suffix.lower().lstrip(".") if raw_path else ""
        override = str(raw.get("format") or "").lower()
        if declared_format and suffix and suffix != declared_format and override != declared_format:
            metadata_problems.append(
                f"{ident}: raw suffix .{suffix} does not match declared format"
                f" {declared_format}")

    raw_record, raw_problems, raw_warnings = _verify_asset(ident, raw, "raw", resolver,
                                                           strict=strict)
    reference_record, reference_problems, reference_warnings = _verify_reference(
        ident, case, resolver, strict=strict)
    xmp_record = None
    if xmp.get("path") or xmp.get("path_hint") or xmp.get("sha256"):
        xmp_record, xmp_problems, xmp_warnings = _verify_asset(ident, xmp, "xmp", resolver,
                                                               strict=strict)
    else:
        xmp_problems, xmp_warnings = [], []
    problems = (schema_problems + metadata_problems + raw_problems + reference_problems
                + xmp_problems)
    warnings.extend(raw_warnings + reference_warnings + xmp_warnings)
    assets = {"raw": raw_record, "reference": reference_record, "xmp": xmp_record,
              "config_fingerprint": next(
                  (case[field] for field in FINGERPRINT_FIELDS if case.get(field)), None)}
    hashed = bool(raw_record["hashed"] and reference_record["hashed"]
                  and (xmp_record is None or xmp_record["hashed"]))
    verified = bool(hashed and reference_record["verified"] and not problems)
    metadata_ok = not schema_problems and not metadata_problems
    if problems:
        state = STATE_INVALID
    elif strict and metadata_ok and verified:
        state = STATE_ASSETS_VERIFIED
    elif hashed:
        state = STATE_ASSETS_HASHED
    else:
        state = STATE_METADATA_ONLY
    return {
        "id": ident,
        "state": state,
        "schema_ok": not schema_problems,
        "metadata_ok": metadata_ok,
        "assets_hashed": hashed,
        "assets_verified": bool(strict and metadata_ok and verified),
        "problems": problems,
        "warnings": warnings,
        "assets": assets,
    }


def verify_manifest_report(manifest: dict, *, strict_lr: bool = False,
                           manifest_path: Path | None = None, data_root=None,
                           project_root: Path = ROOT) -> dict:
    schema_ok = manifest.get("schema") == MANIFEST_SCHEMA
    structure_problems: list[str] = []
    if not schema_ok:
        structure_problems.append(f"schema must be {MANIFEST_SCHEMA}")
    cases = manifest.get("cases")
    if not isinstance(cases, list) or not cases:
        return {
            "schema_ok": False, "metadata_ok": False, "assets_hashed": False,
            "assets_verified": False, "strict_lr": bool(strict_lr), "accepted": False,
            "problems": structure_problems + ["manifest has no cases"], "warnings": [],
            "cases": [], "resolution": {},
        }
    resolver = Resolver.for_manifest(manifest, manifest_path=manifest_path,
                                     data_root=data_root, project_root=project_root)
    case_reports = []
    seen: set[str] = set()
    for case in cases:
        ident = case.get("id", "<missing>")
        if ident in seen:
            structure_problems.append(f"{ident}: duplicate id")
        seen.add(ident)
        case_reports.append(_verify_case(case, resolver, strict=strict_lr))
    problems = list(structure_problems)
    warnings: list[str] = []
    for case_report in case_reports:
        problems.extend(case_report["problems"])
        warnings.extend(case_report["warnings"])
    if not strict_lr and any(case.get("kind") != "lightroom" for case in cases):
        warnings.append(
            "manifest contains non-Lightroom diagnostic cases; not a target-LR acceptance set")
    metadata_ok = schema_ok and all(case_report["metadata_ok"] for case_report in case_reports)
    assets_hashed = metadata_ok and all(case_report["assets_hashed"] for case_report in case_reports)
    assets_verified = metadata_ok and all(
        case_report["assets_verified"] for case_report in case_reports)
    accepted = not problems and metadata_ok and (assets_verified if strict_lr else True)
    if strict_lr and assets_verified and not problems:
        accepted = True
    return {
        "schema_ok": schema_ok,
        "metadata_ok": metadata_ok,
        "assets_hashed": bool(assets_hashed),
        "assets_verified": bool(assets_verified),
        "strict_lr": bool(strict_lr),
        "accepted": bool(accepted),
        "problems": problems,
        "warnings": warnings,
        "cases": case_reports,
        "resolution": {"bases": [str(base) for base in resolver.bases],
                       "data_root": str(resolver.bases[0]) if resolver.bases else None},
    }


def verify_manifest(manifest: dict, *, strict_lr: bool = False,
                    manifest_path: Path | None = None, data_root=None,
                    project_root: Path = ROOT) -> tuple[bool, list[str], list[str]]:
    report = verify_manifest_report(manifest, strict_lr=strict_lr,
                                    manifest_path=manifest_path, data_root=data_root,
                                    project_root=project_root)
    return report["accepted"], report["problems"], report["warnings"]


def self_test() -> int:
    failures: list[str] = []
    white = np.full((8, 8, 3), 255.0, np.float32)
    black = np.zeros((8, 8, 3), np.float32)
    de = delta_e00(srgb_to_lab(white.reshape(-1, 3)), srgb_to_lab(black.reshape(-1, 3)))
    if not (95.0 < float(de[0]) < 105.0):
        failures.append(f"white/black deltaE00 out of range: {float(de[0])}")
    gray = np.full((4, 4, 3), 128.0, np.float32)
    de = delta_e00(srgb_to_lab(gray.reshape(-1, 3)), srgb_to_lab(gray.reshape(-1, 3)))
    if float(np.max(de)) > 1e-6:
        failures.append("identical gray deltaE00 is not zero")
    candidate = gray.copy()
    candidate[0, 0] = 255.0
    reference = gray.copy()
    reference[0, 0] = 200.0
    metrics = compare_images(reference, candidate, default_rois())
    all_roi = next(roi for roi in metrics["rois"] if roi["name"] == "all")
    if not (0.0 < all_roi["excess_white_percent"] < 10.0):
        failures.append(f"excess white not detected: {all_roi['excess_white_percent']}")
    identical = compare_images(reference, reference, default_rois())
    for roi in identical["rois"]:
        if roi["delta_e00_max"] > 1e-6 or roi["rgb_mae"] > 1e-6:
            failures.append(f"identical images differ in ROI {roi['name']}")
    if not same_value(0, "0.00") or not same_value("+0", 0.0) or same_value(1, "1.5"):
        failures.append("numeric equivalence comparison is wrong")
    manifest = {
        "schema": MANIFEST_SCHEMA,
        "cases": [{
            "id": "x", "kind": "lightroom", "format": "arw",
            "raw": {"path_hint": "missing.raw", "sha256": "0" * 64},
            "reference": {"path": "missing.png", "sha256": "0" * 64,
                          "output": {"icc": "sRGB", "bit_depth": 16,
                                     "width": 10, "height": 10}},
            "params": {"Exposure2012": "1"},
            "effective": {"Exposure2012": "0.5"},
            "lightroom": {"version": "14", "build": "1", "process_version": "11.0"},
        }],
    }
    ok, problems, _ = verify_manifest(manifest, strict_lr=True)
    if ok or not any("requested Exposure2012" in problem for problem in problems):
        failures.append("manifest verifier did not alarm on changed effective parameter")
    manifest["cases"][0]["effective"] = {"Exposure2012": "1"}
    ok, problems, _ = verify_manifest(manifest, strict_lr=True)
    if ok:
        failures.append("strict mode accepted a manifest with missing assets")
    for needle in ("raw file missing", "reference file missing", "no config fingerprint"):
        if not any(needle in problem for problem in problems):
            failures.append(f"strict mode did not report {needle!r}")
    without_effective = {
        "schema": MANIFEST_SCHEMA,
        "cases": [{key: value for key, value in manifest["cases"][0].items()
                   if key != "effective"}],
    }
    ok, problems, _ = verify_manifest(without_effective, strict_lr=True)
    if ok or not any("effective object missing" in problem for problem in problems):
        failures.append("manifest verifier did not alarm on the missing effective entry")
    diagnostic_ok, diagnostic_problems, diagnostic_warnings = verify_manifest(
        manifest, strict_lr=False)
    if not diagnostic_ok:
        failures.append(f"diagnostic mode must allow missing files: {diagnostic_problems}")
    if not any("raw file missing" in warning for warning in diagnostic_warnings):
        failures.append("diagnostic mode did not warn about the missing raw asset")
    if failures:
        print("self-test FAILED:")
        for failure in failures:
            print(" -", failure)
        return 1
    print("self-test passed: deltaE00, white-excess, ROI identity, numeric equivalence"
          " and strict manifest alarms")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    verify = sub.add_parser("verify")
    verify.add_argument("manifest", type=Path)
    verify.add_argument("--strict-lr", action="store_true")
    verify.add_argument("--data-root", type=Path, default=None,
                        help="directory that manifest-relative asset paths resolve against")
    verify.add_argument("--report", type=Path, default=None)
    compare = sub.add_parser("compare")
    compare.add_argument("manifest", type=Path)
    compare.add_argument("--candidate-dir", type=Path)
    compare.add_argument("--data-root", type=Path, default=None)
    compare.add_argument("--report", type=Path)
    compare.add_argument("--strict-lr", action="store_true")
    sub.add_parser("self-test")
    args = parser.parse_args()
    if args.command == "self-test":
        return self_test()
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    report = verify_manifest_report(manifest, strict_lr=args.strict_lr,
                                    manifest_path=args.manifest, data_root=args.data_root)
    summary = {
        "manifest": str(args.manifest),
        "target": manifest.get("target", {}),
        "cases": len(manifest.get("cases", [])),
        "ok": report["accepted"],
        "verification": {
            "schema_ok": report["schema_ok"],
            "metadata_ok": report["metadata_ok"],
            "assets_hashed": report["assets_hashed"],
            "assets_verified": report["assets_verified"],
            "strict_lr": report["strict_lr"],
            "accepted": report["accepted"],
        },
        "resolution": report["resolution"],
        "states": {case["id"]: case["state"] for case in report["cases"]},
        "problems": report["problems"],
        "warnings": report["warnings"],
    }
    if args.report:
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n",
                               encoding="utf-8")
    if args.command == "verify":
        if report["problems"] or not report["accepted"]:
            summary["cases_detail"] = report["cases"]
        print(json.dumps(summary, ensure_ascii=False, indent=2))
        return 0 if report["accepted"] else 1
    if args.strict_lr and not report["accepted"]:
        summary["cases_detail"] = report["cases"]
        summary["error"] = "strict verification failed; comparison skipped"
        print(json.dumps(summary, ensure_ascii=False, indent=2))
        return 1
    if report["problems"]:
        summary["cases_detail"] = report["cases"]
    resolver = Resolver.for_manifest(manifest, manifest_path=args.manifest,
                                     data_root=args.data_root)
    evaluation = {
        "schema": REPORT_SCHEMA,
        "manifest": str(args.manifest),
        "target": manifest.get("target", {}),
        "strict_lr": bool(args.strict_lr),
        "verification": summary["verification"],
        "cases": [],
    }
    missing_candidates = []
    for case in manifest["cases"]:
        reference_meta = case.get("reference") or {}
        reference_path, reason = resolver.resolve(asset_paths(reference_meta)[0],
                                                  note="reference")
        if reference_path is None:
            missing_candidates.append({"id": case["id"], "reason": reason})
            continue
        declared_digest = reference_meta.get("sha256")
        if not valid_digest(declared_digest):
            missing_candidates.append(
                {"id": case["id"], "reason": "reference hash missing or invalid"})
            continue
        if sha256(reference_path) != declared_digest.lower():
            missing_candidates.append({"id": case["id"], "reason": "reference hash drifted"})
            continue
        candidate_path = None
        if args.candidate_dir:
            for suffix in (".png", ".tif", ".tiff", ".jpg", ".jpeg"):
                path = args.candidate_dir / f"{case['id']}{suffix}"
                if path.is_file():
                    candidate_path = path
                    break
        elif (case.get("candidate") or {}).get("path"):
            path, _ = resolver.resolve(case["candidate"]["path"], note="candidate")
            if path is not None and path.is_file():
                candidate_path = path
        if candidate_path is None:
            missing_candidates.append({"id": case["id"], "reason": "candidate missing"})
            continue
        try:
            reference, reference_loader = load_rgb(reference_path, icc=True)
            candidate, candidate_loader = load_rgb(candidate_path, icc=True)
        except Exception as error:
            missing_candidates.append(
                {"id": case["id"], "reason": f"decode failed: {type(error).__name__}: {error}"})
            continue
        candidate_header = probe_image_header(candidate_path)
        color_errors = [f"{label}: {problem}"
                        for label, loader in (("reference", reference_loader),
                                              ("candidate", candidate_loader))
                        for problem in color_problems(
                            loader, strict_precision=args.strict_lr,
                            minimum_bit_depth=max(
                                reference_loader.get("bit_depth", 8),
                                (candidate_header.get("bit_depth") or 0)
                                if label == "candidate" else 0))]
        if color_errors:
            missing_candidates.append({"id": case["id"], "reason": "; ".join(color_errors)})
            continue
        if args.strict_lr and reference_loader.get("bit_depth", 8) > candidate_loader.get("bit_depth", 8):
            missing_candidates.append({
                "id": case["id"],
                "reason": f"candidate loaded at {candidate_loader.get('bit_depth')}-bit but"
                          f" reference is {reference_loader.get('bit_depth')}-bit",
            })
            continue
        height, width = reference.shape[:2]
        resized = candidate.shape[:2] != (height, width)
        candidate = resize_to(candidate, width, height)
        metrics = compare_images(reference, candidate, case.get("rois") or default_rois())
        metrics.update({
            "id": case["id"],
            "kind": case.get("kind"),
            "params": case.get("params"),
            "candidate": str(candidate_path),
            "candidate_resized": resized,
            "reference_loader": reference_loader,
            "candidate_loader": candidate_loader,
        })
        evaluation["cases"].append(metrics)
    evaluation["missing"] = missing_candidates
    text = json.dumps(evaluation, ensure_ascii=False, indent=2)
    if args.report:
        args.report.write_text(text + "\n", encoding="utf-8")
    print(text)
    return 1 if missing_candidates else 0


if __name__ == "__main__":
    sys.exit(main())
