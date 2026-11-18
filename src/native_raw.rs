//! Versioned C ABI; LibRaw state and allocations remain inside the native DLL.
use crate::{
    arw::{ArwInfo, CaptureMetadata, JpegRef},
    imaging::{Level, Pixels},
};
use anyhow::{Result, ensure};
use std::path::Path;

pub struct DevelopSettings {
    pub max_edge: u32,
    pub temperature: Option<f64>,
    pub tint: f64,
    pub exposure: f64,
}
pub struct Developed {
    pub pixels: Pixels,
    pub orientation: u16,
    pub temperature: Option<f64>,
    pub tint: Option<f64>,
    pub camera_multipliers: [f32; 4],
    pub camera_matrix: [[f32; 3]; 4],
    pub open_ms: f64,
    pub unpack_ms: f64,
    pub process_ms: f64,
}
/// Linear, white-balanced camera RGB, normalized to the green multiplier.
/// Values above one retain sensor highlight headroom. Exposure is not applied.
#[derive(Clone)]
pub struct LinearDeveloped {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<[f32; 3]>,
    pub orientation: u16,
    pub temperature: Option<f64>,
    pub tint: Option<f64>,
    pub camera_multipliers: [f32; 4],
    /// WB actually used for the returned RGB, normalized to green = 1.
    pub applied_mul: [f32; 4],
    pub camera_matrix: [[f32; 3]; 4],
    pub camera_to_srgb: [[f32; 4]; 3],
    pub baseline_exposure: f64,
    pub make: String,
    pub model: String,
    pub open_ms: f64,
    pub unpack_ms: f64,
    pub process_ms: f64,
}

#[cfg(any(windows, target_os = "macos", test))]
fn decode_linear_bytes(
    bytes: &[u8],
    count: usize,
    scale: f64,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<[f32; 3]>> {
    ensure!(
        count > 0 && count <= 160_000_000 && count.checked_mul(6) == Some(bytes.len()),
        "无效线性 RAW 缓冲长度"
    );
    ensure!(
        scale.is_finite() && scale > 0. && scale <= 65536.,
        "无效线性 RAW 通道归一尺度"
    );
    ensure!(!cancelled(), "已取消线性 RAW 转换");
    let started = std::time::Instant::now();
    let mut rgb: Vec<[f32; 3]> = Vec::new();
    rgb.try_reserve_exact(count)?;
    let factor = (scale / 65535.) as f32;
    crate::raw_parallel::bands(
        &mut rgb.spare_capacity_mut()[..count],
        1,
        8192,
        |first, band| {
            ensure!(!cancelled(), "已取消线性 RAW 转换");
            let end = (first + band.len()) * 6;
            for (pixel, source) in band.iter_mut().zip(bytes[first * 6..end].chunks_exact(6)) {
                pixel.write(std::array::from_fn(|c| {
                    u16::from_ne_bytes([source[2 * c], source[2 * c + 1]]) as f32 * factor
                }));
            }
            Ok(())
        },
    )?;
    ensure!(!cancelled(), "已取消线性 RAW 转换");
    // Every element was written by a disjoint band. On error length remains
    // zero, so cancellation drops only the allocation, never partial pixels.
    unsafe {
        rgb.set_len(count);
    }
    log::info!(
        "RAW linear conversion_ms={:.3} pixels={count}",
        started.elapsed().as_secs_f64() * 1000.
    );
    Ok(rgb)
}

#[cfg(any(windows, target_os = "macos"))]
mod desktop {
    use super::*;
    #[cfg(windows)]
    use std::os::windows::ffi::OsStrExt;
    use std::{ffi::c_void, sync::OnceLock};
    #[cfg(windows)]
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        LoadLibraryExW,
    };
    #[repr(C)]
    struct Info {
        abi: u32,
        width: u32,
        height: u32,
        orientation: u32,
        pw: u32,
        ph: u32,
        iso: f32,
        aperture: f32,
        shutter: f32,
        focal: f32,
        make: [u8; 64],
        model: [u8; 64],
        lens: [u8; 128],
        captured: [u8; 32],
    }
    impl Default for Info {
        fn default() -> Self {
            unsafe { std::mem::zeroed() }
        }
    }
    #[repr(C)]
    #[derive(Default)]
    struct Frame {
        kind: u32,
        width: u32,
        height: u32,
        channels: u32,
        bits: u32,
        rendered: u32,
        len: u64,
        data: *const u8,
        owner: *mut c_void,
    }
    #[cfg(windows)]
    type PathChar = u16;
    #[cfg(target_os = "macos")]
    type PathChar = std::ffi::c_char;
    type Probe = unsafe extern "C" fn(*const PathChar, *mut Info, *mut u8) -> i32;
    type Decode = unsafe extern "C" fn(*const PathChar, i32, *mut Frame, *mut u8) -> i32;
    type Release = unsafe extern "C" fn(*mut c_void);
    type ScaledJpeg = unsafe extern "C" fn(*const u8, u32, u32, u32, *mut Frame) -> i32;
    #[repr(C)]
    struct DevelopParams {
        abi: u32,
        max_edge: u32,
        temperature: f64,
        tint: f64,
        exposure: f64,
        cancelled: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
        context: *mut c_void,
    }
    #[repr(C)]
    #[derive(Default)]
    struct DevelopInfo {
        abi: u32,
        orientation: u32,
        camera_mul: [f32; 4],
        applied_mul: [f32; 4],
        camera_xyz: [[f32; 3]; 4],
        temperature: f64,
        tint: f64,
        open_ms: f64,
        unpack_ms: f64,
        process_ms: f64,
    }
    #[repr(C)]
    struct LinearInfo {
        abi: u32,
        orientation: u32,
        camera_mul: [f32; 4],
        applied_mul: [f32; 4],
        camera_xyz: [[f32; 3]; 4],
        camera_to_srgb: [[f32; 4]; 3],
        temperature: f64,
        tint: f64,
        baseline_exposure: f64,
        linear_scale: f64,
        open_ms: f64,
        unpack_ms: f64,
        process_ms: f64,
        make: [u8; 64],
        model: [u8; 64],
    }
    impl Default for LinearInfo {
        fn default() -> Self {
            unsafe { std::mem::zeroed() }
        }
    }
    type Develop = unsafe extern "C" fn(
        *const PathChar,
        *const DevelopParams,
        *mut Frame,
        *mut DevelopInfo,
        *mut u8,
    ) -> i32;
    type DevelopLinear = unsafe extern "C" fn(
        *const PathChar,
        *const DevelopParams,
        *mut Frame,
        *mut LinearInfo,
        *mut u8,
    ) -> i32;
    #[repr(C)]
    struct BayerParams {
        abi: u32,
        width: u32,
        height: u32,
        filters: u32,
        xyz: [[f32; 4]; 3],
        cube_root: *const f32,
    }
    type BayerCompute = unsafe extern "C" fn(*mut c_void, *mut u16, *const BayerParams) -> i32;
    type DevelopLinearGpu = unsafe extern "C" fn(
        *const PathChar,
        *const DevelopParams,
        *mut Frame,
        *mut LinearInfo,
        *mut u8,
        Option<BayerCompute>,
        *mut c_void,
    ) -> i32;
    #[repr(C)]
    struct EntropyParams {
        abi: u32,
        kind: u32,
        width: u32,
        height: u32,
        channels: u32,
    }
    /// What the native bridge actually selected for this unpack, plus the
    /// storage/active geometry it saw. Auditable dispatch evidence (Z03).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct EntropyDispatch {
        abi: u32,
        matched: u32,
        reason: u32,
        callback: u32,
        raw_width: u32,
        raw_height: u32,
        active_width: u32,
        active_height: u32,
        filters: u32,
        colors: u32,
        dng_version: u32,
        tiff_samples: u32,
        shot_select: u32,
        raw_pitch: u32,
        decoder: [u8; 32],
    }
    impl Default for EntropyDispatch {
        fn default() -> Self {
            unsafe { std::mem::zeroed() }
        }
    }
    impl EntropyDispatch {
        fn decoder_name(&self) -> String {
            let end = self
                .decoder
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(self.decoder.len());
            String::from_utf8_lossy(&self.decoder[..end]).into_owned()
        }
        fn matched_label(&self) -> &'static str {
            match self.matched {
                0 => "none",
                1 => "sony-ljpeg-tiles",
                2 => "linear-dng-tiles",
                _ => "unknown",
            }
        }
        fn reason_label(&self) -> &'static str {
            match self.reason {
                0 => "adopted",
                1 => "decoder-not-supported",
                2 => "size-limit",
                3 => "no-entropy-callback",
                4 => "unpack-reused",
                5 => "callback-declined",
                6 => "callback-cancelled",
                7 => "commit-layout-unsupported",
                8 => "callback-not-called",
                _ => "unknown",
            }
        }
    }
    type EntropyCompute = unsafe extern "C" fn(*mut c_void, *mut u16, *const EntropyParams) -> i32;
    type LastUnpackDispatch = unsafe extern "C" fn(*mut EntropyDispatch) -> i32;
    type SensorVisit = unsafe extern "C" fn(
        *mut c_void,
        *const crate::raw_sensor::NativeCheckpoint,
        *const u16,
    ) -> i32;
    type SensorCapture = unsafe extern "C" fn(
        *const PathChar,
        *const DevelopParams,
        Option<EntropyCompute>,
        *mut c_void,
        SensorVisit,
        *mut c_void,
        *mut u8,
    ) -> i32;
    type DevelopLinearCompute = unsafe extern "C" fn(
        *const PathChar,
        *const DevelopParams,
        *mut Frame,
        *mut LinearInfo,
        *mut u8,
        Option<BayerCompute>,
        *mut c_void,
        Option<EntropyCompute>,
        *mut c_void,
    ) -> i32;
    type CancelDevelop = unsafe extern "C" fn();
    type WriteJpeg = unsafe extern "C" fn(*const PathChar, *const u8, u32, u32, *mut u8) -> i32;
    struct OwnedFrame {
        frame: Frame,
        release: Release,
    }
    impl Drop for OwnedFrame {
        fn drop(&mut self) {
            unsafe {
                (self.release)(self.frame.owner);
            }
        }
    }
    struct Library {
        probe: Probe,
        decode: Decode,
        release: Release,
        scaled_jpeg: Option<(ScaledJpeg, Release)>,
        develop: Option<Develop>,
        develop_linear: Option<DevelopLinear>,
        develop_linear_gpu: Option<DevelopLinearGpu>,
        develop_linear_compute: Option<DevelopLinearCompute>,
        last_unpack_dispatch: Option<LastUnpackDispatch>,
        sensor_capture: Option<SensorCapture>,
        cancel_develop: Option<CancelDevelop>,
        release_idle: Option<CancelDevelop>,
        write_jpeg: Option<WriteJpeg>,
        #[cfg(target_os = "macos")]
        _module: libloading::Library,
    }
    static LIBRARY: OnceLock<std::result::Result<Library, String>> = OnceLock::new();
    #[cfg(windows)]
    fn library() -> Result<&'static Library> {
        LIBRARY.get_or_init(|| {
            let exe=std::env::current_exe().map_err(|e|e.to_string())?;
            let mut path=exe.with_file_name("fastcull_raw.dll");
            if cfg!(debug_assertions) && !path.exists() {path=Path::new(env!("CARGO_MANIFEST_DIR")).join("lrplugins/FastCull.lrplugin/fastcull_raw.dll");}
            let path:Vec<u16>=path.as_os_str().encode_wide().chain([0]).collect();
            // The module lives for the process lifetime. Symbols are the fixed,
            // versioned ABI of our bundled adapter, never Rust/C++ struct layouts.
            unsafe {
                let module=LoadLibraryExW(path.as_ptr(),std::ptr::null_mut(),LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR|LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
                if module.is_null(){return Err(format!("无法加载 fastcull_raw.dll：{}",std::io::Error::last_os_error()));}
                let probe=GetProcAddress(module,c"fc_raw_info".as_ptr().cast()).ok_or("原生 RAW 元数据接口缺失")?;
                let decode=GetProcAddress(module,c"fc_raw_decode".as_ptr().cast()).ok_or("原生 RAW 解码接口缺失")?;
                let release=GetProcAddress(module,c"fc_raw_release".as_ptr().cast()).ok_or("原生 RAW 释放接口缺失")?;
                let scaled_jpeg = GetProcAddress(module,c"fc_jpeg_scaled".as_ptr().cast())
                    .zip(GetProcAddress(module,c"fc_jpeg_release".as_ptr().cast()))
                    .map(|(decode,release)| (std::mem::transmute::<unsafe extern "system" fn()->isize,ScaledJpeg>(decode), std::mem::transmute::<unsafe extern "system" fn()->isize,Release>(release)));
                let develop=GetProcAddress(module,c"fc_raw_develop".as_ptr().cast()).map(|p|std::mem::transmute::<unsafe extern "system" fn()->isize,Develop>(p));
                let develop_linear=GetProcAddress(module,c"fc_raw_develop_linear".as_ptr().cast()).map(|p|std::mem::transmute::<unsafe extern "system" fn()->isize,DevelopLinear>(p));
                let develop_linear_gpu=GetProcAddress(module,c"fc_raw_develop_linear_gpu".as_ptr().cast()).map(|p|std::mem::transmute::<unsafe extern "system" fn()->isize,DevelopLinearGpu>(p));
                let develop_linear_compute=GetProcAddress(module,c"fc_raw_develop_linear_compute".as_ptr().cast()).map(|p|std::mem::transmute::<unsafe extern "system" fn()->isize,DevelopLinearCompute>(p));
                let last_unpack_dispatch=GetProcAddress(module,c"fc_raw_last_unpack_dispatch".as_ptr().cast()).map(|p|std::mem::transmute::<unsafe extern "system" fn()->isize,LastUnpackDispatch>(p));
                let sensor_capture=GetProcAddress(module,c"fc_raw_sensor_checkpoint_v1".as_ptr().cast()).map(|p|std::mem::transmute::<unsafe extern "system" fn()->isize,SensorCapture>(p));
                let cancel_develop=GetProcAddress(module,c"fc_raw_develop_cancel".as_ptr().cast()).map(|p|std::mem::transmute::<unsafe extern "system" fn()->isize,CancelDevelop>(p));
                let write_jpeg=GetProcAddress(module,c"fc_jpeg_write".as_ptr().cast()).map(|p|std::mem::transmute::<unsafe extern "system" fn()->isize,WriteJpeg>(p));
                let release_idle=GetProcAddress(module,c"fc_raw_develop_idle".as_ptr().cast()).map(|p|std::mem::transmute::<unsafe extern "system" fn()->isize,CancelDevelop>(p));
                Ok(Library {probe:std::mem::transmute::<unsafe extern "system" fn()->isize,Probe>(probe),decode:std::mem::transmute::<unsafe extern "system" fn()->isize,Decode>(decode),release:std::mem::transmute::<unsafe extern "system" fn()->isize,Release>(release),scaled_jpeg,develop,develop_linear,develop_linear_gpu,develop_linear_compute,last_unpack_dispatch,sensor_capture,cancel_develop,release_idle,write_jpeg})
            }
        }).as_ref().map_err(|e|anyhow::anyhow!(e.clone()))
    }
    #[cfg(target_os = "macos")]
    fn library() -> Result<&'static Library> {
        LIBRARY
            .get_or_init(|| {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let adjacent = exe.with_file_name("libfastcull_raw.dylib");
                let bundled = exe
                    .parent()
                    .ok_or("missing executable directory")?
                    .join("../Frameworks/libfastcull_raw.dylib");
                let path = if bundled.is_file() { bundled } else { adjacent };
                // The library is retained alongside its copied C function pointers.
                // Only load from our app bundle or executable directory, never PATH.
                unsafe {
                    let module = libloading::Library::new(&path)
                        .map_err(|e| format!("无法加载 {}：{e}", path.display()))?;
                    let probe = *module
                        .get::<Probe>(b"fc_raw_info\0")
                        .map_err(|e| e.to_string())?;
                    let decode = *module
                        .get::<Decode>(b"fc_raw_decode\0")
                        .map_err(|e| e.to_string())?;
                    let release = *module
                        .get::<Release>(b"fc_raw_release\0")
                        .map_err(|e| e.to_string())?;
                    // Capability probe like Windows: the stock dylib has never
                    // exported fc_jpeg_scaled, so this stays None until a
                    // future libfastcull_raw build ships a native scaled path.
                    let scaled_jpeg = module
                        .get::<ScaledJpeg>(b"fc_jpeg_scaled\0")
                        .ok()
                        .zip(module.get::<Release>(b"fc_jpeg_release\0").ok())
                        .map(|(decode, release)| (*decode, *release));
                    Ok(Library {
                        probe,
                        decode,
                        release,
                        scaled_jpeg,
                        develop: module.get::<Develop>(b"fc_raw_develop\0").ok().map(|p| *p),
                        develop_linear: module
                            .get::<DevelopLinear>(b"fc_raw_develop_linear\0")
                            .ok()
                            .map(|p| *p),
                        develop_linear_gpu: module
                            .get::<DevelopLinearGpu>(b"fc_raw_develop_linear_gpu\0")
                            .ok()
                            .map(|p| *p),
                        develop_linear_compute: module
                            .get::<DevelopLinearCompute>(b"fc_raw_develop_linear_compute\0")
                            .ok()
                            .map(|p| *p),
                        last_unpack_dispatch: module
                            .get::<LastUnpackDispatch>(b"fc_raw_last_unpack_dispatch\0")
                            .ok()
                            .map(|p| *p),
                        sensor_capture: module
                            .get::<SensorCapture>(b"fc_raw_sensor_checkpoint_v1\0")
                            .ok()
                            .map(|p| *p),
                        cancel_develop: module
                            .get::<CancelDevelop>(b"fc_raw_develop_cancel\0")
                            .ok()
                            .map(|p| *p),
                        release_idle: module
                            .get::<CancelDevelop>(b"fc_raw_develop_idle\0")
                            .ok()
                            .map(|p| *p),
                        write_jpeg: None,
                        _module: module,
                    })
                }
            })
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))
    }
    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes.split(|v| *v == 0).next().unwrap_or_default())
            .trim()
            .to_owned()
    }
    pub fn cancel_develop() {
        if let Ok(lib) = library()
            && let Some(cancel) = lib.cancel_develop
        {
            unsafe {
                cancel();
            }
        }
    }
    pub fn release_idle() {
        if let Ok(lib) = library()
            && let Some(release) = lib.release_idle
        {
            unsafe {
                release();
            }
        }
    }
    pub fn write_jpeg(path: &Path, pixels: &Pixels) -> Result<()> {
        let write = library()?
            .write_jpeg
            .ok_or_else(|| anyhow::anyhow!("原生 JPEG 写出接口缺失，请更新 fastcull_raw.dll"))?;
        let path = native_path(path)?;
        let mut error = [0_u8; 256];
        let status = unsafe {
            write(
                path.as_ptr(),
                pixels.rgba.as_ptr(),
                pixels.width.try_into()?,
                pixels.height.try_into()?,
                error.as_mut_ptr(),
            )
        };
        ensure!(status == 0, "JPEG 写出：{}", text(&error));
        Ok(())
    }
    pub fn develop(
        path: &Path,
        settings: &DevelopSettings,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Developed> {
        let source = crate::dng::jxl_native_source(path, cancelled)?;
        let path = source.as_ref().map_or(path, |f| f.as_ref().as_ref());
        let lib = library()?;
        let develop = lib
            .develop
            .ok_or_else(|| anyhow::anyhow!("本地 RAW 显影接口缺失，请更新 fastcull_raw.dll"))?;
        struct Context<'a> {
            cancelled: &'a (dyn Fn() -> bool + Sync),
        }
        unsafe extern "C" fn check(pointer: *mut c_void) -> i32 {
            let context = unsafe { &*(pointer as *const Context<'_>) };
            i32::from((context.cancelled)())
        }
        let mut context = Context { cancelled };
        let params = DevelopParams {
            abi: 1,
            max_edge: settings.max_edge,
            temperature: settings.temperature.unwrap_or(0.0),
            tint: settings.tint,
            exposure: settings.exposure,
            cancelled: Some(check),
            context: (&mut context as *mut Context<'_>).cast(),
        };
        let mut frame = Frame::default();
        let mut info = DevelopInfo::default();
        let mut error = [0_u8; 256];
        let path = native_path(path)?;
        let status = unsafe {
            develop(
                path.as_ptr(),
                &params,
                &mut frame,
                &mut info,
                error.as_mut_ptr(),
            )
        };
        ensure!(status == 0, "本地 RAW 显影：{}", text(&error));
        let owner = OwnedFrame {
            frame,
            release: lib.release,
        };
        let frame = &owner.frame;
        ensure!(
            info.abi == 1
                && frame.kind == 2
                && frame.bits == 8
                && frame.channels == 3
                && !frame.data.is_null(),
            "无效本地 RAW 显影帧"
        );
        let count = u64::from(frame.width) * u64::from(frame.height);
        ensure!(
            count > 0 && count <= 160_000_000 && frame.len == count * 3,
            "本地 RAW 显影尺寸超出限制"
        );
        let source = unsafe { std::slice::from_raw_parts(frame.data, frame.len as usize) };
        let mut rgba = Vec::with_capacity(count as usize * 4);
        for pixel in source.chunks_exact(3) {
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
        let has_wb =
            info.temperature.is_finite() && info.temperature > 0.0 && info.tint.is_finite();
        Ok(Developed {
            pixels: Pixels {
                width: frame.width as usize,
                height: frame.height as usize,
                rgba,
            },
            orientation: info.orientation as u16,
            temperature: has_wb.then_some(info.temperature),
            tint: has_wb.then_some(info.tint),
            camera_multipliers: info.camera_mul,
            camera_matrix: info.camera_xyz,
            open_ms: info.open_ms,
            unpack_ms: info.unpack_ms,
            process_ms: info.process_ms,
        })
    }
    struct LinearContext<'a> {
        cancelled: &'a (dyn Fn() -> bool + Sync),
        path: &'a Path,
        adoption: std::sync::Mutex<EntropyAdoption>,
    }
    /// What the entropy callback actually did, for the dispatch audit.
    #[derive(Default)]
    struct EntropyAdoption {
        called: bool,
        backend: String,
        reason: String,
        tiles: usize,
        samples: usize,
        h2d_bytes: u64,
        d2h_bytes: u64,
        ms: f64,
    }
    impl EntropyAdoption {
        fn failed(&mut self, reason: String, ms: f64, cancelled: bool) {
            self.called = true;
            self.backend = if cancelled {
                "cancelled"
            } else {
                "libraw-unpack"
            }
            .into();
            self.reason = reason;
            self.ms = ms;
        }
    }
    unsafe extern "C" fn entropy_callback(
        pointer: *mut c_void,
        destination: *mut u16,
        params: *const EntropyParams,
    ) -> i32 {
        let context = unsafe { &*(pointer as *const LinearContext<'_>) };
        let started = std::time::Instant::now();
        {
            let mut record = context.adoption.lock().unwrap_or_else(|e| e.into_inner());
            record.called = true;
            record.backend = "libraw-unpack".into();
            record.reason = "callback-pending".into();
        }
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<bool> {
            ensure!(
                !params.is_null() && !destination.is_null(),
                "missing entropy callback data"
            );
            let p = unsafe { &*params };
            let count = u64::from(p.width)
                .checked_mul(u64::from(p.height))
                .and_then(|n| n.checked_mul(u64::from(p.channels)))
                .ok_or_else(|| anyhow::anyhow!("entropy size overflow"))?;
            let kind = match p.kind {
                1 => crate::raw_entropy::Kind::Sony,
                2 => crate::raw_entropy::Kind::LinearDng,
                _ => anyhow::bail!("unsupported entropy kind"),
            };
            ensure!(
                (p.abi == 1 || p.abi == 2)
                    && count > 0
                    && count <= 160_000_000
                    && p.channels == if p.kind == 1 { 1 } else { 3 },
                "invalid entropy ABI"
            );
            ensure!(!(context.cancelled)(), "cancelled entropy prepare");
            let start = std::time::Instant::now();
            let image =
                crate::raw_entropy::read(context.path, kind, p.width as usize, p.height as usize)?;
            match crate::raw_compute::try_entropy(&image, context.cancelled)? {
                crate::raw_compute::Outcome::Adopted(output, adoption) => {
                    ensure!(
                        !(context.cancelled)() && output.len() == count as usize,
                        "cancelled or invalid entropy commit"
                    );
                    let target =
                        unsafe { std::slice::from_raw_parts_mut(destination, count as usize) };
                    target.copy_from_slice(&output);
                    let mut record = context.adoption.lock().unwrap_or_else(|e| e.into_inner());
                    *record = EntropyAdoption {
                        called: true,
                        backend: adoption.backend.to_string(),
                        reason: adoption.reason,
                        tiles: adoption.tiles,
                        samples: adoption.samples,
                        h2d_bytes: adoption.h2d_bytes,
                        d2h_bytes: adoption.d2h_bytes,
                        ms: adoption.ms,
                    };
                    log::info!(
                        "RAW entropy prepare+decode+commit backend={} ms={:.3}",
                        record.backend,
                        start.elapsed().as_secs_f64() * 1000.
                    );
                    Ok(true)
                }
                crate::raw_compute::Outcome::Unavailable(reason) => {
                    let mut record = context.adoption.lock().unwrap_or_else(|e| e.into_inner());
                    *record = EntropyAdoption {
                        called: true,
                        backend: "libraw-unpack".into(),
                        reason,
                        tiles: image.tiles.len(),
                        samples: image.width * image.height * image.channels,
                        ms: start.elapsed().as_secs_f64() * 1000.,
                        ..Default::default()
                    };
                    Ok(false)
                }
            }
        }));
        if (context.cancelled)() {
            context
                .adoption
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .failed(
                    "cancelled entropy callback".into(),
                    started.elapsed().as_secs_f64() * 1000.,
                    true,
                );
            return -1;
        }
        match attempt {
            Ok(Ok(done)) => i32::from(done),
            Ok(Err(e)) => {
                context
                    .adoption
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .failed(
                        format!("entropy callback failed: {e:#}"),
                        started.elapsed().as_secs_f64() * 1000.,
                        false,
                    );
                log::debug!("RAW tiled entropy uses LibRaw: {e:#}");
                0
            }
            Err(_) => {
                context
                    .adoption
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .failed(
                        "entropy callback panicked".into(),
                        started.elapsed().as_secs_f64() * 1000.,
                        false,
                    );
                log::warn!("RAW entropy callback panicked; using LibRaw");
                0
            }
        }
    }
    pub fn develop_linear(
        path: &Path,
        settings: &DevelopSettings,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<LinearDeveloped> {
        let source = crate::dng::jxl_native_source(path, cancelled)?;
        let path = source.as_ref().map_or(path, |f| f.as_ref().as_ref());
        let lib = library()?;
        let develop = lib
            .develop_linear
            .ok_or_else(|| anyhow::anyhow!("高精度 RAW 显影接口缺失，请更新 fastcull_raw.dll"))?;
        unsafe extern "C" fn check(pointer: *mut c_void) -> i32 {
            let context = unsafe { &*(pointer as *const LinearContext<'_>) };
            i32::from((context.cancelled)())
        }
        unsafe extern "C" fn bayer(
            pointer: *mut c_void,
            image: *mut u16,
            params: *const BayerParams,
        ) -> i32 {
            let context = unsafe { &*(pointer as *const LinearContext<'_>) };
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<bool> {
                    ensure!(
                        !params.is_null() && !image.is_null(),
                        "missing GPU Bayer data"
                    );
                    let p = unsafe { &*params };
                    let count = u64::from(p.width) * u64::from(p.height);
                    ensure!(
                        p.abi == 1 && count > 0 && count <= 160_000_000 && !p.cube_root.is_null(),
                        "invalid Bayer ABI"
                    );
                    let source = unsafe {
                        std::slice::from_raw_parts_mut(image.cast::<[u16; 4]>(), count as usize)
                    };
                    let table = unsafe { std::slice::from_raw_parts(p.cube_root, 65536) };
                    let data = crate::raw_compute::Bayer {
                        image: source,
                        width: p.width as usize,
                        height: p.height as usize,
                        filters: p.filters,
                        xyz: p.xyz,
                        cube_root: table,
                    };
                    if let Some(output) = crate::raw_compute::try_demosaic(data, context.cancelled)?
                    {
                        ensure!(!(context.cancelled)(), "cancelled GPU Bayer commit");
                        source.copy_from_slice(&output);
                        return Ok(true);
                    }
                    Ok(false)
                }));
            if (context.cancelled)() {
                return -1;
            }
            match result {
                Ok(Ok(done)) => i32::from(done),
                Ok(Err(e)) => {
                    log::warn!("GPU Bayer callback: {e:#}");
                    0
                }
                Err(_) => {
                    log::warn!("GPU Bayer callback panicked; using CPU");
                    0
                }
            }
        }
        /// Emit one auditable record of what the real edit path used.
        fn report_unpack_dispatch(
            lib: &Library,
            context: &LinearContext<'_>,
            entry: &str,
            ok: bool,
        ) {
            let mut bridge = EntropyDispatch::default();
            let bridge_ok = lib
                .last_unpack_dispatch
                .is_some_and(|poll| unsafe { poll(&mut bridge) } != 0);
            let adoption = context.adoption.lock().unwrap_or_else(|e| e.into_inner());
            let decoder = if bridge_ok {
                bridge.decoder_name()
            } else {
                "unknown".to_string()
            };
            let backend = if bridge_ok && bridge.reason == 4 {
                "unpack-reused"
            } else if bridge_ok && bridge.reason == 6 {
                "cancelled"
            } else if bridge_ok && matches!(bridge.reason, 5 | 7 | 8) {
                "libraw-unpack"
            } else if adoption.called {
                adoption.backend.as_str()
            } else {
                "libraw-unpack"
            };
            let reason = if bridge_ok && matches!(bridge.reason, 5..=8) {
                format!("{}: {}", bridge.reason_label(), adoption.reason)
            } else if !adoption.reason.is_empty() {
                adoption.reason.clone()
            } else if bridge_ok {
                bridge.reason_label().to_string()
            } else {
                "bridge-report-unavailable".to_string()
            };
            let (raw_width, raw_height, active_width, active_height) = if bridge_ok {
                (
                    bridge.raw_width,
                    bridge.raw_height,
                    bridge.active_width,
                    bridge.active_height,
                )
            } else {
                (0, 0, 0, 0)
            };
            log::info!(
                "RAW unpack dispatch entry={entry} ok={ok} decoder={decoder} matched={} \
                 bridge_reason={} raw={raw_width}x{raw_height} active={active_width}x{active_height} \
                 filters={} colors={} callback={} backend={backend} tiles={} samples={} \
                 h2d_bytes={} d2h_bytes={} entropy_ms={:.3} fallback={reason}",
                bridge.matched_label(),
                bridge.reason_label(),
                bridge.filters,
                bridge.colors,
                adoption.called,
                adoption.tiles,
                adoption.samples,
                adoption.h2d_bytes,
                adoption.d2h_bytes,
                adoption.ms,
            );
            crate::raw_trace::event(
                "entropy_dispatch",
                vec![
                    ("entry", crate::raw_trace::string(entry)),
                    ("ok", ok.to_string()),
                    ("decoder", crate::raw_trace::string(&decoder)),
                    ("bridge_abi", bridge.abi.to_string()),
                    ("matched", crate::raw_trace::string(bridge.matched_label())),
                    (
                        "bridge_reason",
                        crate::raw_trace::string(bridge.reason_label()),
                    ),
                    ("raw_width", raw_width.to_string()),
                    ("raw_height", raw_height.to_string()),
                    ("active_width", active_width.to_string()),
                    ("active_height", active_height.to_string()),
                    ("filters", bridge.filters.to_string()),
                    ("colors", bridge.colors.to_string()),
                    ("dng_version", bridge.dng_version.to_string()),
                    ("tiff_samples", bridge.tiff_samples.to_string()),
                    ("callback", adoption.called.to_string()),
                    ("backend", crate::raw_trace::string(backend)),
                    ("tiles", adoption.tiles.to_string()),
                    ("samples", adoption.samples.to_string()),
                    ("h2d_bytes", adoption.h2d_bytes.to_string()),
                    ("d2h_bytes", adoption.d2h_bytes.to_string()),
                    ("entropy_ms", format!("{:.3}", adoption.ms)),
                    ("fallback", crate::raw_trace::string(&reason)),
                ],
            );
        }
        let mut context = LinearContext {
            cancelled,
            path,
            adoption: std::sync::Mutex::new(EntropyAdoption::default()),
        };
        let params = DevelopParams {
            abi: 1,
            max_edge: settings.max_edge,
            temperature: settings.temperature.unwrap_or(0.),
            tint: settings.tint,
            // Exposure belongs to the downstream float color pipeline. It is
            // deliberately not included in this linear sensor decode.
            exposure: 0.,
            cancelled: Some(check),
            context: (&mut context as *mut LinearContext<'_>).cast(),
        };
        let mut frame = Frame::default();
        let mut info = LinearInfo::default();
        let mut error = [0; 256];
        let warm = settings.max_edge == 0
            && crate::raw_compute::demosaic_warmup_enabled()
            && context
                .path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("arw") || e.eq_ignore_ascii_case("nef"));
        let path = native_path(path)?;
        let compute = lib
            .develop_linear_compute
            .filter(|_| crate::raw_compute::entropy_enabled());
        let gpu_entry = lib
            .develop_linear_gpu
            .filter(|_| settings.max_edge == 0 && compute.is_none());
        let entry = if compute.is_some() {
            "develop_linear_compute"
        } else if gpu_entry.is_some() {
            "develop_linear_gpu"
        } else {
            "develop_linear"
        };
        let status = std::thread::scope(|scope| {
            let prewarm = warm.then(|| scope.spawn(crate::raw_compute::prewarm_demosaic));
            let status = unsafe {
                if let Some(compute) = compute {
                    let ctx = (&mut context as *mut LinearContext<'_>).cast();
                    compute(
                        path.as_ptr(),
                        &params,
                        &mut frame,
                        &mut info,
                        error.as_mut_ptr(),
                        Some(bayer),
                        ctx,
                        Some(entropy_callback),
                        ctx,
                    )
                } else if let Some(gpu) = gpu_entry {
                    gpu(
                        path.as_ptr(),
                        &params,
                        &mut frame,
                        &mut info,
                        error.as_mut_ptr(),
                        Some(bayer),
                        (&mut context as *mut LinearContext<'_>).cast(),
                    )
                } else {
                    develop(
                        path.as_ptr(),
                        &params,
                        &mut frame,
                        &mut info,
                        error.as_mut_ptr(),
                    )
                }
            };
            if let Some(prewarm) = prewarm {
                let _ = prewarm.join();
            }
            status
        });
        report_unpack_dispatch(lib, &context, entry, status == 0);
        ensure!(status == 0, "高精度 RAW 显影：{}", text(&error));
        let owner = OwnedFrame {
            frame,
            release: lib.release,
        };
        let frame = &owner.frame;
        ensure!(
            info.abi == 1
                && frame.kind == 2
                && frame.bits == 16
                && frame.channels == 3
                && !frame.data.is_null(),
            "无效高精度 RAW 显影帧"
        );
        let count = u64::from(frame.width) * u64::from(frame.height);
        ensure!(
            count > 0 && count <= 160_000_000 && frame.len == count * 6,
            "高精度 RAW 显影尺寸超出限制"
        );
        let source = unsafe { std::slice::from_raw_parts(frame.data, frame.len as usize) };
        let rgb = decode_linear_bytes(source, count as usize, info.linear_scale, cancelled)?;
        let has_wb = info.temperature.is_finite()
            && info.temperature >= 2000.
            && info.temperature <= 50000.
            && info.tint.is_finite()
            && info.tint.abs() <= 150.;
        ensure!(
            info.camera_to_srgb.iter().flatten().all(|v| v.is_finite())
                && info.camera_xyz.iter().flatten().all(|v| v.is_finite())
                && info.baseline_exposure.is_finite(),
            "无效高精度 RAW 色彩元数据"
        );
        Ok(LinearDeveloped {
            width: frame.width as usize,
            height: frame.height as usize,
            rgb,
            orientation: info.orientation as u16,
            temperature: has_wb.then_some(info.temperature),
            tint: has_wb.then_some(info.tint),
            camera_multipliers: info.camera_mul,
            applied_mul: info.applied_mul,
            camera_matrix: info.camera_xyz,
            camera_to_srgb: info.camera_to_srgb,
            baseline_exposure: info.baseline_exposure,
            make: text(&info.make),
            model: text(&info.model),
            open_ms: info.open_ms,
            unpack_ms: info.unpack_ms,
            process_ms: info.process_ms,
        })
    }
    pub fn sensor_checkpoint(
        path: &Path,
        backend: crate::raw_sensor::Backend,
        gpu: Option<&crate::raw_compute::Context>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<crate::raw_sensor::Checkpoint> {
        use crate::raw_sensor::{Backend, Checkpoint, NativeCheckpoint};
        struct Context<'a> {
            path: &'a Path,
            backend: Backend,
            gpu: Option<&'a crate::raw_compute::Context>,
            cancelled: &'a (dyn Fn() -> bool + Sync),
            output: std::sync::Mutex<Option<Result<Checkpoint>>>,
            adopted: std::sync::atomic::AtomicBool,
        }
        unsafe extern "C" fn check(p: *mut c_void) -> i32 {
            i32::from((unsafe { &*p.cast::<Context<'_>>() }.cancelled)())
        }
        unsafe extern "C" fn visit(
            p: *mut c_void,
            info: *const NativeCheckpoint,
            data: *const u16,
        ) -> i32 {
            let context = unsafe { &*p.cast::<Context<'_>>() };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ensure!(!info.is_null(), "missing sensor descriptor");
                unsafe { crate::raw_sensor::copy_native(&*info, data, context.cancelled) }
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("sensor visitor panicked")));
            let ok = result.is_ok();
            *context.output.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            i32::from(ok)
        }
        unsafe extern "C" fn entropy(
            p: *mut c_void,
            destination: *mut u16,
            params: *const EntropyParams,
        ) -> i32 {
            let context = unsafe { &*p.cast::<Context<'_>>() };
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                    ensure!(
                        !params.is_null() && !destination.is_null(),
                        "missing sensor entropy input"
                    );
                    let params = unsafe { &*params };
                    ensure!(
                        params.abi == 1 || params.abi == 2,
                        "invalid sensor entropy ABI"
                    );
                    let kind = match params.kind {
                        1 => crate::raw_entropy::Kind::Sony,
                        2 => crate::raw_entropy::Kind::LinearDng,
                        _ => anyhow::bail!("unsupported sensor entropy kind"),
                    };
                    let image = crate::raw_entropy::read(
                        context.path,
                        kind,
                        params.width as usize,
                        params.height as usize,
                    )?;
                    let samples = match context.backend {
                        Backend::Parallel => {
                            crate::raw_entropy::decode_cpu(&image, context.cancelled)?
                        }
                        Backend::Gpu => {
                            context
                                .gpu
                                .ok_or_else(|| anyhow::anyhow!("sensor GPU context missing"))?
                                .decode_entropy(&image, context.cancelled)?
                                .0
                        }
                        Backend::LibRaw => anyhow::bail!("unexpected reference callback"),
                    };
                    let count = u64::from(params.width)
                        * u64::from(params.height)
                        * u64::from(params.channels);
                    ensure!(
                        !(context.cancelled)()
                            && count <= 160_000_000
                            && samples.len() as u64 == count,
                        "cancelled or incomplete sensor entropy"
                    );
                    unsafe { std::slice::from_raw_parts_mut(destination, samples.len()) }
                        .copy_from_slice(&samples);
                    context
                        .adopted
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    Ok(())
                }));
            if (context.cancelled)() {
                return -1;
            }
            match result {
                Ok(Ok(())) => 1,
                other => {
                    log::warn!("sensor diagnostic entropy fallback: {other:?}");
                    0
                }
            }
        }
        let lib = library()?;
        let capture = lib
            .sensor_capture
            .ok_or_else(|| anyhow::anyhow!("传感器检查点接口缺失，请使用配套候选 DLL"))?;
        let mut context = Context {
            path,
            backend,
            gpu,
            cancelled,
            output: std::sync::Mutex::new(None),
            adopted: std::sync::atomic::AtomicBool::new(false),
        };
        let pointer = (&mut context as *mut Context<'_>).cast();
        let params = DevelopParams {
            abi: 1,
            max_edge: 0,
            temperature: 0.,
            tint: 0.,
            exposure: 0.,
            cancelled: Some(check),
            context: pointer,
        };
        let mut error = [0_u8; 256];
        let native_path = native_path(path)?;
        let status = unsafe {
            capture(
                native_path.as_ptr(),
                &params,
                (backend != Backend::LibRaw).then_some(entropy),
                pointer,
                visit,
                pointer,
                error.as_mut_ptr(),
            )
        };
        let output = context
            .output
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .transpose()?;
        ensure!(status == 0, "传感器检查点：{}", text(&error));
        ensure!(!cancelled(), "cancelled sensor checkpoint publish");
        let mut output = output.ok_or_else(|| anyhow::anyhow!("sensor visitor did not publish"))?;
        let mut dispatch = EntropyDispatch::default();
        output.adopted=context.adopted.load(std::sync::atomic::Ordering::Relaxed)
            && lib.last_unpack_dispatch.is_some_and(|poll|unsafe{poll(&mut dispatch)}!=0&&dispatch.reason==0&&dispatch.callback==1);
        Ok(output)
    }
    pub fn scaled_jpeg(bytes: &[u8], width: u32, height: u32) -> Option<Pixels> {
        // A future libfastcull_raw export wins; otherwise ImageIO covers the
        // stock macOS dylib, which has never shipped fc_jpeg_scaled. Any None
        // falls back to the bounded Rust decoder in `imaging`.
        #[cfg(target_os = "macos")]
        if library().ok().and_then(|lib| lib.scaled_jpeg).is_none() {
            return crate::imageio::scaled_jpeg(bytes, width, height);
        }
        let (decode, release) = library().ok()?.scaled_jpeg?;
        let mut frame = Frame::default();
        let status = unsafe {
            decode(
                bytes.as_ptr(),
                bytes.len().try_into().ok()?,
                width,
                height,
                &mut frame,
            )
        };
        if status != 0 {
            return None;
        }
        let owned = OwnedFrame { frame, release };
        let f = &owned.frame;
        let expected = u64::from(f.width) * u64::from(f.height) * 4;
        if f.data.is_null()
            || f.width < width
            || f.height < height
            || expected > 640_000_000
            || f.len != expected
        {
            return None;
        }
        Some(Pixels {
            width: f.width as usize,
            height: f.height as usize,
            rgba: unsafe { std::slice::from_raw_parts(f.data, f.len as usize) }.to_vec(),
        })
    }
    fn optional(bytes: &[u8]) -> Option<String> {
        let s = text(bytes);
        (!s.is_empty()).then_some(s)
    }
    fn positive(value: f32) -> Option<f64> {
        (value.is_finite() && value > 0.).then_some(value as f64)
    }
    #[cfg(windows)]
    fn native_path(path: &Path) -> Result<Vec<u16>> {
        let path: Vec<_> = path.as_os_str().encode_wide().collect();
        ensure!(!path.contains(&0), "RAW 路径含 NUL");
        Ok(path.into_iter().chain([0]).collect())
    }
    #[cfg(target_os = "macos")]
    fn native_path(path: &Path) -> Result<std::ffi::CString> {
        use std::os::unix::ffi::OsStrExt;
        Ok(std::ffi::CString::new(path.as_os_str().as_bytes())?)
    }
    pub fn read(path: &Path) -> Result<ArwInfo> {
        let source = crate::dng::jxl_native_source(path, &|| false)?;
        let path = source.as_ref().map_or(path, |f| f.as_ref().as_ref());
        let lib = library()?;
        let mut info = Info::default();
        let mut error = [0u8; 256];
        let path = native_path(path)?;
        let code = unsafe { (lib.probe)(path.as_ptr(), &mut info, error.as_mut_ptr()) };
        ensure!(code == 0, "RAW 元数据：{}", text(&error));
        ensure!(info.abi == 1, "RAW 适配层版本不匹配");
        let capture = CaptureMetadata {
            make: optional(&info.make),
            camera: optional(&info.model),
            lens: optional(&info.lens),
            captured_at: optional(&info.captured),
            iso: positive(info.iso).map(|v| v.round() as u32),
            aperture: positive(info.aperture),
            shutter: positive(info.shutter),
            focal: positive(info.focal),
        };
        let summary = format!(
            "{}  RAW {}×{}{}",
            capture.summary(),
            info.width,
            info.height,
            if info.pw > 0 && info.ph > 0 {
                format!(" · 预览 {}×{}", info.pw, info.ph)
            } else {
                String::new()
            }
        );
        let jpeg = JpegRef { offset: 0, len: 0 };
        Ok(ArwInfo {
            jpeg: jpeg.clone(),
            preview: jpeg.clone(),
            thumbnail: jpeg,
            bitmap_thumbnail: None,
            jxl_preview: false,
            orientation: info.orientation as u16,
            summary,
            capture,
            raw_size: Some((info.width, info.height)),
            image_size: Some((info.width, info.height)),
        })
    }
    fn frame(path: &Path, full: bool) -> Result<OwnedFrame> {
        let source = crate::dng::jxl_native_source(path, &|| false)?;
        let path = source.as_ref().map_or(path, |f| f.as_ref().as_ref());
        let lib = library()?;
        let mut frame = Frame::default();
        let mut error = [0u8; 256];
        let wide = native_path(path)?;
        let code = unsafe {
            (lib.decode)(
                wide.as_ptr(),
                i32::from(full),
                &mut frame,
                error.as_mut_ptr(),
            )
        };
        ensure!(code == 0, "RAW 解码：{}", text(&error));
        let owned = OwnedFrame {
            frame,
            release: lib.release,
        };
        let frame = &owned.frame;
        ensure!(
            !frame.data.is_null() && frame.len > 0 && frame.len <= 640_000_000,
            "invalid native RAW buffer"
        );
        Ok(owned)
    }
    pub fn preview_jpeg(path: &Path) -> Result<Vec<u8>> {
        let owned = frame(path, false)?;
        let frame = &owned.frame;
        ensure!(frame.kind == 1, "该 RAW 没有 JPEG 格式的内嵌预览");
        Ok(unsafe { std::slice::from_raw_parts(frame.data, frame.len as usize) }.to_vec())
    }
    pub fn decode(
        path: &Path,
        level: Level,
        geometry: &mut dyn FnMut([usize; 2]),
    ) -> Result<Pixels> {
        let owned = frame(path, level == Level::Full)?;
        let frame = &owned.frame;
        let bytes = unsafe { std::slice::from_raw_parts(frame.data, frame.len as usize) };
        let pixels = if frame.kind == 1 {
            crate::imaging::decode_jpeg_bytes_with_geometry(bytes, level, geometry)?
        } else {
            ensure!(
                frame.kind == 2 && frame.bits == 8 && (1..=4).contains(&frame.channels),
                "unsupported native bitmap"
            );
            let n = frame.width as usize * frame.height as usize;
            let channels = frame.channels as usize;
            ensure!(
                n <= 160_000_000 && n.checked_mul(channels) == Some(bytes.len()),
                "invalid native bitmap dimensions"
            );
            geometry([frame.width as usize, frame.height as usize]);
            let mut rgba = Vec::with_capacity(n * 4);
            for p in bytes.chunks_exact(channels) {
                if channels < 3 {
                    rgba.extend([p[0], p[0], p[0], 255]);
                } else {
                    rgba.extend([p[0], p[1], p[2], 255]);
                }
            }
            let mut pixels = Pixels {
                width: frame.width as usize,
                height: frame.height as usize,
                rgba,
            };
            if frame.rendered != 0 {
                crate::dng::jxl_mask(path, &mut pixels)?;
            }
            crate::imaging::resize_level(pixels, level)?
        };
        if frame.rendered != 0 {
            log::info!(
                "RAW sensor render full={} {}x{} {}",
                level == Level::Full,
                pixels.width,
                pixels.height,
                path.display()
            );
        }
        Ok(pixels)
    }
    #[cfg(test)]
    mod linear_abi_tests {
        use super::*;
        #[test]
        fn linear_info_layout_matches_the_native_abi() {
            assert_eq!(std::mem::size_of::<LinearInfo>(), 320);
            assert_eq!(std::mem::align_of::<LinearInfo>(), 8);
            assert_eq!(std::mem::offset_of!(LinearInfo, temperature), 136);
            assert_eq!(std::mem::offset_of!(LinearInfo, linear_scale), 160);
            assert_eq!(std::mem::offset_of!(LinearInfo, make), 192);
            assert_eq!(std::mem::offset_of!(LinearInfo, model), 256);
        }
        #[test]
        fn entropy_dispatch_layout_matches_the_native_abi() {
            // 14 x u32 + 32-byte decoder name; the C++ bridge asserts the same.
            assert_eq!(std::mem::size_of::<EntropyDispatch>(), 88);
            assert_eq!(std::mem::align_of::<EntropyDispatch>(), 4);
            assert_eq!(std::mem::offset_of!(EntropyDispatch, raw_width), 16);
            assert_eq!(std::mem::offset_of!(EntropyDispatch, decoder), 56);
            // Read only the original v1 prefix, also safe with the temporary
            // ABI=2 bridge which appended an unused pointer to this prefix.
            assert_eq!(std::mem::size_of::<EntropyParams>(), 20);
            assert_eq!(std::mem::align_of::<EntropyParams>(), 4);
            assert_eq!(std::mem::offset_of!(EntropyParams, channels), 16);
        }
        #[test]
        fn entropy_parse_failure_records_real_attempt_and_leaves_destination_untouched() {
            for abi in [1, 2] {
                let missing = tempfile::tempdir().unwrap();
                let path = missing.path().join("missing.arw");
                let mut context = LinearContext {
                    path: &path,
                    cancelled: &|| false,
                    adoption: std::sync::Mutex::new(EntropyAdoption::default()),
                };
                let params = EntropyParams {
                    abi,
                    kind: 1,
                    width: 2,
                    height: 2,
                    channels: 1,
                };
                let mut destination = [12345_u16; 4];
                let status = unsafe {
                    entropy_callback(
                        (&mut context as *mut LinearContext<'_>).cast(),
                        destination.as_mut_ptr(),
                        &params,
                    )
                };
                assert_eq!(status, 0);
                assert_eq!(destination, [12345; 4]);
                let record = context.adoption.lock().unwrap();
                assert!(record.called);
                assert_eq!(record.backend, "libraw-unpack");
                assert!(record.reason.contains("entropy callback failed"));
            }
        }
        #[test]
        fn entropy_cancellation_records_no_committed_output() {
            let mut context = LinearContext {
                path: Path::new("unused.arw"),
                cancelled: &|| true,
                adoption: std::sync::Mutex::new(EntropyAdoption::default()),
            };
            let params = EntropyParams {
                abi: 1,
                kind: 1,
                width: 2,
                height: 2,
                channels: 1,
            };
            let mut destination = [12345_u16; 4];
            let status = unsafe {
                entropy_callback(
                    (&mut context as *mut LinearContext<'_>).cast(),
                    destination.as_mut_ptr(),
                    &params,
                )
            };
            assert_eq!(status, -1);
            assert_eq!(destination, [12345; 4]);
            let record = context.adoption.lock().unwrap();
            assert!(record.called);
            assert_eq!(record.backend, "cancelled");
        }
    }
}
#[cfg(any(windows, target_os = "macos"))]
pub(crate) use desktop::sensor_checkpoint;
pub fn read(path: &Path) -> Result<ArwInfo> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        desktop::read(path)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = path;
        anyhow::bail!("此构建没有原生 RAW 解码器")
    }
}
pub(crate) fn scaled_jpeg(bytes: &[u8], width: u32, height: u32) -> Option<Pixels> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        desktop::scaled_jpeg(bytes, width, height)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (bytes, width, height);
        None
    }
}
pub fn decode(path: &Path, level: Level) -> Result<Pixels> {
    decode_with_geometry(path, level, &mut |_| {})
}
pub fn decode_with_geometry(
    path: &Path,
    level: Level,
    geometry: &mut dyn FnMut([usize; 2]),
) -> Result<Pixels> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        desktop::decode(path, level, geometry)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (path, level, geometry);
        anyhow::bail!("此构建没有原生 RAW 解码器")
    }
}
pub fn preview_jpeg(path: &Path) -> Result<Vec<u8>> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        desktop::preview_jpeg(path)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = path;
        anyhow::bail!("此构建没有原生 RAW 解码器")
    }
}
pub fn develop(
    path: &Path,
    settings: &DevelopSettings,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Developed> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        desktop::develop(path, settings, cancelled)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (path, settings, cancelled);
        anyhow::bail!("此构建没有本地 RAW 显影器")
    }
}
pub fn develop_linear(
    path: &Path,
    settings: &DevelopSettings,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<LinearDeveloped> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        desktop::develop_linear(path, settings, cancelled)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (path, settings, cancelled);
        anyhow::bail!("此构建没有高精度 RAW 显影器")
    }
}
pub fn cancel_develop() {
    #[cfg(any(windows, target_os = "macos"))]
    desktop::cancel_develop();
}
pub fn release_idle() {
    #[cfg(any(windows, target_os = "macos"))]
    desktop::release_idle();
}
pub fn write_jpeg(path: &Path, pixels: &Pixels) -> Result<()> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        desktop::write_jpeg(path, pixels)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (path, pixels);
        anyhow::bail!("此构建没有原生 JPEG 写出器")
    }
}

#[cfg(test)]
mod linear_tests {
    use super::*;
    #[test]
    fn parallel_conversion_matches_scalar_through_partial_final_band_and_cancellation() {
        let count = 300_007_usize;
        let bytes: Vec<u8> = (0..count * 6)
            .map(|i| (i.wrapping_mul(37) >> 3) as u8)
            .collect();
        let factor = (3.75_f64 / 65535.) as f32;
        let expected: Vec<[f32; 3]> = bytes
            .chunks_exact(6)
            .map(|p| {
                std::array::from_fn(|c| {
                    u16::from_ne_bytes([p[2 * c], p[2 * c + 1]]) as f32 * factor
                })
            })
            .collect();
        assert_eq!(
            decode_linear_bytes(&bytes, count, 3.75, &|| false).unwrap(),
            expected
        );
        let calls = std::sync::atomic::AtomicUsize::new(0);
        assert!(
            decode_linear_bytes(&bytes, count, 3.75, &|| calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                > 2)
            .is_err()
        );
    }
    #[test]
    fn sixteen_bit_conversion_preserves_headroom_and_sub_byte_steps() {
        let words = [32768_u16, 32769, 65535];
        let bytes: Vec<_> = words.into_iter().flat_map(u16::to_ne_bytes).collect();
        let rgb = decode_linear_bytes(&bytes, 1, 3., &|| false).unwrap()[0];
        assert!(rgb[0] > 1.49 && rgb[0] < 1.51);
        assert!(rgb[1] > rgb[0] && rgb[1] - rgb[0] < 1. / 255.);
        assert_eq!(rgb[2], 3.);
    }
    #[test]
    fn malformed_and_cancelled_linear_frames_are_rejected() {
        assert!(decode_linear_bytes(&[0; 6], 2, 1., &|| false).is_err());
        assert!(decode_linear_bytes(&[0; 6], 1, f64::NAN, &|| false).is_err());
        assert!(decode_linear_bytes(&[0; 6], 1, 1., &|| true).is_err());
    }
}
