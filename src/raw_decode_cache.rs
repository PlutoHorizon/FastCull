//! One bounded, immutable camera-RGB decode. Profile/crop edits must not run
//! demosaicing again; WB, resolution and source changes remain separate keys.
use crate::native_raw::{DevelopSettings, LinearDeveloped};
use anyhow::{Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

const MAX_BYTES: usize = 512 * 1024 * 1024;
#[derive(PartialEq, Eq)]
struct Key {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
    edge: u32,
    temperature: Option<u64>,
    tint: u64,
}
impl Key {
    fn read(path: &Path, settings: &DevelopSettings) -> Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            path: path.to_owned(),
            len: metadata.len(),
            modified: metadata.modified()?,
            edge: settings.max_edge,
            temperature: settings.temperature.map(f64::to_bits),
            tint: if settings.temperature.is_some() {
                settings.tint.to_bits()
            } else {
                0
            },
        })
    }
}
pub struct Decoded {
    pub source: Arc<LinearDeveloped>,
    pub reused: bool,
}
pub struct Cache(Mutex<Option<(Key, Arc<LinearDeveloped>)>>);
impl Cache {
    pub fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }
    pub fn develop(
        &self,
        path: &Path,
        settings: &DevelopSettings,
        cancelled: &(dyn Fn() -> bool + Sync),
        decode: impl FnOnce() -> Result<LinearDeveloped>,
    ) -> Result<Decoded> {
        ensure!(!cancelled(), "已取消线性 RAW 缓存读取");
        let key = Key::read(path, settings)?;
        if let Some(decoded) = self
            .0
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
        {
            ensure!(!cancelled(), "已取消线性 RAW 缓存读取");
            return Ok(Decoded {
                source: decoded,
                reused: true,
            });
        }
        let decoded = Arc::new(decode()?);
        ensure!(!cancelled(), "已取消线性 RAW 显影");
        if decoded
            .rgb
            .len()
            .checked_mul(12)
            .is_some_and(|n| n <= MAX_BYTES)
            && Key::read(path, settings)? == key
        {
            *self.0.lock().unwrap() = Some((key, decoded.clone()));
        } else {
            // A large image must not pin the previous photo in memory.
            *self.0.lock().unwrap() = None;
        }
        Ok(Decoded {
            source: decoded,
            reused: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    fn frame() -> LinearDeveloped {
        LinearDeveloped {
            width: 1,
            height: 1,
            rgb: vec![[0.2, 0.3, 2.]],
            orientation: 1,
            temperature: Some(5500.),
            tint: Some(0.),
            camera_multipliers: [1.; 4],
            applied_mul: [1.; 4],
            camera_matrix: [[0.; 3]; 4],
            camera_to_srgb: [[0.; 4]; 3],
            baseline_exposure: 0.,
            make: "test".into(),
            model: "test".into(),
            open_ms: 1.,
            unpack_ms: 2.,
            process_ms: 3.,
        }
    }
    #[test]
    fn reuses_only_matching_decode_and_never_returns_mutated_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.arw");
        fs::write(&path, b"raw").unwrap();
        let cache = Cache::new();
        let calls = Cell::new(0);
        let mut settings = DevelopSettings {
            max_edge: 1600,
            temperature: Some(5500.),
            tint: 0.,
            exposure: 0.,
        };
        let read = |s: &DevelopSettings| {
            cache.develop(&path, s, &|| false, || {
                calls.set(calls.get() + 1);
                Ok(frame())
            })
        };
        let first = read(&settings).unwrap();
        let mut working = first.source.rgb.clone();
        working[0] = [9.; 3];
        let hit = read(&settings).unwrap();
        assert_eq!(hit.source.rgb, vec![[0.2, 0.3, 2.]]);
        assert!(hit.reused);
        assert!(
            Arc::ptr_eq(&first.source, &hit.source),
            "cache hit copied the camera raster"
        );
        assert_eq!(calls.get(), 1);
        settings.exposure = 2.;
        read(&settings).unwrap();
        assert_eq!(calls.get(), 1);
        settings.temperature = Some(6200.);
        read(&settings).unwrap();
        assert_eq!(calls.get(), 2);
        settings.max_edge = 0;
        read(&settings).unwrap();
        assert_eq!(calls.get(), 3);
        fs::write(&path, b"replacement").unwrap();
        read(&settings).unwrap();
        assert_eq!(calls.get(), 4);
        assert!(
            cache
                .develop(&path, &settings, &|| true, || Ok(frame()))
                .is_err()
        );
    }
}
