//! Small, stamp-checked caches for parsed camera profiles. Never cache failures.
use anyhow::Result;
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

#[derive(PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}
fn stamp(path: &Path) -> Result<Stamp> {
    let metadata = fs::metadata(path)?;
    Ok(Stamp {
        path: path.to_owned(),
        len: metadata.len(),
        modified: metadata.modified()?,
    })
}
pub struct Cache<T>(Mutex<VecDeque<(Stamp, T)>>);
impl<T: Clone> Cache<T> {
    pub const fn new() -> Self {
        Self(Mutex::new(VecDeque::new()))
    }
    pub fn read(&self, path: &Path, load: impl FnOnce() -> Result<T>) -> Result<T> {
        let key = stamp(path)?;
        if let Some(value) = self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|(s, _)| *s == key)
            .map(|(_, v)| v.clone())
        {
            return Ok(value);
        }
        let value = load()?;
        if stamp(path)? == key {
            let mut cache = self.0.lock().unwrap();
            cache.retain(|(s, _)| s.path != key.path);
            if cache.len() >= 16 {
                cache.pop_front();
            }
            cache.push_back((key, value.clone()));
        }
        Ok(value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn reuses_parse_and_invalidates_replacements_and_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile");
        fs::write(&path, b"one").unwrap();
        let cache = Cache::new();
        let count = Cell::new(0);
        let read = || {
            cache.read(&path, || {
                count.set(count.get() + 1);
                Ok(fs::read(&path)?)
            })
        };
        assert_eq!(read().unwrap(), b"one");
        assert_eq!(read().unwrap(), b"one");
        assert_eq!(count.get(), 1);
        fs::write(&path, b"replacement").unwrap();
        assert_eq!(read().unwrap(), b"replacement");
        assert_eq!(count.get(), 2);
        let cache = Cache::<u32>::new();
        assert!(cache.read(&path, || anyhow::bail!("bad profile")).is_err());
        assert_eq!(cache.read(&path, || Ok(42)).unwrap(), 42);
    }
}
