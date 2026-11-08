//! Read-only, bounded MakerNote focus coordinates. No image-content inference.
//! Layout references: ExifTool Sony.pm (2027/202a), Nikon.pm (AFInfo2),
//! Canon.pm (AFInfo2). Unsupported layouts deliberately return no coordinates.
use anyhow::{Context, Result, ensure};
use eframe::egui::{Pos2, Rect, Vec2};
use std::{
    collections::HashSet,
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, Debug)]
pub struct Area {
    /// Coordinates in the un-oriented image, normalized to [0, 1].
    pub center: Pos2,
    /// None means the camera records a point, not a measured rectangle.
    pub size: Option<Vec2>,
}
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub areas: Vec<Area>,
    pub source: &'static str,
}
impl Info {
    fn add(&mut self, dimensions: [u16; 2], center: [u16; 2], size: Option<[u16; 2]>) {
        let [w, h] = dimensions.map(f32::from);
        let [x, y] = center.map(f32::from);
        if w <= 0. || h <= 0. || x > w || y > h {
            return;
        }
        self.areas.push(Area {
            center: Pos2::new(x / w, y / h),
            size: size
                .filter(|s| s[0] > 0 && s[1] > 0 && s[0] <= dimensions[0] && s[1] <= dimensions[1])
                .map(|s| Vec2::new(f32::from(s[0]) / w, f32::from(s[1]) / h)),
        });
    }
}

struct Reader<R> {
    input: R,
    budget: usize,
}
impl<R: Read + Seek> Reader<R> {
    fn bytes(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        ensure!(len <= self.budget, "focus metadata exceeds read budget");
        self.budget -= len;
        self.input.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0; len];
        self.input.read_exact(&mut data)?;
        Ok(data)
    }
    fn entries(&mut self, base: u64, off: u32, little: bool) -> Result<Vec<Entry>> {
        let pos = base.checked_add(u64::from(off)).context("IFD overflow")?;
        let n = usize::from(u16_at(&self.bytes(pos, 2)?, 0, little)?);
        ensure!(n <= 2048, "too many focus metadata entries");
        let data = self.bytes(pos + 2, n * 12)?;
        data.chunks_exact(12)
            .enumerate()
            .map(|(i, e)| {
                let tag = u16_at(e, 0, little)?;
                let typ = u16_at(e, 2, little)?;
                let count = u32_at(e, 4, little)? as usize;
                let unit = match typ {
                    1 | 2 | 7 => 1,
                    3 => 2,
                    4 | 9 => 4,
                    _ => 0,
                };
                let len = count.checked_mul(unit).context("tag size overflow")?;
                let offset = if len <= 4 {
                    pos + 2 + (i * 12) as u64 + 8
                } else {
                    base.checked_add(u64::from(u32_at(e, 8, little)?))
                        .context("tag offset overflow")?
                };
                Ok(Entry {
                    tag,
                    typ,
                    len,
                    offset,
                })
            })
            .collect()
    }
    fn tiff(&mut self, base: u64) -> Result<(bool, u32)> {
        let h = self.bytes(base, 8)?;
        let little = match &h[..2] {
            b"II" => true,
            b"MM" => false,
            _ => anyhow::bail!("not TIFF"),
        };
        ensure!(u16_at(&h, 2, little)? == 42, "unsupported TIFF");
        Ok((little, u32_at(&h, 4, little)?))
    }
    fn parse(&mut self) -> Result<Info> {
        let (little, first) = self.tiff(0)?;
        let mut queue = vec![first];
        let mut seen = HashSet::new();
        let mut make = String::new();
        let mut maker = None;
        while let Some(off) = queue.pop() {
            if off == 0 || !seen.insert(off) {
                continue;
            }
            ensure!(seen.len() <= 16, "too many focus IFDs");
            for e in self.entries(0, off, little)? {
                match e.tag {
                    0x010f if e.typ == 2 && e.len <= 256 => {
                        make = String::from_utf8_lossy(&self.bytes(e.offset, e.len)?)
                            .to_ascii_uppercase()
                    }
                    0x8769 if e.typ == 4 && e.len == 4 => {
                        queue.push(u32_at(&self.bytes(e.offset, 4)?, 0, little)?)
                    }
                    0x927c if e.len >= 12 => maker = Some(e),
                    _ => {}
                }
            }
        }
        let Some(maker) = maker else {
            return Ok(Info::default());
        };
        let signature = self.bytes(maker.offset, 12)?;
        if make.contains("SONY") || signature.starts_with(b"SONY") {
            let skip = if signature.starts_with(b"SONY DSC ") || signature.starts_with(b"SONY CAM ")
            {
                12
            } else {
                0
            };
            let off = u32::try_from(maker.offset + skip)?;
            let entries = self.entries(0, off, little)?;
            let mut location = Info::default();
            for e in &entries {
                if e.tag == 0x2027 && e.typ == 3 && e.len == 8 {
                    let d = self.bytes(e.offset, e.len)?;
                    location.source = "相机记录位置（可能为回放默认中心）";
                    location.add(pair(&d, 0, little)?, pair(&d, 4, little)?, None);
                }
            }
            for e in &entries {
                if e.tag == 0x202a && (6..=1024).contains(&e.len) {
                    let d = self.bytes(e.offset, e.len)?;
                    let count = usize::from(d[1]);
                    if d[0] != 1 || !(1..=15).contains(&count) || d.len() < 6 + count * 4 {
                        continue;
                    }
                    let mut points = Info {
                        source: "相机相位对焦点",
                        ..Default::default()
                    };
                    for i in 0..count {
                        points.add(pair(&d, 2, little)?, pair(&d, 6 + i * 4, little)?, None);
                    }
                    if !points.areas.is_empty() {
                        return Ok(points);
                    }
                }
            }
            return Ok(location);
        }
        if make.contains("NIKON") && signature.starts_with(b"Nikon\0\x02") {
            let base = maker.offset + 10;
            let (little, off) = self.tiff(base)?;
            for e in self.entries(base, off, little)? {
                if e.tag == 0x00b7 && (30..=65536).contains(&e.len) {
                    return nikon(&self.bytes(e.offset, e.len)?, little);
                }
            }
        }
        if make.contains("CANON") {
            for e in self.entries(0, u32::try_from(maker.offset)?, little)? {
                if e.tag == 0x26 && (20..=65536).contains(&e.len) {
                    return canon(&self.bytes(e.offset, e.len)?, little);
                }
            }
        }
        Ok(Info::default())
    }
}
struct Entry {
    tag: u16,
    typ: u16,
    len: usize,
    offset: u64,
}
fn u16_at(d: &[u8], o: usize, little: bool) -> Result<u16> {
    let v = d.get(o..o + 2).context("truncated focus u16")?.try_into()?;
    Ok(if little {
        u16::from_le_bytes(v)
    } else {
        u16::from_be_bytes(v)
    })
}
fn u32_at(d: &[u8], o: usize, little: bool) -> Result<u32> {
    let v = d.get(o..o + 4).context("truncated focus u32")?.try_into()?;
    Ok(if little {
        u32::from_le_bytes(v)
    } else {
        u32::from_be_bytes(v)
    })
}
fn pair(d: &[u8], o: usize, little: bool) -> Result<[u16; 2]> {
    Ok([u16_at(d, o, little)?, u16_at(d, o + 2, little)?])
}
fn nikon(d: &[u8], little: bool) -> Result<Info> {
    ensure!(d.len() >= 8, "truncated Nikon AF header");
    let mut info = Info {
        source: "相机对焦区域",
        ..Default::default()
    };
    let version = d.get(..4).context("missing Nikon AF version")?;
    let offset = match version {
        b"0100" if d[4] == 1 => 0x10,
        b"0101" if d[4] == 1 => 0x46,
        b"0300" | b"0301" if d[7] == 1 => 0x2a,
        b"0400" | b"0401" | b"0402" if d[7] == 1 => 0x3e,
        _ => return Ok(info),
    };
    info.add(
        pair(d, offset, little)?,
        pair(d, offset + 4, little)?,
        Some(pair(d, offset + 8, little)?),
    );
    Ok(info)
}
fn canon(d: &[u8], little: bool) -> Result<Info> {
    let mut info = Info {
        source: "相机合焦框",
        ..Default::default()
    };
    let n = usize::from(u16_at(d, 4, little)?);
    let valid = usize::from(u16_at(d, 6, little)?);
    if n == 0 || n > 1024 || valid > n || u16_at(d, 2, little)? == 0 {
        return Ok(info);
    }
    let masks = n.div_ceil(16);
    ensure!(
        d.len() >= (8 + n * 4 + masks) * 2,
        "truncated Canon AF arrays"
    );
    let dimensions = pair(d, 12, little)?;
    for i in 0..valid {
        if u16_at(d, (8 + n * 4 + i / 16) * 2, little)? & (1 << (i % 16)) == 0 {
            continue;
        }
        let x = i32::from(u16_at(d, (8 + n * 2 + i) * 2, little)? as i16)
            + i32::from(dimensions[0]) / 2;
        let y = i32::from(dimensions[1]) / 2
            - i32::from(u16_at(d, (8 + n * 3 + i) * 2, little)? as i16);
        if let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) {
            info.add(
                dimensions,
                [x, y],
                Some([
                    u16_at(d, (8 + i) * 2, little)?,
                    u16_at(d, (8 + n + i) * 2, little)?,
                ]),
            );
        }
    }
    Ok(info)
}
pub fn read(path: &Path) -> Result<Info> {
    let mut file = File::open(path)?;
    if crate::formats::is_jpeg(path) {
        let header = crate::jpeg::header(&mut file)?;
        for segment in header.app1 {
            if segment.payload.starts_with(b"Exif\0\0") {
                return Reader {
                    input: Cursor::new(&segment.payload[6..]),
                    budget: 2 * 1024 * 1024,
                }
                .parse();
            }
        }
        return Ok(Info::default());
    }
    Reader {
        input: file,
        budget: 2 * 1024 * 1024,
    }
    .parse()
}

/// Invert the exact affine UV transform used to paint the photo. This covers
/// all EXIF mirrors/rotations as well as leveled and noncentral XMP crops.
pub fn project(point: Pos2, uvs: [Pos2; 4], screen: Rect) -> Option<Pos2> {
    let a = uvs[1] - uvs[0];
    let b = uvs[3] - uvs[0];
    let v = point - uvs[0];
    let determinant = a.x * b.y - a.y * b.x;
    if determinant.abs() < 1e-9 {
        return None;
    }
    let uv = Vec2::new(
        (v.x * b.y - v.y * b.x) / determinant,
        (a.x * v.y - a.y * v.x) / determinant,
    );
    Some(screen.min + uv * screen.size())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sony(little: bool, points: bool) -> Vec<u8> {
        let mut data = vec![0; 300];
        data[..2].copy_from_slice(if little { b"II" } else { b"MM" });
        let w16 = |d: &mut [u8], o, v: u16| {
            d[o..o + 2].copy_from_slice(&if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            })
        };
        let w32 = |d: &mut [u8], o, v: u32| {
            d[o..o + 4].copy_from_slice(&if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            })
        };
        w16(&mut data, 2, 42);
        w32(&mut data, 4, 8);
        let entry = |d: &mut [u8], o, tag, typ, count, value| {
            w16(d, o, tag);
            w16(d, o + 2, typ);
            w32(d, o + 4, count);
            w32(d, o + 8, value);
        };
        w16(&mut data, 8, 2);
        entry(&mut data, 10, 0x10f, 2, 5, 48);
        entry(&mut data, 22, 0x8769, 4, 1, 64);
        data[48..53].copy_from_slice(b"SONY\0");
        w16(&mut data, 64, 1);
        entry(&mut data, 66, 0x927c, 7, 150, 100);
        data[100..112].copy_from_slice(b"SONY DSC \0\0\0");
        w16(&mut data, 112, if points { 2 } else { 1 });
        entry(&mut data, 114, 0x2027, 3, 4, 160);
        for (i, v) in [6000, 4000, 4200, 1200].into_iter().enumerate() {
            w16(&mut data, 160 + i * 2, v);
        }
        if points {
            entry(&mut data, 126, 0x202a, 7, 14, 180);
            data[180] = 1;
            data[181] = 2;
            for (i, v) in [640, 428, 320, 214, 160, 107].into_iter().enumerate() {
                w16(&mut data, 182 + i * 2, v);
            }
        }
        data
    }
    #[test]
    fn sony_endianness_and_phase_points_take_precedence_over_playback_location() {
        for little in [true, false] {
            for points in [true, false] {
                let mut reader = Reader {
                    input: Cursor::new(sony(little, points)),
                    budget: 100_000,
                };
                let info = reader.parse().unwrap();
                assert_eq!(info.areas.len(), if points { 2 } else { 1 });
                let expected = if points {
                    Pos2::new(0.5, 0.5)
                } else {
                    Pos2::new(0.7, 0.3)
                };
                assert!((info.areas[0].center - expected).length() < 1e-6);
                assert!(info.areas[0].size.is_none());
            }
        }
    }
    #[test]
    fn reads_tiff_and_jpeg_exif_without_external_tools() {
        let directory = tempfile::tempdir().unwrap();
        let tiff = sony(true, true);
        let raw = directory.path().join("focus.ARW");
        std::fs::write(&raw, &tiff).unwrap();
        assert_eq!(read(&raw).unwrap().areas.len(), 2);
        let mut jpeg = vec![255, 216, 255, 225];
        jpeg.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&include_bytes!("../tests/fixtures/tiny.jpg")[2..]);
        let path = directory.path().join("focus.JPG");
        std::fs::write(&path, jpeg).unwrap();
        assert_eq!(read(&path).unwrap().areas.len(), 2);
    }
    #[test]
    fn nikon_versioned_coordinates_and_unavailable_coordinates() {
        for (version, offset) in [
            (b"0100", 0x10),
            (b"0101", 0x46),
            (b"0301", 0x2a),
            (b"0402", 0x3e),
        ] {
            for little in [true, false] {
                let mut d = vec![0; 128];
                d[..4].copy_from_slice(version);
                d[4] = 1;
                d[7] = 1;
                for (i, v) in [6000u16, 4000, 4200, 1200, 300, 200]
                    .into_iter()
                    .enumerate()
                {
                    d[offset + i * 2..offset + i * 2 + 2].copy_from_slice(&if little {
                        v.to_le_bytes()
                    } else {
                        v.to_be_bytes()
                    });
                }
                let info = nikon(&d, little).unwrap();
                assert_eq!(info.areas.len(), 1);
                assert_eq!(info.areas[0].center, Pos2::new(0.7, 0.3));
                assert_eq!(info.areas[0].size, Some(Vec2::splat(0.05)));
                d[4] = 0;
                d[7] = 0;
                assert!(nikon(&d, little).unwrap().areas.is_empty());
            }
        }
    }
    #[test]
    fn canon_only_displays_in_focus_points_with_signed_centered_coordinates() {
        let words: [u16; 17] = [
            34,
            2,
            2,
            2,
            6000,
            4000,
            6000,
            4000,
            300,
            400,
            200,
            300,
            1200,
            (-1200i16) as u16,
            800,
            (-800i16) as u16,
            1,
        ];
        let data: Vec<u8> = words.into_iter().flat_map(u16::to_le_bytes).collect();
        let info = canon(&data, true).unwrap();
        assert_eq!(info.areas.len(), 1);
        assert_eq!(info.areas[0].center, Pos2::new(0.7, 0.3));
        assert_eq!(info.areas[0].size, Some(Vec2::splat(0.05)));
    }
    #[test]
    fn projection_roundtrips_all_orientations_and_rotated_off_center_crops() {
        let size = Vec2::new(6000., 4000.);
        let screen = Rect::from_min_size(Pos2::new(-800., -300.), Vec2::new(2400., 1600.));
        for o in 1..=8 {
            for crop in [
                crate::crop::Crop::default(),
                crate::crop::Crop {
                    enabled: true,
                    left: 150_000,
                    top: 200_000,
                    right: 800_000,
                    bottom: 750_000,
                    angle: 12_000_000,
                },
            ] {
                let raw = crop.raw_uvs(size);
                let uvs = crate::crop::order(o).map(|i| raw[i]);
                for uv in [Vec2::splat(0.5), Vec2::new(0.1, 0.8), Vec2::new(-0.1, 1.2)] {
                    let point = uvs[0] + (uvs[1] - uvs[0]) * uv.x + (uvs[3] - uvs[0]) * uv.y;
                    let actual = project(point, uvs, screen).unwrap();
                    assert!((actual - (screen.min + screen.size() * uv)).length() < 0.002);
                }
            }
        }
    }
    #[test]
    fn truncated_or_cyclic_tiffs_fail_safely_and_reads_are_bounded() {
        let data = sony(true, true);
        for n in 0..data.len() {
            let mut reader = Reader {
                input: Cursor::new(&data[..n]),
                budget: 65536,
            };
            let _ = reader.parse();
        }
        assert!(
            Reader {
                input: Cursor::new(data),
                budget: 8
            }
            .parse()
            .is_err()
        );
        assert!(project(Pos2::ZERO, [Pos2::ZERO; 4], Rect::EVERYTHING).is_none());
    }
}
