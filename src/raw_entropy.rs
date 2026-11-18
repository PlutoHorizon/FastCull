//! Strict tiled SOF3 / predictor-1 decoding. Unsupported encodings stay in LibRaw.
use anyhow::{Context, Result, ensure};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sony = 1,
    LinearDng = 2,
}
pub struct Tile {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub bits: u32,
    pub data: Vec<u8>,
    pub tables: Vec<Arc<[u32]>>,
}
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub kind: Kind,
    pub tiles: Vec<Tile>,
}
struct Tiff<'a> {
    data: &'a [u8],
    little: bool,
}
impl Tiff<'_> {
    fn bytes(&self, p: usize, n: usize) -> Result<&[u8]> {
        self.data
            .get(p..p.checked_add(n).context("TIFF overflow")?)
            .context("truncated TIFF")
    }
    fn u16(&self, p: usize) -> Result<u16> {
        let a = self.bytes(p, 2)?.try_into()?;
        Ok(if self.little {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        })
    }
    fn u32(&self, p: usize) -> Result<u32> {
        let a = self.bytes(p, 4)?.try_into()?;
        Ok(if self.little {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }
    fn fields(&self, p: usize) -> Result<(HashMap<u16, Vec<u32>>, usize)> {
        let n = self.u16(p)? as usize;
        ensure!(n <= 4096, "too many TIFF entries");
        let mut tags = HashMap::new();
        self.bytes(p + 2, n * 12 + 4)?;
        for i in 0..n {
            let at = p + 2 + i * 12;
            let tag = self.u16(at)?;
            if !matches!(
                tag,
                256 | 257 | 258 | 259 | 277 | 322 | 323 | 324 | 325 | 330
            ) {
                continue;
            }
            let typ = self.u16(at + 2)?;
            let count = self.u32(at + 4)? as usize;
            let size = match typ {
                3 => 2,
                4 => 4,
                _ => continue,
            };
            ensure!(count <= 100_000, "TIFF array too large");
            let offset = if count * size <= 4 {
                at + 8
            } else {
                self.u32(at + 8)? as usize
            };
            self.bytes(offset, count * size)?;
            let values = (0..count)
                .map(|j| {
                    if size == 2 {
                        self.u16(offset + j * size).map(u32::from)
                    } else {
                        self.u32(offset + j * size)
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            tags.insert(tag, values);
        }
        Ok((tags, self.u32(p + 2 + n * 12)? as usize))
    }
}
fn one(tags: &HashMap<u16, Vec<u32>>, tag: u16) -> Option<usize> {
    tags.get(&tag)
        .filter(|v| v.len() == 1)
        .map(|v| v[0] as usize)
}
type Tables = HashMap<Vec<u8>, Arc<[u32]>>;
fn huffman(bytes: &[u8], cache: &mut Tables) -> Result<Arc<[u32]>> {
    if let Some(v) = cache.get(bytes) {
        return Ok(v.clone());
    }
    ensure!(cache.len() < 4096, "too many distinct Huffman tables");
    ensure!(
        bytes.len() >= 17 && bytes[0] >> 4 == 0,
        "not a DC Huffman table"
    );
    let counts = &bytes[1..17];
    let n: usize = counts.iter().map(|v| *v as usize).sum();
    ensure!(
        n > 0 && bytes.len() == 17 + n && n <= 17,
        "invalid lossless Huffman table"
    );
    let mut result = vec![0u32; 4096];
    let mut code = 0usize;
    let mut symbol = 17;
    for len in 1..=16 {
        let count = counts[len - 1] as usize;
        ensure!(code + count <= 1usize << len, "oversubscribed Huffman tree");
        for _ in 0..count {
            let category = bytes[symbol];
            symbol += 1;
            ensure!(category <= 16, "invalid lossless category");
            if len <= 12 {
                let start = code << (12 - len);
                let end = (code + 1) << (12 - len);
                result[start..end].fill(((len as u32) << 8) | u32::from(category));
            } else {
                let prefix = code >> (len - 12);
                if result[prefix] == 0 {
                    let offset = result.len();
                    result.resize(offset + 16, 0);
                    result[prefix] = 0x8000_0000 | offset as u32;
                }
                ensure!(
                    result[prefix] & 0x8000_0000 != 0,
                    "overlapping Huffman codes"
                );
                let offset = (result[prefix] & 0x7fff_ffff) as usize;
                let low = code & ((1usize << (len - 12)) - 1);
                let start = offset + (low << (16 - len));
                let end = start + (1usize << (16 - len));
                result[start..end].fill(((len as u32) << 8) | u32::from(category));
            }
            code += 1;
        }
        code <<= 1;
    }
    let result: Arc<[u32]> = result.into();
    cache.insert(bytes.to_vec(), result.clone());
    Ok(result)
}
fn jpeg(bytes: &[u8], cache: &mut Tables) -> Result<Tile> {
    ensure!(bytes.starts_with(&[255, 216]), "not JPEG");
    let mut at = 2usize;
    let mut frame = None;
    let mut tables = HashMap::new();
    loop {
        ensure!(bytes.get(at) == Some(&255), "invalid JPEG marker");
        let marker = *bytes.get(at + 1).context("truncated JPEG marker")?;
        let length = u16::from_be_bytes(
            bytes
                .get(at + 2..at + 4)
                .context("truncated JPEG length")?
                .try_into()?,
        ) as usize;
        ensure!(length >= 2, "invalid JPEG length");
        let end = at.checked_add(2 + length).context("JPEG overflow")?;
        let data = bytes.get(at + 4..end).context("truncated JPEG header")?;
        at = end;
        match marker {
            0xc3 => {
                ensure!(data.len() >= 6, "short SOF3");
                let bits = u32::from(data[0]);
                let h = u16::from_be_bytes(data[1..3].try_into()?) as usize;
                let w = u16::from_be_bytes(data[3..5].try_into()?) as usize;
                let channels = data[5] as usize;
                ensure!(
                    (8..=16).contains(&bits)
                        && w > 0
                        && h > 0
                        && channels > 0
                        && channels <= 4
                        && data.len() == 6 + 3 * channels
                        && w * h * channels <= 2_000_000,
                    "unsupported SOF3 dimensions"
                );
                let mut ids = Vec::new();
                ensure!(
                    bytes.len() <= w * h * channels * 8 + 65536,
                    "encoded tile exceeds sample budget"
                );
                for c in 0..channels {
                    ensure!(
                        data[7 + 3 * c] == 0x11 && data[8 + 3 * c] == 0,
                        "unsupported JPEG sampling"
                    );
                    ids.push(data[6 + 3 * c]);
                }
                frame = Some((w, h, channels, bits, ids));
            }
            0xc4 => {
                let mut i = 0;
                while i < data.len() {
                    ensure!(i + 17 <= data.len(), "truncated DHT");
                    let n: usize = data[i + 1..i + 17].iter().map(|v| *v as usize).sum();
                    let stop = i + 17 + n;
                    let dht = data.get(i..stop).context("truncated DHT symbols")?;
                    ensure!(data[i] <= 3, "unsupported JPEG Huffman selector");
                    tables.insert(data[i], huffman(dht, cache)?);
                    i = stop;
                }
            }
            0xdd => ensure!(data == [0, 0], "JPEG restart intervals use CPU fallback"),
            0xda => {
                let (w, h, channels, bits, ids) = frame.context("SOS without SOF3")?;
                ensure!(
                    data.len() == 1 + 2 * channels + 3
                        && data[0] as usize == channels
                        && data[1 + 2 * channels..] == [1, 0, 0],
                    "only complete predictor-1 scans supported"
                );
                let mut selected = Vec::new();
                for c in 0..channels {
                    ensure!(data[1 + 2 * c] == ids[c], "reordered JPEG channels");
                    let id = data[2 + 2 * c];
                    ensure!(id & 15 == 0, "lossless AC selector");
                    selected.push(
                        tables
                            .get(&(id >> 4))
                            .context("missing DC Huffman table")?
                            .clone(),
                    );
                }
                let mut entropy = Vec::with_capacity(bytes.len() - at);
                let mut ended = false;
                while at < bytes.len() {
                    let v = bytes[at];
                    at += 1;
                    if v != 255 {
                        entropy.push(v);
                        continue;
                    }
                    let next = *bytes.get(at).context("truncated JPEG escape")?;
                    at += 1;
                    match next {
                        0 => entropy.push(255),
                        0xd9 => {
                            ended = true;
                            break;
                        }
                        _ => anyhow::bail!("unsupported entropy marker"),
                    }
                }
                ensure!(ended && !entropy.is_empty(), "missing JPEG end marker");
                return Ok(Tile {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h,
                    channels,
                    bits,
                    data: entropy,
                    tables: selected,
                });
            }
            0xe0..=0xef | 0xfe => {}
            _ => anyhow::bail!("unsupported JPEG process {marker:x}"),
        }
    }
}
/// Summary of one TIFF directory, used for dispatch diagnostics and probe output.
#[derive(Clone, Debug)]
pub struct Directory {
    pub offset: usize,
    pub width: usize,
    pub height: usize,
    pub compression: usize,
    pub samples: usize,
    pub bits: Vec<usize>,
    pub tile_width: usize,
    pub tile_height: usize,
    pub tile_count: usize,
    /// Tiled lossless-JPEG stream of the channel count this kind decodes.
    pub stream_supported: bool,
    /// Requested dimensions equal this directory's dimensions.
    pub dimension_match: bool,
}
impl Directory {
    pub fn codec(&self) -> String {
        match self.compression {
            1 => "raw(1)".into(),
            6 => "old-jpeg(6)".into(),
            7 => "lossless-jpeg(7)".into(),
            8 | 32946 => "deflate(8)".into(),
            other => format!("compression({other})"),
        }
    }
    pub fn kind_label(&self) -> &'static str {
        match self.samples {
            1 => "cfa",
            3 => "linear-rgb",
            4 => "rgba",
            _ => "multi-sample",
        }
    }
    pub fn summary(&self) -> String {
        let tiles = if self.tile_count > 0 {
            format!(
                "tiles={}x{}({})",
                self.tile_width, self.tile_height, self.tile_count
            )
        } else {
            "untiled".to_string()
        };
        let bits = self
            .bits
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join("/");
        format!(
            "{}x{} type={} codec={} samples={} bits={} {}",
            self.width,
            self.height,
            self.kind_label(),
            self.codec(),
            self.samples,
            if bits.is_empty() { "-".into() } else { bits },
            tiles
        )
    }
}
fn directory_from_tags(
    offset: usize,
    tags: &HashMap<u16, Vec<u32>>,
    kind: Kind,
    width: usize,
    height: usize,
) -> Directory {
    let samples = one(tags, 277).unwrap_or(1);
    let compression = one(tags, 259).unwrap_or(0);
    let tile_width = one(tags, 322).unwrap_or(0);
    let tile_height = one(tags, 323).unwrap_or(0);
    let tile_count = tags.get(&324).map_or(0, Vec::len);
    let bits = tags
        .get(&258)
        .map(|values| values.iter().map(|v| *v as usize).collect())
        .unwrap_or_default();
    let directory_width = one(tags, 256).unwrap_or(0);
    let directory_height = one(tags, 257).unwrap_or(0);
    Directory {
        offset,
        width: directory_width,
        height: directory_height,
        compression,
        samples,
        bits,
        tile_width,
        tile_height,
        tile_count,
        stream_supported: compression == 7
            && tile_count > 0
            && samples == if kind == Kind::Sony { 1 } else { 3 },
        dimension_match: directory_width == width && directory_height == height,
    }
}
/// Every TIFF directory of an encoded RAW, with stream/dimension match flags.
pub fn directories(bytes: &[u8], kind: Kind) -> Result<Vec<Directory>> {
    ensure!(
        bytes.len() >= 8 && bytes.len() <= 2 * 1024 * 1024 * 1024,
        "invalid TIFF size"
    );
    let little = match &bytes[..2] {
        b"II" => true,
        b"MM" => false,
        _ => anyhow::bail!("not TIFF"),
    };
    let t = Tiff {
        data: bytes,
        little,
    };
    ensure!(t.u16(2)? == 42, "BigTIFF uses CPU fallback");
    let mut todo = vec![t.u32(4)? as usize];
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    while let Some(at) = todo.pop() {
        if at == 0 || !seen.insert(at) {
            continue;
        }
        ensure!(seen.len() <= 64, "too many TIFF directories");
        let (tags, next) = t.fields(at)?;
        todo.push(next);
        if let Some(subs) = tags.get(&330) {
            todo.extend(subs.iter().map(|v| *v as usize));
        }
        found.push(directory_from_tags(at, &tags, kind, 0, 0));
    }
    Ok(found)
}
/// Storage (encoded) dimensions of the first supported tiled stream.
pub fn storage_size(bytes: &[u8], kind: Kind) -> Result<(usize, usize)> {
    let found = directories(bytes, kind)?;
    found
        .iter()
        .find(|directory| directory.stream_supported && directory.tile_count > 0)
        .map(|directory| (directory.width, directory.height))
        .context("no supported tiled RAW stream")
}
pub fn parse(bytes: &[u8], kind: Kind, width: usize, height: usize) -> Result<Image> {
    ensure!(
        bytes.len() >= 8 && bytes.len() <= 2 * 1024 * 1024 * 1024,
        "invalid TIFF size"
    );
    let little = match &bytes[..2] {
        b"II" => true,
        b"MM" => false,
        _ => anyhow::bail!("not TIFF"),
    };
    let t = Tiff {
        data: bytes,
        little,
    };
    ensure!(t.u16(2)? == 42, "BigTIFF uses CPU fallback");
    ensure!(
        width > 0 && height > 0 && width.checked_mul(height).is_some_and(|n| n <= 160_000_000),
        "invalid RAW dimensions"
    );
    let mut todo = vec![t.u32(4)? as usize];
    let mut seen = HashSet::new();
    let mut candidate = None;
    let mut found: Vec<Directory> = Vec::new();
    let want_samples = if kind == Kind::Sony { 1 } else { 3 };
    while let Some(at) = todo.pop() {
        if at == 0 || !seen.insert(at) {
            continue;
        }
        ensure!(seen.len() <= 64, "too many TIFF directories");
        let (tags, next) = t.fields(at)?;
        todo.push(next);
        if let Some(subs) = tags.get(&330) {
            todo.extend(subs.iter().map(|v| *v as usize));
        }
        let directory = directory_from_tags(at, &tags, kind, width, height);
        let supported = directory.stream_supported && directory.dimension_match;
        found.push(directory);
        if supported {
            candidate = Some(tags);
            break;
        }
    }
    let tags = match candidate {
        Some(tags) => tags,
        None => {
            let storage = found
                .iter()
                .find(|directory| directory.stream_supported)
                .map(|directory| format!("{}x{}", directory.width, directory.height))
                .unwrap_or_else(|| "unknown".into());
            let listed = found
                .iter()
                .map(|directory| directory.summary())
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!(
                "no supported tiled RAW directory: kind={kind:?} requested={width}x{height} \
                 storage={storage} supported_samples={want_samples} directories=[{listed}] \
                 (pass the storage dimensions of the supported stream)"
            )
        }
    };
    let tw = one(&tags, 322).context("missing tile width")?;
    let th = one(&tags, 323).context("missing tile height")?;
    ensure!(
        tw > 0 && th > 0 && tw <= 4096 && th <= 4096,
        "invalid tile size"
    );
    let nx = width.div_ceil(tw);
    let ny = height.div_ceil(th);
    let offsets = tags.get(&324).context("missing tile offsets")?;
    let sizes = tags.get(&325).context("missing tile counts")?;
    ensure!(
        offsets.len() == nx * ny && offsets.len() == sizes.len() && offsets.len() <= 16384,
        "invalid tile grid"
    );
    let mut cache = Tables::new();
    let mut tiles = Vec::with_capacity(offsets.len());
    let mut entropy_bytes = 0usize;
    for (i, (&offset, &len)) in offsets.iter().zip(sizes).enumerate() {
        let mut tile = jpeg(t.bytes(offset as usize, len as usize)?, &mut cache)?;
        entropy_bytes += tile.data.len();
        ensure!(
            entropy_bytes <= 512 * 1024 * 1024,
            "entropy tile data exceeds memory budget"
        );
        tile.x = i % nx * tw;
        tile.y = i / nx * th;
        if kind == Kind::Sony {
            ensure!(
                tile.channels == 4 && tile.width * 2 == tw && tile.height * 2 == th,
                "unsupported Sony tile layout"
            );
        } else {
            ensure!(
                tile.channels == 3 && tile.width == tw && tile.height == th,
                "unsupported linear DNG tile layout"
            );
        }
        tiles.push(tile);
    }
    Ok(Image {
        width,
        height,
        channels: if kind == Kind::Sony { 1 } else { 3 },
        kind,
        tiles,
    })
}
fn bits(bytes: &[u8], position: usize, n: usize) -> u32 {
    if n == 0 {
        return 0;
    }
    let p = position / 8;
    let offset = position % 8;
    let word = (0..4).fold(0u32, |v, i| {
        (v << 8) | u32::from(bytes.get(p + i).copied().unwrap_or(0))
    });
    let mut window = word << offset;
    if offset != 0 {
        window |= u32::from(bytes.get(p + 4).copied().unwrap_or(0)) >> (8 - offset);
    }
    window >> (32 - n)
}
pub fn decode_tile(tile: &Tile, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<Vec<u16>> {
    let mut out = vec![0u16; tile.width * tile.height * tile.channels];
    let mut position = 0usize;
    let mut first = [1i32 << (tile.bits - 1); 4];
    let mut left = [0i32; 4];
    for y in 0..tile.height {
        ensure!(!cancelled(), "已取消 RAW 熵解码");
        for x in 0..tile.width {
            for c in 0..tile.channels {
                let table = &tile.tables[c];
                let word = bits(&tile.data, position, 32);
                let mut entry = table[(word >> 20) as usize];
                if entry & 0x8000_0000 != 0 {
                    entry = *table
                        .get((entry & 0x7fff_ffff) as usize + ((word >> 16) & 15) as usize)
                        .context("invalid secondary Huffman table")?;
                }
                let n = (entry >> 8) as usize;
                let cat = (entry & 255) as usize;
                ensure!(n > 0, "invalid Huffman code");
                position += n;
                let len = if cat == 16 { 0 } else { cat };
                ensure!(
                    position + len <= tile.data.len() * 8,
                    "truncated JPEG entropy"
                );
                let value = if len == 0 {
                    0
                } else {
                    ((word << n) >> (32 - len)) as i32
                };
                position += len;
                let diff = if cat == 16 {
                    -32768
                } else if cat == 0 {
                    0
                } else if value & (1 << (cat - 1)) == 0 {
                    value - ((1 << cat) - 1)
                } else {
                    value
                };
                let sum = if x == 0 {
                    first[c] += diff;
                    first[c]
                } else {
                    left[c] + diff
                };
                let value = sum as u16;
                ensure!(
                    u32::from(value) < 1u32 << tile.bits,
                    "JPEG predictor outside precision"
                );
                left[c] = i32::from(value);
                out[(y * tile.width + x) * tile.channels + c] = value;
            }
        }
    }
    Ok(out)
}
pub fn assemble(image: &Image, tile: &Tile, values: &[u32], out: &mut [u16]) -> Result<()> {
    ensure!(
        values.len() == tile.width * tile.height * tile.channels
            && out.len() == image.width * image.height * image.channels
            && tile.x < image.width
            && tile.y < image.height,
        "invalid entropy tile output"
    );
    if image.kind == Kind::LinearDng {
        let w = tile.width.min(image.width - tile.x) * 3;
        let h = tile.height.min(image.height - tile.y);
        for y in 0..h {
            let at = ((tile.y + y) * image.width + tile.x) * 3;
            let src = &values[y * tile.width * 3..y * tile.width * 3 + w];
            for (dst, src) in out[at..at + w].iter_mut().zip(src) {
                *dst = *src as u16;
            }
        }
        return Ok(());
    }
    for y in 0..tile.height {
        for x in 0..tile.width {
            if image.kind == Kind::Sony {
                for c in 0..4 {
                    let (gx, gy) = (tile.x + x * 2 + c % 2, tile.y + y * 2 + c / 2);
                    if gx < image.width && gy < image.height {
                        out[gy * image.width + gx] = values[(y * tile.width + x) * 4 + c] as u16;
                    }
                }
            } else {
                let (gx, gy) = (tile.x + x, tile.y + y);
                if gx < image.width && gy < image.height {
                    for c in 0..3 {
                        out[(gy * image.width + gx) * 3 + c] =
                            values[(y * tile.width + x) * 3 + c] as u16;
                    }
                }
            }
        }
    }
    Ok(())
}
pub fn decode_cpu(image: &Image, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<Vec<u16>> {
    validate(image)?;
    let mut tiles: Vec<Vec<u16>> = (0..image.tiles.len()).map(|_| Vec::new()).collect();
    crate::raw_parallel::bands(&mut tiles, 1, 1, |i, row| {
        row[0] = decode_tile(&image.tiles[i], cancelled)?;
        Ok(())
    })?;
    let mut output = vec![0u16; image.width * image.height * image.channels];
    for (tile, values) in image.tiles.iter().zip(tiles) {
        let values: Vec<_> = values.into_iter().map(u32::from).collect();
        assemble(image, tile, &values, &mut output)?;
    }
    Ok(output)
}
pub fn read(path: &Path, kind: Kind, width: usize, height: usize) -> Result<Image> {
    ensure!(
        std::fs::metadata(path)?.len() <= 512 * 1024 * 1024,
        "large encoded RAW uses LibRaw fallback"
    );
    parse(&std::fs::read(path)?, kind, width, height)
}
pub fn validate(image: &Image) -> Result<()> {
    ensure!(
        image.width > 0
            && image.height > 0
            && image
                .width
                .checked_mul(image.height)
                .and_then(|n| n.checked_mul(image.channels))
                .is_some_and(|n| n <= 160_000_000)
            && image.channels == if image.kind == Kind::Sony { 1 } else { 3 },
        "invalid entropy image"
    );
    ensure!(
        !image.tiles.is_empty() && image.tiles.len() <= 16384,
        "invalid entropy tile count"
    );
    for t in &image.tiles {
        ensure!(
            t.width > 0
                && t.height > 0
                && t.width
                    .checked_mul(t.height)
                    .and_then(|n| n.checked_mul(t.channels))
                    .is_some_and(|n| n <= 2_000_000)
                && t.channels == if image.kind == Kind::Sony { 4 } else { 3 },
            "invalid entropy tile dimensions"
        );
        ensure!(
            (8..=16).contains(&t.bits)
                && !t.data.is_empty()
                && t.data.len() <= 16 * 1024 * 1024
                && t.tables.len() == t.channels
                && t.tables.iter().all(|v| (4096..=4368).contains(&v.len())),
            "invalid entropy tile data"
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn small_table() -> Arc<[u32]> {
        let mut d = vec![0u8; 17];
        d[2] = 3;
        d.extend([0, 1, 2]);
        huffman(&d, &mut Tables::new()).unwrap()
    }
    #[test]
    fn predictor_one_matches_known_interleaved_samples_and_cancel() {
        let deltas = [0, 1, -1, 2, 1, 0, -1, -2, 2, -1, 1, -2, 0, 0, 0, 0];
        let mut stream = String::new();
        for d in deltas {
            stream.push_str(match d {
                0 => "00",
                1 => "011",
                -1 => "010",
                2 => "1010",
                -2 => "1001",
                _ => unreachable!(),
            });
        }
        while !stream.len().is_multiple_of(8) {
            stream.push('1');
        }
        let bytes = stream
            .as_bytes()
            .chunks(8)
            .map(|v| v.iter().fold(0u8, |a, b| (a << 1) | u8::from(*b == b'1')))
            .collect();
        let mut tile = Tile {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            channels: 4,
            bits: 8,
            data: bytes,
            tables: vec![small_table(); 4],
        };
        assert_eq!(
            decode_tile(&tile, &|| false).unwrap(),
            [
                128, 129, 127, 130, 129, 129, 126, 128, 130, 128, 128, 128, 130, 128, 128, 128
            ]
        );
        assert!(decode_tile(&tile, &|| true).is_err());
        tile.data.truncate(1);
        assert!(decode_tile(&tile, &|| false).is_err());
    }
    #[test]
    fn secondary_huffman_table_preserves_sixteen_bit_codes() {
        let mut d = vec![0u8; 17];
        d[16] = 1;
        d.push(0);
        let table = huffman(&d, &mut Tables::new()).unwrap();
        assert_eq!(table.len(), 4112);
        let tile = Tile {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            channels: 1,
            bits: 8,
            data: vec![0, 0],
            tables: vec![table],
        };
        assert_eq!(decode_tile(&tile, &|| false).unwrap(), [128]);
        let mut invalid = vec![0u8; 17];
        invalid[1] = 3;
        invalid.extend([0, 1, 2]);
        assert!(huffman(&invalid, &mut Tables::new()).is_err());
    }
    #[test]
    fn malformed_tiff_does_not_escape_bounds_or_loop() {
        assert!(parse(b"II*\0\xff\xff\xff\xff", Kind::Sony, 16, 16).is_err());
        assert!(parse(b"II+\0\x08\0\0\0", Kind::Sony, 16, 16).is_err());
        assert!(parse(b"not a TIFF file", Kind::LinearDng, 16, 16).is_err());
    }
}
