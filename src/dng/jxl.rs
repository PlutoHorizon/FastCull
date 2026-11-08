//! DNG 1.7 JPEG XL: rendered previews stay cheap; integer RAW tiles are
//! unpacked into a private TIFF copy for the existing LibRaw color pipeline.
//! The source and its metadata are never written. Only one converted source
//! is retained per process, and tempfile removes it when no decoder owns it.
use super::*;
use crate::imaging::{self, Level, Pixels};
use jxl_oxide::{AllocTracker, EnumColourEncoding, JxlImage, RenderingIntent};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

const JXL: u32 = 52546;
#[derive(Clone, Debug)]
struct Polynomial {
    area: [u32; 8],
    coefficients: Vec<f64>,
}
fn polynomials(r: &mut Reader, d: &Directory) -> Result<Vec<Polynomial>> {
    let mut result = Vec::new();
    for e in d
        .entries
        .iter()
        .filter(|e| matches!(e.tag, 51008 | 51009 | 51022))
    {
        ensure!(e.kind == 7, "invalid DNG opcode list type");
        let bytes = r.data(e, 1, MAX_METADATA as u32)?;
        let word = |p: usize| -> Result<u32> {
            Ok(u32::from_be_bytes(
                bytes
                    .get(p..p + 4)
                    .context("truncated DNG opcode")?
                    .try_into()?,
            ))
        };
        let count = word(0)?;
        ensure!(count <= 1024, "too many DNG opcodes");
        let mut p = 4;
        for _ in 0..count {
            let id = word(p)?;
            let version = word(p + 4)?;
            let flags = word(p + 8)?;
            let len = word(p + 12)? as usize;
            p += 16;
            let end = p.checked_add(len).context("DNG opcode length overflow")?;
            ensure!(end <= bytes.len(), "DNG opcode outside bounds");
            if e.tag == 51009 && id == 8 && version <= 0x01030000 {
                ensure!(len >= 44, "truncated MapPolynomial opcode");
                let mut area = [0; 8];
                for (i, v) in area.iter_mut().enumerate() {
                    *v = word(p + i * 4)?;
                }
                let degree = word(p + 32)? as usize;
                ensure!(
                    degree <= 8
                        && len == 36 + (degree + 1) * 8
                        && area[0] < area[2]
                        && area[1] < area[3]
                        && area[2] <= d.height
                        && area[3] <= d.width
                        && area[5] > 0
                        && area[4].checked_add(area[5]).is_some_and(|v| v <= d.samples)
                        && area[6] > 0
                        && area[7] > 0,
                    "invalid MapPolynomial area/degree"
                );
                let coefficients = bytes[p + 36..end]
                    .chunks_exact(8)
                    .map(|b| f64::from_be_bytes(b.try_into().unwrap()))
                    .collect::<Vec<_>>();
                ensure!(
                    coefficients.iter().all(|v| v.is_finite()),
                    "invalid MapPolynomial coefficient"
                );
                result.push(Polynomial { area, coefficients });
            } else {
                ensure!(
                    flags & 1 != 0,
                    "unsupported required DNG opcode {id} in list {}",
                    e.tag
                );
            }
            p = end;
        }
        ensure!(p == bytes.len(), "trailing DNG opcode bytes");
    }
    if !result.is_empty() {
        // Stage 2 opcodes consume normalized, linearized samples. This path
        // can bake them losslessly in position for zero-black/full-white
        // integer images, as emitted by ACR panorama/lossy DNG compression.
        ensure!(
            !d.entries
                .iter()
                .any(|e| matches!(e.tag, 50712 | 50715 | 50716)),
            "DNG opcode mapping with linearization/black deltas is unsupported"
        );
        for e in d.entries.iter().filter(|e| matches!(e.tag, 50714 | 50717)) {
            let expected = if e.tag == 50714 {
                0.
            } else {
                (1u32 << d.bits) as f64 - 1.
            };
            ensure!(
                r.numbers(e, 16)?.iter().all(|v| *v == expected),
                "DNG opcode mapping requires zero black and full-range white"
            );
        }
    }
    Ok(result)
}
fn mapped_sample(sample: u16, x: u32, y: u32, channel: u32, maps: &[Polynomial]) -> u16 {
    let mut v = f64::from(sample) / 65535.;
    for map in maps {
        let a = map.area;
        if y >= a[0]
            && y < a[2]
            && x >= a[1]
            && x < a[3]
            && channel >= a[4]
            && channel < a[4] + a[5]
            && (y - a[0]).is_multiple_of(a[6])
            && (x - a[1]).is_multiple_of(a[7])
        {
            v = map
                .coefficients
                .iter()
                .rev()
                .fold(0., |acc, c| acc * v + c)
                .clamp(0., 1.);
        }
    }
    (v * 65535.).round().clamp(0., 65535.) as u16
}
fn sample_luts(d: &Directory, maps: &[Polynomial]) -> Option<Vec<Vec<u16>>> {
    if maps.is_empty()
        || maps
            .iter()
            .any(|m| m.area[..4] != [0, 0, d.height, d.width] || m.area[6..] != [1, 1])
    {
        return None;
    }
    Some(
        (0..d.samples)
            .map(|channel| {
                (0..=u16::MAX)
                    .map(|sample| mapped_sample(sample, 0, 0, channel, maps))
                    .collect()
            })
            .collect(),
    )
}
struct Directory {
    offset: u32,
    entries: Vec<Entry>,
    width: u32,
    height: u32,
    samples: u32,
    bits: u32,
    subfile: u32,
    photometric: u32,
    tile: Option<(u32, u32)>,
    offsets: Vec<u64>,
    lengths: Vec<u64>,
}
fn scalar(r: &Reader, entries: &[Entry], tag: u16) -> Option<u32> {
    entries
        .iter()
        .find(|e| e.tag == tag)
        .and_then(|e| r.number(e))
}
#[derive(Clone, Copy)]
enum Kind {
    Raw,
    Preview,
    Mask,
}
fn directories(r: &mut Reader, kind: Kind) -> Result<Vec<Directory>> {
    let mut queue = vec![r.first];
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    while let Some(offset) = queue.pop() {
        if offset == 0 || !seen.insert(offset) {
            continue;
        }
        ensure!(seen.len() <= 256, "too many DNG IFDs");
        let (entries, next) = r.ifd(offset)?;
        queue.push(next);
        for e in entries.iter().filter(|e| e.tag == 330) {
            queue.extend(r.numbers(e, 256)?.into_iter().map(|v| v as u32));
        }
        if scalar(r, &entries, 259) != Some(JXL) {
            continue;
        }
        let subfile = scalar(r, &entries, 254).unwrap_or(0);
        let photometric = scalar(r, &entries, 262).unwrap_or(0);
        // Transparency, depth and semantic masks never become image previews.
        if !(match kind {
            Kind::Raw => subfile == 0 && matches!(photometric, 32803 | 34892),
            Kind::Preview => subfile == 1 && photometric == 2,
            Kind::Mask => subfile == 4 && photometric == 4,
        }) {
            continue;
        }
        let (width, height) = dimensions(scalar(r, &entries, 256), scalar(r, &entries, 257))
            .context("invalid JPEG XL DNG dimensions")?;
        let samples = scalar(r, &entries, 277).unwrap_or(1);
        ensure!(matches!(samples, 1 | 3), "unsupported JPEG XL DNG channels");
        ensure!(
            scalar(r, &entries, 284).unwrap_or(1) == 1,
            "unsupported JPEG XL DNG planar layout"
        );
        if let Some(format) = entries.iter().find(|e| e.tag == 339) {
            ensure!(
                r.numbers(format, 4)?.iter().all(|v| *v == 1.),
                "unsupported JPEG XL DNG sample format"
            );
        }
        ensure!(
            scalar(r, &entries, 317).unwrap_or(1) == 1,
            "unsupported JPEG XL DNG predictor"
        );
        let bits = entries
            .iter()
            .find(|e| e.tag == 258)
            .context("missing DNG bit depth")?;
        let values = r.numbers(bits, 4)?;
        let bits = *values.first().context("empty DNG bit depth")? as u32;
        ensure!(
            matches!(bits, 8 | 16) && values.iter().all(|v| *v == bits as f64),
            "unsupported JPEG XL DNG bit depth"
        );
        let tile = scalar(r, &entries, 322).zip(scalar(r, &entries, 323));
        let tags = if tile.is_some() {
            (324, 325)
        } else {
            (273, 279)
        };
        let mut array = |tag| -> Result<Vec<u64>> {
            let e = entries
                .iter()
                .find(|e| e.tag == tag)
                .context("missing DNG image range")?;
            ensure!(matches!(e.kind, 3 | 4), "invalid DNG image range type");
            Ok(r.numbers(e, 65536)?.into_iter().map(|v| v as u64).collect())
        };
        let offsets = array(tags.0)?;
        let lengths = array(tags.1)?;
        ensure!(
            !offsets.is_empty() && offsets.len() == lengths.len(),
            "invalid DNG image ranges"
        );
        for (&off, &len) in offsets.iter().zip(&lengths) {
            ensure!(
                len > 0
                    && len <= MAX_JPEG
                    && off >= 8
                    && off.checked_add(len).is_some_and(|end| end <= r.size),
                "DNG image outside bounds"
            );
        }
        found.push(Directory {
            offset,
            entries,
            width,
            height,
            samples,
            bits,
            subfile,
            photometric,
            tile,
            offsets,
            lengths,
        });
    }
    Ok(found)
}

fn read_image(r: &mut Reader, off: u64, len: u64, size: (u32, u32)) -> Result<JxlImage> {
    r.file.seek(SeekFrom::Start(off))?;
    let image = JxlImage::builder()
        .alloc_tracker(AllocTracker::with_limit(512 * 1024 * 1024))
        .read((&mut r.file).take(len))
        .map_err(anyhow::Error::from_boxed)?;
    ensure!(
        (image.width(), image.height()) == size && image.image_header().metadata.orientation == 1,
        "JPEG XL dimensions/orientation differ from DNG tile"
    );
    ensure!(
        !image.image_header().metadata.xyb_encoded || image.original_icc().is_none(),
        "JPEG XL RAW ICC conversion requires a color management system"
    );
    Ok(image)
}

pub(crate) fn preview(
    path: &Path,
    level: Level,
    geometry: &mut dyn FnMut([usize; 2]),
) -> Result<Option<Pixels>> {
    let mut r = Reader::open(path)?;
    let dirs = directories(&mut r, Kind::Preview)?;
    let Some(d) = dirs
        .iter()
        .filter(|d| {
            d.subfile == 1
                && d.photometric == 2
                && d.samples == 3
                && d.tile.is_none()
                && d.offsets.len() == 1
        })
        .max_by_key(|d| u64::from(d.width) * u64::from(d.height))
    else {
        return Ok(None);
    };
    // Rendered RGB previews already include ACR's recipe. RAW/LinearRaw
    // previews require the DNG color pipeline and are deliberately excluded.
    let mut image = read_image(&mut r, d.offsets[0], d.lengths[0], (d.width, d.height))?;
    image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
    let rendered = image.render_frame(0).map_err(anyhow::Error::from_boxed)?;
    let mut stream = rendered.stream_no_alpha();
    ensure!(stream.channels() == 3, "invalid JPEG XL RGB preview");
    let count = d.width as usize * d.height as usize;
    let mut rgb = vec![0u8; count * 3];
    ensure!(
        stream.write_to_buffer(&mut rgb) == rgb.len(),
        "incomplete JPEG XL preview"
    );
    let mut rgba = Vec::with_capacity(count * 4);
    for p in rgb.chunks_exact(3) {
        rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
    }
    geometry([d.width as usize, d.height as usize]);
    Ok(Some(imaging::resize_level(
        Pixels {
            width: d.width as usize,
            height: d.height as usize,
            rgba,
        },
        level,
    )?))
}

/// Apply the panorama's full-resolution transparency after native development.
/// Mask tiles have their own photometry and are never interpreted as RGB.
pub(crate) fn mask(path: &Path, pixels: &mut Pixels) -> Result<()> {
    if !crate::formats::is_dng(path) {
        return Ok(());
    }
    let Ok(mut r) = Reader::open(path) else {
        return Ok(());
    };
    let dirs = directories(&mut r, Kind::Mask)?;
    let Some(d) = dirs.first() else {
        return Ok(());
    };
    ensure!(
        d.samples == 1 && (d.width as usize, d.height as usize) == (pixels.width, pixels.height),
        "DNG transparency dimensions differ from native image"
    );
    let (tw, th) = d.tile.unwrap_or((d.width, d.height));
    ensure!(
        tw > 0 && th > 0 && u64::from(tw) * u64::from(th) <= 16_000_000,
        "DNG transparency tile too large"
    );
    let columns = d.width.div_ceil(tw);
    ensure!(
        d.offsets.len() == (columns * d.height.div_ceil(th)) as usize,
        "incomplete DNG transparency tile grid"
    );
    for (index, (&off, &len)) in d.offsets.iter().zip(&d.lengths).enumerate() {
        let image = read_image(&mut r, off, len, (tw, th))?;
        let rendered = image.render_frame(0).map_err(anyhow::Error::from_boxed)?;
        let mut stream = rendered.stream_no_alpha();
        ensure!(stream.channels() == 1, "invalid DNG transparency channels");
        let mut alpha = vec![0u8; tw as usize * th as usize];
        ensure!(
            stream.write_to_buffer(&mut alpha) == alpha.len(),
            "incomplete DNG transparency"
        );
        let (x, y) = (index as u32 % columns * tw, index as u32 / columns * th);
        for dy in 0..(d.height - y).min(th) {
            for dx in 0..(d.width - x).min(tw) {
                let target = ((y + dy) as usize * pixels.width + (x + dx) as usize) * 4;
                let a = alpha[(dy * tw + dx) as usize];
                if a == 0 {
                    pixels.rgba[target..target + 3].fill(0);
                }
                pixels.rgba[target + 3] = a;
            }
        }
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Key {
    path: PathBuf,
    length: u64,
    modified: std::time::SystemTime,
}
type Cached = (Key, Arc<tempfile::TempPath>);
static CONVERTED: Mutex<Option<Cached>> = Mutex::new(None);

pub(crate) fn native_source(
    path: &Path,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Arc<tempfile::TempPath>>> {
    native_source_in(path, None, cancelled)
}
fn native_source_in(
    path: &Path,
    directory: Option<&Path>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Arc<tempfile::TempPath>>> {
    if !crate::formats::is_dng(path) {
        return Ok(None);
    }
    let metadata = fs_metadata(path)?;
    let key = Key {
        path: path.canonicalize()?,
        length: metadata.len(),
        modified: metadata.modified()?,
    };
    let mut cache = CONVERTED
        .lock()
        .map_err(|_| anyhow::anyhow!("DNG conversion cache poisoned"))?;
    if let Some((cached, file)) = &*cache
        && cached == &key
    {
        return Ok(Some(file.clone()));
    }
    let Ok(mut r) = Reader::open(path) else {
        return Ok(None);
    };
    let dirs = directories(&mut r, Kind::Raw)?;
    let Some(d) = dirs.iter().find(|d| d.subfile == 0) else {
        return Ok(None);
    };
    let maps = polynomials(&mut r, d)?;
    let luts = sample_luts(d, &maps);
    ensure!(!cancelled(), "DNG decode cancelled");
    let directory = directory
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("FASTCULL_DNG_CACHE_DIR").map(PathBuf::from))
        .unwrap_or_else(|| crate::platform::data_directory().join("dng-decode"));
    std::fs::create_dir_all(&directory)?;
    let mut output = tempfile::Builder::new()
        .prefix("fastcull-jxl-")
        .suffix(".dng")
        .tempfile_in(directory)?;
    r.file.seek(SeekFrom::Start(0))?;
    std::io::copy(&mut r.file, output.as_file_mut())?;
    // Append a tightly packed raster, then replace only the RAW IFD's image
    // layout tags in this copy. Every original metadata offset stays valid.
    let start = r.size + (r.size & 1);
    let bytes_per_pixel = u64::from(d.samples * (d.bits / 8));
    let length = u64::from(d.width) * u64::from(d.height) * bytes_per_pixel;
    ensure!(
        start + length <= u32::MAX as u64,
        "converted DNG exceeds classic TIFF range"
    );
    output.as_file_mut().set_len(start + length)?;
    let (tw, th) = d.tile.unwrap_or((d.width, d.height));
    ensure!(
        tw > 0
            && th > 0
            && tw <= 16384
            && th <= 16384
            && u64::from(tw) * u64::from(th) <= 16_000_000,
        "JPEG XL tile too large"
    );
    let columns = d.width.div_ceil(tw);
    let rows = d.height.div_ceil(th);
    ensure!(
        d.offsets.len() == (columns * rows) as usize,
        "incomplete JPEG XL DNG tile grid"
    );
    for (index, (&off, &len)) in d.offsets.iter().zip(&d.lengths).enumerate() {
        ensure!(!cancelled(), "DNG decode cancelled");
        let x = index as u32 % columns * tw;
        let y = index as u32 / columns * th;
        let image = read_image(&mut r, off, len, (tw, th))?;
        ensure!(
            matches!(image.image_header().metadata.bit_depth,
            jxl_oxide::image::BitDepth::IntegerSample { bits_per_sample } if bits_per_sample == d.bits),
            "JPEG XL tile bit depth differs from DNG samples"
        );
        // Preserve the tile's declared encoding. Applying sRGB here would
        // corrupt camera-space RAW samples before LibRaw's color transform.
        let rendered = image.render_frame(0).map_err(anyhow::Error::from_boxed)?;
        let mut stream = rendered.stream_no_alpha();
        ensure!(
            stream.channels() == d.samples,
            "JPEG XL tile channel mismatch"
        );
        let mut pixels = vec![0u16; tw as usize * th as usize * d.samples as usize];
        ensure!(
            stream.write_to_buffer(&mut pixels) == pixels.len(),
            "incomplete JPEG XL tile"
        );
        let cols = (d.width - x).min(tw) as usize * d.samples as usize;
        let mut row = Vec::with_capacity(cols * (d.bits / 8) as usize);
        for dy in 0..(d.height - y).min(th) {
            row.clear();
            let from = dy as usize * tw as usize * d.samples as usize;
            for (i, &sample) in pixels[from..from + cols].iter().enumerate() {
                let channel = i as u32 % d.samples;
                let sample = luts.as_ref().map_or_else(
                    || mapped_sample(sample, x + i as u32 / d.samples, y + dy, channel, &maps),
                    |tables| tables[channel as usize][sample as usize],
                );
                if d.bits == 8 {
                    row.push(((u32::from(sample) + 128) / 257) as u8);
                } else {
                    row.extend_from_slice(&encode_short(sample, r.little));
                }
            }
            output.seek(SeekFrom::Start(
                start + (u64::from(y + dy) * u64::from(d.width) + u64::from(x)) * bytes_per_pixel,
            ))?;
            output.write_all(&row)?;
        }
    }
    let mut entries = Vec::new();
    for e in &d.entries {
        // Replace tiles/strips with one contiguous strip, leaving all color,
        // crop, linearization and opcode metadata for LibRaw to interpret.
        if matches!(
            e.tag,
            273 | 278 | 279 | 322 | 323 | 324 | 325 | 51008 | 51009 | 51022
        ) {
            continue;
        }
        let value = if e.tag == 259 {
            encode_short(1, r.little)
                .into_iter()
                .chain([0, 0])
                .collect::<Vec<_>>()
        } else {
            e.value.to_vec()
        };
        entries.push(Entry {
            tag: e.tag,
            kind: e.kind,
            count: e.count,
            value: value.try_into().unwrap(),
        });
    }
    for (tag, value) in [(273, start as u32), (278, d.height), (279, length as u32)] {
        entries.push(Entry {
            tag,
            kind: 4,
            count: 1,
            value: encode_long(value, r.little),
        });
    }
    entries.sort_by_key(|e| e.tag);
    // A tiled directory has more entries than the strip layout. Reject any
    // layout that would grow into the next original metadata block.
    ensure!(
        entries.len() <= d.entries.len(),
        "converted RAW directory would grow"
    );
    let (_, next) = r.ifd(d.offset)?;
    output.seek(SeekFrom::Start(u64::from(d.offset)))?;
    output.write_all(&encode_short(entries.len() as u16, r.little))?;
    for e in entries {
        output.write_all(&encode_short(e.tag, r.little))?;
        output.write_all(&encode_short(e.kind, r.little))?;
        output.write_all(&encode_long(e.count, r.little))?;
        output.write_all(&e.value)?;
    }
    output.write_all(&encode_long(next, r.little))?;
    output.flush()?;
    let after = fs_metadata(path)?;
    ensure!(
        after.len() == key.length && after.modified()? == key.modified,
        "DNG changed while decoding"
    );
    ensure!(!cancelled(), "DNG decode cancelled");
    // LibRaw's Windows datastream opens with FILE_SHARE_READ. Close our
    // writable handle before handing it the path, while retaining deletion
    // ownership. Otherwise CreateFile reports an I/O/sharing error.
    let (file, temporary_path) = output.into_parts();
    drop(file);
    let output = Arc::new(temporary_path);
    *cache = Some((key, output.clone()));
    Ok(Some(output))
}
fn fs_metadata(path: &Path) -> Result<std::fs::Metadata> {
    Ok(std::fs::metadata(path)?)
}

pub(super) fn clear_cache() {
    if let Ok(mut cache) = CONVERTED.lock() {
        *cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Minimal codestream from jxl-oxide 0.12.6's public decoding example
    // (MIT OR Apache-2.0); contains no user photograph.
    const IMAGE: &[u8] = &[
        0xff, 0x0a, 0x30, 0x54, 0x10, 0x09, 0x08, 0x06, 0x01, 0x00, 0x78, 0x00, 0x4b, 0x38, 0x41,
        0x3c, 0xb6, 0x3a, 0x51, 0xfe, 0x00, 0x47, 0x1e, 0xa0, 0x85, 0xb8, 0x27, 0x1a, 0x48, 0x45,
        0x84, 0x1b, 0x71, 0x4f, 0xa8, 0x3e, 0x8e, 0x30, 0x03, 0x92, 0x84, 0x01,
    ];
    fn fixture(little: bool) -> Vec<u8> {
        let image = JxlImage::builder().read(IMAGE).unwrap();
        let (w, h) = (image.width(), image.height());
        let mut b = vec![0; 4096];
        b[..2].copy_from_slice(if little { b"II" } else { b"MM" });
        b[2..4].copy_from_slice(&encode_short(42, little));
        b[4..8].copy_from_slice(&encode_long(8, little));
        super::super::tests::ifd(
            &mut b,
            8,
            &[
                (254, 4, 1, 1),
                (256, 4, 1, w),
                (257, 4, 1, h),
                (258, 3, 3, 512),
                (259, 3, 1, JXL),
                (262, 3, 1, 2),
                (273, 4, 1, 3000),
                (277, 3, 1, 3),
                (279, 4, 1, IMAGE.len() as u32),
                (330, 4, 1, 1024),
            ],
            little,
        );
        for i in 0..3 {
            b[512 + i * 2..514 + i * 2].copy_from_slice(&encode_short(8, little));
        }
        super::super::tests::ifd(
            &mut b,
            1024,
            &[
                (254, 4, 1, 0),
                (256, 4, 1, w * 2 - 3),
                (257, 4, 1, h - 2),
                (258, 3, 3, 512),
                (259, 3, 1, JXL),
                (262, 3, 1, 34892),
                (277, 3, 1, 3),
                (322, 4, 1, w),
                (323, 4, 1, h),
                (324, 4, 2, 600),
                (325, 4, 2, 608),
            ],
            little,
        );
        for (i, v) in [3000, 3000, IMAGE.len() as u32, IMAGE.len() as u32]
            .into_iter()
            .enumerate()
        {
            b[600 + i * 4..604 + i * 4].copy_from_slice(&encode_long(v, little));
        }
        b[3000..3000 + IMAGE.len()].copy_from_slice(IMAGE);
        b
    }
    #[test]
    fn jpeg_xl_only_preview_reaches_real_culler_path_in_both_byte_orders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.dng");
        for little in [true, false] {
            let bytes = fixture(little);
            std::fs::write(&path, &bytes).unwrap();
            let info = ArwInfo::read(&path).unwrap();
            assert!(info.jxl_preview);
            assert_eq!(info.jpeg.len, 0);
            for level in [Level::Thumb, Level::Fit(50)] {
                let pixels = imaging::decode_level(&path, &info, level).unwrap();
                assert!(pixels.width > 0 && pixels.height > 0);
                assert!(pixels.rgba.chunks_exact(4).all(|p| p[3] == 255));
            }
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn jpeg_xl_raw_tiles_preserve_samples_edges_and_metadata_without_source_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiles.dng");
        for little in [true, false] {
            let bytes = fixture(little);
            std::fs::write(&path, &bytes).unwrap();
            let source = native_source_in(&path, Some(dir.path()), &|| false)
                .unwrap()
                .unwrap();
            let again = native_source_in(&path, Some(dir.path()), &|| false)
                .unwrap()
                .unwrap();
            assert!(Arc::ptr_eq(&source, &again));
            let mut r = Reader::open(source.as_ref()).unwrap();
            let (entries, _) = r.ifd(1024).unwrap();
            assert_eq!(scalar(&r, &entries, 259), Some(1));
            let off = scalar(&r, &entries, 273).unwrap() as u64;
            let len = scalar(&r, &entries, 279).unwrap() as usize;
            r.file.seek(SeekFrom::Start(off)).unwrap();
            let mut raster = vec![0; len];
            r.file.read_exact(&mut raster).unwrap();
            let image = JxlImage::builder().read(IMAGE).unwrap();
            let rendered = image.render_frame(0).unwrap();
            let (w, h) = (image.width() as usize, image.height() as usize);
            let mut tile = vec![0u8; w * h * 3];
            assert_eq!(
                rendered.stream_no_alpha().write_to_buffer(&mut tile),
                tile.len()
            );
            let mut expected = Vec::new();
            for row in tile.chunks_exact(w * 3).take(h - 2) {
                expected.extend_from_slice(row);
                expected.extend_from_slice(&row[..(w - 3) * 3]);
            }
            assert_eq!(raster, expected);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            drop(again);
            drop(source);
            clear_cache();
        }
    }
    #[test]
    fn jpeg_xl_bad_raw_ranges_do_not_hide_preview_or_leave_converted_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.dng");
        let mut bytes = fixture(true);
        bytes[600..604].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let info = ArwInfo::read(&path).unwrap();
        assert!(imaging::decode_level(&path, &info, Level::Fit(50)).is_ok());
        assert!(native_source_in(&path, Some(dir.path()), &|| false).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::write(&path, fixture(true)).unwrap();
        assert!(native_source_in(&path, Some(dir.path()), &|| true).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn dng_stage2_polynomial_uses_channel_area_pitch_and_order() {
        let area = [2, 3, 8, 9, 1, 1, 2, 2];
        let maps = vec![Polynomial {
            area,
            coefficients: vec![0., 0., 1.],
        }];
        assert_eq!(mapped_sample(32768, 3, 2, 1, &maps), 16384);
        for (x, y, c) in [(4, 2, 1), (3, 3, 1), (3, 2, 0), (3, 2, 2), (1, 2, 1)] {
            assert_eq!(mapped_sample(32768, x, y, c, &maps), 32768);
        }
        let twice = [
            Polynomial {
                area,
                coefficients: vec![0., 0.5],
            },
            Polynomial {
                area,
                coefficients: vec![0., 2.],
            },
        ];
        assert_eq!(mapped_sample(32768, 3, 2, 1, &twice), 32768);
    }
}
