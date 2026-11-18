//! Same-stage integer oracle. Canonical samples exclude row padding and the
//! unused fourth channel of LibRaw's legacy linear-RGB storage.
use anyhow::{Result, ensure};
use std::path::Path;

pub const STAGE: &str = "libraw-unpack-u16-v1";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    LibRaw,
    Parallel,
    Gpu,
}
impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Self::LibRaw => "libraw",
            Self::Parallel => "parallel",
            Self::Gpu => "gpu",
        }
    }
}
#[repr(C)]
pub(crate) struct NativeCheckpoint {
    pub abi: u32,
    pub stage: u32,
    pub width: u32,
    pub height: u32,
    pub active_width: u32,
    pub active_height: u32,
    pub left: u32,
    pub top: u32,
    pub channels: u32,
    pub storage_channels: u32,
    pub row_stride: u32,
    pub filters: u32,
    pub orientation: u32,
    pub bits: u32,
    pub dng_version: u32,
    pub decoder_flags: u32,
    pub black: u32,
    pub white: u32,
    pub camera_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    pub file_bytes: u64,
    pub file_stamp: u64,
    pub storage_len: u64,
    pub cblack: *const u32,
    pub cblack_len: u32,
    pub reserved: u32,
    pub curve: *const u16,
    pub curve_len: u32,
    pub flags: u32,
    pub decoder: [u8; 32],
}
#[derive(Debug, PartialEq)]
pub struct Metadata {
    pub size: [u32; 2],
    pub active: [u32; 4],
    pub channels: u32,
    pub storage_channels: u32,
    pub row_stride: u32,
    pub filters: u32,
    pub orientation: u32,
    pub bits: u32,
    pub dng_version: u32,
    pub decoder_flags: u32,
    pub black: u32,
    pub white: u32,
    pub camera_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    pub source_stamp: [u64; 2],
    pub flags: u32,
    pub decoder: String,
    pub cblack: Vec<u32>,
    pub curve: Vec<u16>,
}
pub struct Checkpoint {
    pub metadata: Metadata,
    pub samples: Vec<u16>,
    pub adopted: bool,
}
impl Checkpoint {
    /// Check metadata too: matching pixels from different domains is not proof.
    pub fn compare(&self, other: &Self) -> Result<()> {
        ensure!(
            self.metadata == other.metadata,
            "sensor checkpoint metadata/domain differ"
        );
        ensure!(
            self.samples.len() == other.samples.len(),
            "sensor sample count differs"
        );
        if let Some((index, (a, b))) = self
            .samples
            .iter()
            .zip(&other.samples)
            .enumerate()
            .find(|(_, (a, b))| a != b)
        {
            anyhow::bail!("sensor sample mismatch at {index}: {a} != {b}");
        }
        Ok(())
    }
}
#[cfg(any(windows, target_os = "macos"))]
pub fn capture(
    path: &Path,
    backend: Backend,
    gpu: Option<&crate::raw_compute::Context>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Checkpoint> {
    crate::native_raw::sensor_checkpoint(path, backend, gpu, cancelled)
}
/// The native visitor owns the source during this copy; all output is Rust-owned.
pub(crate) unsafe fn copy_native(
    info: &NativeCheckpoint,
    data: *const u16,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Checkpoint> {
    ensure!(
        info.abi == 1 && info.stage == 1 && !data.is_null(),
        "invalid sensor checkpoint ABI"
    );
    let (w, h, c, storage, stride) = (
        info.width as usize,
        info.height as usize,
        info.channels as usize,
        info.storage_channels as usize,
        info.row_stride as usize,
    );
    let count = w.checked_mul(h).and_then(|n| n.checked_mul(c));
    ensure!(
        w > 0
            && h > 0
            && matches!((c, storage), (1, 1) | (3, 3) | (3, 4))
            && count.is_some_and(|n| n <= 160_000_000)
            && stride >= w * storage,
        "invalid sensor checkpoint shape"
    );
    let span = (h - 1)
        .checked_mul(stride)
        .and_then(|n| n.checked_add(w * storage));
    ensure!(
        span.is_some_and(|n| n <= 640_000_000 && n as u64 == info.storage_len),
        "invalid sensor checkpoint span"
    );
    ensure!(
        !info.cblack.is_null()
            && info.cblack_len == 4104
            && !info.curve.is_null()
            && info.curve_len == 65536,
        "invalid sensor checkpoint metadata"
    );
    ensure!(
        info.active_width > 0
            && info.active_height > 0
            && info
                .left
                .checked_add(info.active_width)
                .is_some_and(|v| v <= info.width)
            && info
                .top
                .checked_add(info.active_height)
                .is_some_and(|v| v <= info.height),
        "invalid sensor checkpoint active region"
    );
    ensure!(!cancelled(), "cancelled sensor checkpoint copy");
    let source = unsafe { std::slice::from_raw_parts(data, span.unwrap()) };
    let mut samples = Vec::with_capacity(count.unwrap());
    for y in 0..h {
        ensure!(!cancelled(), "cancelled sensor checkpoint copy");
        let row = &source[y * stride..y * stride + w * storage];
        for pixel in row.chunks_exact(storage) {
            samples.extend_from_slice(&pixel[..c]);
        }
    }
    let end = info.decoder.iter().position(|b| *b == 0).unwrap_or(32);
    let metadata = Metadata {
        size: [info.width, info.height],
        active: [info.left, info.top, info.active_width, info.active_height],
        channels: info.channels,
        storage_channels: info.storage_channels,
        row_stride: info.row_stride,
        filters: info.filters,
        orientation: info.orientation,
        bits: info.bits,
        dng_version: info.dng_version,
        decoder_flags: info.decoder_flags,
        black: info.black,
        white: info.white,
        camera_mul: info.camera_mul,
        pre_mul: info.pre_mul,
        source_stamp: [info.file_bytes, info.file_stamp],
        flags: info.flags,
        decoder: String::from_utf8_lossy(&info.decoder[..end]).into_owned(),
        cblack: unsafe { std::slice::from_raw_parts(info.cblack, info.cblack_len as usize) }
            .to_vec(),
        curve: unsafe { std::slice::from_raw_parts(info.curve, info.curve_len as usize) }.to_vec(),
    };
    ensure!(!cancelled(), "cancelled sensor checkpoint commit");
    Ok(Checkpoint {
        metadata,
        samples,
        adopted: false,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_copy_ignores_padding_and_fourth_channel_and_rejects_cancellation() {
        let curve = vec![0_u16; 65536];
        let black = vec![0_u32; 4104];
        let mut info: NativeCheckpoint = unsafe { std::mem::zeroed() };
        info.abi = 1;
        info.stage = 1;
        info.width = 2;
        info.height = 2;
        info.active_width = 2;
        info.active_height = 2;
        info.channels = 3;
        info.storage_channels = 4;
        info.row_stride = 10;
        info.storage_len = 18;
        info.cblack = black.as_ptr();
        info.cblack_len = 4104;
        info.curve = curve.as_ptr();
        info.curve_len = 65536;
        let source = [
            1, 2, 3, 99, 4, 5, 6, 99, 888, 888, 7, 8, 9, 99, 10, 11, 12, 99,
        ];
        let frame = unsafe { copy_native(&info, source.as_ptr(), &|| false) }.unwrap();
        assert_eq!(frame.samples, (1..=12).collect::<Vec<_>>());
        assert!(unsafe { copy_native(&info, source.as_ptr(), &|| true) }.is_err());
        let checks = std::sync::atomic::AtomicUsize::new(0);
        assert!(
            unsafe {
                copy_native(&info, source.as_ptr(), &|| {
                    checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 2
                })
            }
            .is_err()
        );
        info.storage_len -= 1;
        assert!(unsafe { copy_native(&info, source.as_ptr(), &|| false) }.is_err());
        info.storage_len += 1;
        let mut other = frame;
        other.metadata.white = 100;
        let reference = unsafe { copy_native(&info, source.as_ptr(), &|| false) }.unwrap();
        assert!(reference.compare(&other).is_err());
        assert_eq!(std::mem::size_of::<NativeCheckpoint>(), 192);
    }
}
