//! Read the user's installed Adobe Color profile; no proprietary tables are bundled.
use crate::dcp::{Encoding, HsvMap, ToneCurve};
use anyhow::{Context, Result, ensure};
use quick_xml::{NsReader, events::Event, name::ResolveResult};
use std::{collections::HashMap, fs, io::Read, path::PathBuf};

pub const NAME: &str = "Adobe Color";
pub const UUID: &str = "B952C231111CD8E0ECCF14B86BAA7077";
const CRS: &[u8] = b"http://ns.adobe.com/camera-raw-settings/1.0/";
const MAX: usize = 16 * 1024 * 1024;
const ALPHABET: &[u8] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#";

pub struct Enhanced {
    pub look: HsvMap,
    pub tone: ToneCurve,
    pub fingerprint: u64,
    pub table_id: String,
    pub encoded_table: String,
    pub points: Vec<[f32; 2]>,
}
fn paths() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dir) = std::env::var_os("FASTCULL_PROFILE_DIR") {
        roots.push(PathBuf::from(dir));
    }
    #[cfg(windows)]
    for key in ["APPDATA", "PROGRAMDATA"] {
        if let Some(dir) = std::env::var_os(key) {
            roots.push(PathBuf::from(dir).join("Adobe/CameraRaw/Settings"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(
                PathBuf::from(home).join("Library/Application Support/Adobe/CameraRaw/Settings"),
            );
        }
        roots.push(PathBuf::from(
            "/Library/Application Support/Adobe/CameraRaw/Settings",
        ));
    }
    roots
        .into_iter()
        .flat_map(|r| {
            [
                r.join("Adobe Color.xmp"),
                r.join("Adobe/Profiles/Adobe Raw/Adobe Color.xmp"),
            ]
        })
        .collect()
}
pub fn read() -> Result<Enhanced> {
    let path = paths().into_iter().find(|p| p.is_file()).context(
        "未找到已安装的 Adobe Color.xmp；请安装 Adobe Camera Raw 或设置 FASTCULL_PROFILE_DIR",
    )?;
    let mut text = String::new();
    fs::File::open(path)?
        .take((MAX + 1) as u64)
        .read_to_string(&mut text)?;
    ensure!(text.len() <= MAX, "增强配置文件过大");
    parse(&text)
}
pub fn available() -> bool {
    paths().into_iter().any(|p| p.is_file())
}
/// Older packets can carry only a display name/UUID. Materialize the installed
/// table once when editing those packets, so ACR receives the same actual Look.
pub fn packet_has_payload(text: &str) -> Result<bool> {
    let expected = read()?;
    let table_key = format!("Table_{}", expected.table_id);
    let mut reader = NsReader::from_str(text);
    let mut data = false;
    let mut reference = false;
    loop {
        let (ns, event) = reader.read_resolved_event()?;
        let crs = matches!(ns,ResolveResult::Bound(n) if n.as_ref()==CRS);
        match event {
            Event::Start(e) | Event::Empty(e) => {
                for attr in e.attributes() {
                    let attr = attr?;
                    if matches!(reader.resolve_attribute(attr.key).0,ResolveResult::Bound(n) if n.as_ref()==CRS)
                    {
                        let local = attr.key.local_name();
                        if local.as_ref() == table_key.as_bytes() {
                            data = attr.decode_and_unescape_value(reader.decoder())?.as_ref()
                                == expected.encoded_table;
                        }
                        if local.as_ref() == b"LookTable" {
                            reference = attr.decode_and_unescape_value(reader.decoder())?.as_ref()
                                == expected.table_id;
                        }
                    }
                }
                if crs && e.local_name().as_ref() == b"LookTable" {
                    reference = reader.read_text(e.name())?.trim() == expected.table_id;
                }
                if crs && e.local_name().as_ref() == table_key.as_bytes() {
                    data = quick_xml::escape::unescape(&reader.read_text(e.name())?)?.as_ref()
                        == expected.encoded_table;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(data && reference)
}
fn parse(text: &str) -> Result<Enhanced> {
    let mut reader = NsReader::from_str(text);
    let mut fields = HashMap::new();
    let mut curve_depth = None;
    let mut depth = 0usize;
    let mut points = Vec::new();
    loop {
        let (ns, ev) = reader.read_resolved_event()?;
        let crs = matches!(ns, ResolveResult::Bound(n) if n.as_ref() == CRS);
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let empty =
                    text.as_bytes().get(reader.buffer_position() as usize - 2) == Some(&b'/');
                if e.local_name().as_ref() == b"Description" && depth <= 2 {
                    for attr in e.attributes() {
                        let attr = attr?;
                        if matches!(reader.resolve_attribute(attr.key).0, ResolveResult::Bound(n) if n.as_ref() == CRS)
                        {
                            let key = String::from_utf8(attr.key.local_name().as_ref().to_vec())?;
                            ensure!(
                                fields
                                    .insert(
                                        key,
                                        attr.decode_and_unescape_value(reader.decoder())?
                                            .into_owned()
                                    )
                                    .is_none(),
                                "增强配置含重复属性"
                            );
                        }
                    }
                }
                if crs && e.local_name().as_ref() == b"ToneCurvePV2012" {
                    curve_depth = Some(depth);
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if curve_depth == Some(depth) {
                    curve_depth = None;
                }
            }
            Event::Text(t) if curve_depth.is_some() => {
                let value = t.decode()?;
                if value.trim().is_empty() {
                    continue;
                }
                let (x, y) = value.split_once(',').context("增强色调曲线点无效")?;
                points.push([
                    x.trim().parse::<f32>()? / 255.,
                    y.trim().parse::<f32>()? / 255.,
                ]);
            }
            Event::DocType(_) => anyhow::bail!("增强配置不支持 DTD"),
            Event::Eof => break,
            _ => {}
        }
        ensure!(depth <= 64, "增强配置嵌套过深");
    }
    ensure!(
        fields.get("UUID").is_some_and(|s| s == UUID),
        "不是支持的 Adobe Color 配置"
    );
    ensure!(
        fields
            .get("CameraProfile")
            .is_some_and(|s| s == "Adobe Standard"),
        "增强配置基础 DCP 不支持"
    );
    let table_id = fields
        .get("LookTable")
        .context("增强配置缺少 LookTable")?
        .clone();
    ensure!(
        table_id.len() == 32 && table_id.bytes().all(|c| c.is_ascii_hexdigit()),
        "增强配置表标识无效"
    );
    let encoded_table = fields
        .get(&format!("Table_{table_id}"))
        .context("增强配置缺少表数据")?
        .clone();
    let look = decode_table(&encoded_table)?;
    let tone = ToneCurve::adobe(points.clone())?;
    let fingerprint = text.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    Ok(Enhanced {
        look,
        tone,
        fingerprint,
        table_id,
        encoded_table,
        points,
    })
}
fn decode_table(text: &str) -> Result<HsvMap> {
    ensure!(text.len() <= MAX, "增强查找表过大");
    let mut packed = Vec::new();
    let mut value = 0u64;
    let mut power = 1u64;
    let mut n = 0;
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let digit = ALPHABET
            .iter()
            .position(|v| *v == c)
            .context("增强查找表编码无效")? as u64;
        value += digit * power;
        power *= 85;
        n += 1;
        if n == 5 {
            ensure!(value <= u32::MAX as u64, "增强查找表编码溢出");
            packed.extend_from_slice(&(value as u32).to_le_bytes());
            value = 0;
            power = 1;
            n = 0;
        }
    }
    ensure!(n != 1, "增强查找表编码截断");
    if n > 1 {
        packed.extend_from_slice(&(value as u32).to_le_bytes()[..n - 1]);
    }
    ensure!(packed.len() >= 5, "增强查找表截断");
    let expected = u32::from_le_bytes(packed[..4].try_into()?) as usize;
    ensure!((24..=MAX).contains(&expected), "增强查找表解压尺寸无效");
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(&packed[4..])
        .take((expected + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() == expected, "增强查找表解压长度不符");
    let word = |i| -> Result<u32> {
        Ok(u32::from_le_bytes(
            bytes.get(i..i + 4).context("增强查找表截断")?.try_into()?,
        ))
    };
    ensure!(
        word(0)? == 0 && matches!(word(4)?, 1 | 2),
        "增强查找表类型或版本不支持"
    );
    let dims = [word(8)? as usize, word(12)? as usize, word(16)? as usize];
    ensure!(
        dims.iter().all(|v| (1..=256).contains(v)),
        "增强查找表维数无效"
    );
    let count = dims[0] * dims[1] * dims[2];
    ensure!(
        count <= 1_048_576 && 24 + count * 12 <= bytes.len(),
        "增强查找表数据无效"
    );
    let mut data = Vec::with_capacity(count);
    for p in bytes[20..20 + count * 12].chunks_exact(12) {
        data.push(std::array::from_fn(|i| {
            f32::from_le_bytes(p[i * 4..i * 4 + 4].try_into().unwrap())
        }));
    }
    let encoding = match word(20 + count * 12)? {
        0 => Encoding::Linear,
        1 => Encoding::Srgb,
        _ => anyhow::bail!("增强查找表编码空间不支持"),
    };
    let tail = &bytes[24 + count * 12..];
    if word(4)? == 2 {
        ensure!(
            tail.len() >= 16
                && f64::from_le_bytes(tail[..8].try_into()?) == 1.
                && f64::from_le_bytes(tail[8..16].try_into()?) == 1.,
            "此增强查找表强度范围不支持"
        );
    }
    HsvMap::new(dims, data, encoding)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn encoded(bytes: &[u8]) -> String {
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(bytes).unwrap();
        let mut packed = (bytes.len() as u32).to_le_bytes().to_vec();
        packed.extend(z.finish().unwrap());
        let mut text = String::new();
        for part in packed.chunks(4) {
            let mut b = [0u8; 4];
            b[..part.len()].copy_from_slice(part);
            let mut v = u32::from_le_bytes(b);
            for _ in 0..part.len() + 1 {
                text.push(ALPHABET[(v % 85) as usize] as char);
                v /= 85;
            }
        }
        text
    }
    #[test]
    fn installed_format_roundtrips_without_bundling_a_table() {
        let mut bytes = Vec::new();
        for v in [0u32, 1, 1, 2, 1] {
            bytes.extend(v.to_le_bytes());
        }
        for v in [0f32, 1., 1., 20., 0.8, 1.1] {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend(0u32.to_le_bytes());
        let table = encoded(&bytes);
        let map = decode_table(&table).unwrap();
        assert_eq!(map.dims, [1, 2, 1]);
        assert_eq!(map.data[1], [20., 0.8, 1.1]);
        let id = "0123456789ABCDEF0123456789ABCDEF";
        let xml = format!(
            "<m:xmpmeta xmlns:m=\"adobe:ns:meta/\"><r:RDF xmlns:r=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><r:Description xmlns:p=\"http://ns.adobe.com/camera-raw-settings/1.0/\" p:UUID=\"{UUID}\" p:CameraProfile=\"Adobe Standard\" p:LookTable=\"{id}\" p:Table_{id}=\"{table}\"><p:ToneCurvePV2012><r:Seq><r:li>0, 0</r:li><r:li>255, 255</r:li></r:Seq></p:ToneCurvePV2012></r:Description></r:RDF></m:xmpmeta>"
        );
        let p = parse(&xml).unwrap();
        assert_eq!(p.look.dims, [1, 2, 1]);
        assert!((p.tone.eval(0.4) - 0.4).abs() < 1e-6);
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_table(&encoded(&bytes)).is_err());
    }
    #[test]
    fn malformed_tables_are_bounded() {
        for text in ["a", "~~~~~", "#####", "0000000000"] {
            assert!(decode_table(text).is_err());
        }
    }
}
