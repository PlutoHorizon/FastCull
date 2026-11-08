//! Read only bounded DNG color metadata, never sensor payloads. Repack selected
//! TIFF entries for the same validated DCP parser used by installed profiles.
use super::{Profile, parse};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const TAGS: &[u16] = &[
    50708, 50721, 50722, 50723, 50724, 50727, 50778, 50779, 50931, 50932, 50936, 50937, 50938,
    50939, 50940, 50964, 50965, 50981, 50982, 51107, 51108, 51109, 51110,
];
const BUDGET: usize = 32 * 1024 * 1024;
struct Reader {
    file: File,
    len: u64,
    remaining: usize,
}
impl Reader {
    fn bytes(&mut self, at: u64, len: usize) -> Result<Vec<u8>> {
        ensure!(
            len <= self.remaining
                && at
                    .checked_add(len as u64)
                    .is_some_and(|end| end <= self.len),
            "内嵌配置文件元数据越界或过大"
        );
        self.remaining -= len;
        self.file.seek(SeekFrom::Start(at))?;
        let mut data = vec![0; len];
        self.file.read_exact(&mut data)?;
        Ok(data)
    }
    fn profile(
        &mut self,
        base: u64,
        little: bool,
        ifd: u32,
    ) -> Result<(Option<Profile>, Vec<u32>, bool)> {
        ensure!(ifd >= 8, "内嵌配置文件目录偏移无效");
        let at = base
            .checked_add(u64::from(ifd))
            .context("内嵌配置目录偏移溢出")?;
        let count = u16_at(&self.bytes(at, 2)?, little) as usize;
        ensure!(count <= 4096, "内嵌配置目录条目过多");
        let entries = self.bytes(at + 2, count * 12 + 4)?;
        let mut tags = BTreeMap::new();
        let mut extra = Vec::new();
        let mut dng = false;
        for entry in entries[..count * 12].chunks_exact(12) {
            let tag = u16_at(entry, little);
            let kind = u16_at(&entry[2..], little);
            let count = u32_at(&entry[4..], little) as usize;
            if tag == 50706 {
                dng = true;
            }
            if !TAGS.contains(&tag) && tag != 50933 {
                continue;
            }
            let unit = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => anyhow::bail!("内嵌配置标签 {tag} 类型无效"),
            };
            let len = count.checked_mul(unit).context("内嵌配置标签长度溢出")?;
            let data = if len <= 4 {
                entry[8..8 + len].to_vec()
            } else {
                let offset = base
                    .checked_add(u64::from(u32_at(&entry[8..], little)))
                    .context("内嵌配置数据偏移溢出")?;
                self.bytes(offset, len)?
            };
            if tag == 50933 {
                ensure!(
                    matches!(kind, 4 | 13) && count <= 64,
                    "内嵌配置列表无效或过长"
                );
                extra = data.chunks_exact(4).map(|v| u32_at(v, little)).collect();
            } else {
                ensure!(
                    tags.insert(tag, (kind, count, data)).is_none(),
                    "内嵌配置包含重复色彩标签"
                );
            }
        }
        if !tags.contains_key(&50721) {
            return Ok((None, extra, dng));
        }
        // Stable contiguous metadata-only TIFF, retaining original numeric type
        // and byte order. Both files and inline values use the normal DCP rules.
        let mut packet = vec![0; 8 + 2 + tags.len() * 12 + 4];
        packet[..2].copy_from_slice(if little { b"II" } else { b"MM" });
        put16(&mut packet[2..], 42, little);
        put32(&mut packet[4..], 8, little);
        put16(&mut packet[8..], tags.len() as u16, little);
        for (i, (tag, (kind, count, data))) in tags.into_iter().enumerate() {
            let pos = 10 + i * 12;
            put16(&mut packet[pos..], tag, little);
            put16(&mut packet[pos + 2..], kind, little);
            put32(&mut packet[pos + 4..], count as u32, little);
            if data.len() <= 4 {
                packet[pos + 8..pos + 8 + data.len()].copy_from_slice(&data);
            } else {
                if !packet.len().is_multiple_of(2) {
                    packet.push(0);
                }
                let offset = packet.len() as u32;
                put32(&mut packet[pos + 8..], offset, little);
                packet.extend(data);
            }
        }
        let mut profile = parse(&packet)?;
        profile.embedded = true;
        if profile.name == "Unnamed DCP" {
            profile.name = "Embedded Camera Profile".into();
        }
        Ok((Some(profile), extra, dng))
    }
}
fn u16_at(b: &[u8], le: bool) -> u16 {
    let v = [b[0], b[1]];
    if le {
        u16::from_le_bytes(v)
    } else {
        u16::from_be_bytes(v)
    }
}
fn u32_at(b: &[u8], le: bool) -> u32 {
    let v = [b[0], b[1], b[2], b[3]];
    if le {
        u32::from_le_bytes(v)
    } else {
        u32::from_be_bytes(v)
    }
}
fn put16(b: &mut [u8], v: u16, le: bool) {
    b[..2].copy_from_slice(&if le { v.to_le_bytes() } else { v.to_be_bytes() });
}
fn put32(b: &mut [u8], v: u32, le: bool) {
    b[..4].copy_from_slice(&if le { v.to_le_bytes() } else { v.to_be_bytes() });
}

pub fn read(path: &Path) -> Result<Vec<Profile>> {
    if !path
        .extension()
        .is_some_and(|v| v.eq_ignore_ascii_case("dng"))
    {
        return Ok(Vec::new());
    }
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut reader = Reader {
        file,
        len,
        remaining: BUDGET,
    };
    let head = reader.bytes(0, 8)?;
    ensure!(
        matches!(&head[..2], b"II" | b"MM"),
        "DNG 内嵌配置字节序无效"
    );
    let le = &head[..2] == b"II";
    ensure!(u16_at(&head[2..], le) == 42, "DNG 内嵌配置暂不支持 BigTIFF");
    let (profile, extra, is_dng) = reader.profile(0, le, u32_at(&head[4..], le))?;
    ensure!(is_dng, "DNG 文件缺少 DNGVersion 标记");
    let mut result: Vec<_> = profile.into_iter().collect();
    let mut seen = std::collections::HashSet::new();
    for offset in extra {
        ensure!(
            offset >= 8 && seen.insert(offset),
            "DNG 附加配置偏移重复或无效"
        );
        let head = reader.bytes(u64::from(offset), 8)?;
        ensure!(
            matches!(&head[..2], b"II" | b"MM"),
            "DNG 附加配置字节序无效"
        );
        let le = &head[..2] == b"II";
        ensure!(u16_at(&head[2..], le) == 0x4352, "DNG 附加配置标记无效");
        let (profile, _, _) = reader.profile(u64::from(offset), le, u32_at(&head[4..], le))?;
        if let Some(profile) = profile {
            let mut profile = profile;
            if let Some(primary) = result.first() {
                profile.analog_balance = primary.analog_balance;
                profile.calibration = primary.calibration;
                profile.camera_calibration_signature = primary.camera_calibration_signature.clone();
                profile.fingerprint ^= primary.fingerprint.rotate_left(17);
            }
            result.push(profile);
        }
    }
    for profile in &mut result {
        profile.path = Some(path.to_path_buf());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(le: bool) -> Vec<u8> {
        // DNGVersion, UniqueCameraModel, ColorMatrix1, CalibrationIlluminant1.
        let mut b = vec![0; 62];
        b[..2].copy_from_slice(if le { b"II" } else { b"MM" });
        put16(&mut b[2..], 42, le);
        put32(&mut b[4..], 8, le);
        put16(&mut b[8..], 4, le);
        for (i, (tag, kind, count)) in [(50706, 1, 4), (50708, 2, 5), (50721, 10, 9), (50778, 3, 1)]
            .into_iter()
            .enumerate()
        {
            let at = 10 + i * 12;
            put16(&mut b[at..], tag, le);
            put16(&mut b[at + 2..], kind, le);
            put32(&mut b[at + 4..], count, le);
            match tag {
                50706 => b[at + 8..at + 12].copy_from_slice(&[1, 4, 0, 0]),
                50778 => put16(&mut b[at + 8..], 21, le),
                _ => {
                    let offset = b.len() as u32;
                    put32(&mut b[at + 8..], offset, le);
                    if tag == 50708 {
                        b.extend(b"Test\0");
                    } else {
                        for n in 0..9 {
                            let mut q = [0; 8];
                            put32(&mut q, if n % 4 == 0 { 1 } else { 0 }, le);
                            put32(&mut q[4..], 1, le);
                            b.extend(q);
                        }
                    }
                }
            }
        }
        b
    }
    #[test]
    fn reads_both_orders_without_loading_large_sensor_payloads() {
        for le in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("private.dng");
            std::fs::write(&path, fixture(le)).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(128 * 1024 * 1024)
                .unwrap();
            let profiles = read(&path).unwrap();
            assert_eq!(profiles.len(), 1);
            assert!(profiles[0].embedded);
            assert_eq!(profiles[0].model.as_deref(), Some("Test"));
            assert_eq!(profiles[0].color[0], crate::white_balance::IDENTITY);
            let values = crate::xmp::Values::default();
            let selected = crate::raw_color::find_profile_for_source(
                &path,
                "FastCull",
                "__synthetic_embedded_test__",
                &values,
            )
            .unwrap()
            .unwrap();
            assert!(selected.embedded);
            let explicit = crate::xmp::Values {
                camera_profile: Some("Embedded".into()),
                ..Default::default()
            };
            assert!(
                crate::raw_color::find_profile_for_source(
                    &path,
                    "FastCull",
                    "__synthetic_embedded_test__",
                    &explicit
                )
                .unwrap()
                .unwrap()
                .embedded
            );
            let other = crate::xmp::Values {
                camera_profile: Some("Another Test Profile".into()),
                ..Default::default()
            };
            assert!(
                crate::raw_color::find_profile_for_source(
                    &path,
                    "FastCull",
                    "__synthetic_embedded_test__",
                    &other
                )
                .unwrap()
                .is_none()
            );
        }
    }
    #[test]
    fn rejects_bad_offsets_and_oversized_profile_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.dng");
        let mut bytes = fixture(true);
        put32(&mut bytes[4..], u32::MAX, true);
        std::fs::write(&path, &bytes).unwrap();
        assert!(read(&path).is_err());
        bytes = fixture(true);
        put32(&mut bytes[38..], u32::MAX, true);
        std::fs::write(&path, &bytes).unwrap();
        assert!(read(&path).is_err());
    }
}
