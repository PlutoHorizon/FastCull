//! Small desktop differences shared by the standalone and Adobe entry points.
use std::path::{Path, PathBuf};

pub fn data_directory() -> PathBuf {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(target_os = "macos"))]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    base.unwrap_or_else(std::env::temp_dir).join("FastCull")
}

pub fn open_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    let program = "explorer.exe";
    #[cfg(target_os = "macos")]
    let program = "/usr/bin/open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program).arg(path).spawn()?;
    Ok(())
}

/// The system font for Latin text.
///
/// egui's bundled proportional face is a light grotesque that reads as
/// unfinished next to a system Chinese font. Each platform's own UI font is
/// used instead: the Latin and the Chinese glyphs then come from two faces
/// that were designed to sit together.
pub fn latin_font_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    return [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/HelveticaNeue.ttc",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    #[cfg(windows)]
    {
        let windows = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| "C:/Windows".into());
        return vec![
            windows.join("Fonts/segoeui.ttf"),
            windows.join("Fonts/segoeuib.ttf"),
            windows.join("Fonts/arial.ttf"),
        ];
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    vec![
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into(),
        "/usr/share/fonts/TTF/DejaVuSans.ttf".into(),
    ]
}

/// The system monospaced font, for numbers that have to line up.
pub fn mono_font_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    return [
        "/System/Library/Fonts/SFNSMono.ttf",
        "/System/Library/Fonts/Menlo.ttc",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    #[cfg(windows)]
    {
        let windows = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| "C:/Windows".into());
        return vec![
            windows.join("Fonts/consola.ttf"),
            windows.join("Fonts/cour.ttf"),
        ];
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    vec!["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf".into()]
}

pub fn cjk_font_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    return [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    #[cfg(windows)]
    {
        let windows = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| "C:/Windows".into());
        return vec![
            windows.join("Fonts/msyh.ttc"),
            windows.join("Fonts/msyhbd.ttc"),
            windows.join("Fonts/simhei.ttf"),
        ];
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    vec![
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into(),
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc".into(),
    ]
}
