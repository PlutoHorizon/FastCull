//! Per-user culling preferences, independent of photo metadata.
use eframe::egui;
use std::{io::Write, path::Path};

#[derive(Default)]
pub struct Settings {
    pub focus_on_unviewed: bool,
    error: Option<String>,
}

impl Settings {
    pub fn load() -> Self {
        Self::from_path(&Self::path())
    }

    fn path() -> std::path::PathBuf {
        std::env::var_os("FASTCULL_SETTINGS_FILE")
            .map(Into::into)
            .unwrap_or_else(|| fastcull::platform::data_directory().join("settings.txt"))
    }

    fn from_path(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        Self {
            focus_on_unviewed: text
                .lines()
                .any(|line| line.trim() == "focus_on_unviewed=true"),
            ..Default::default()
        }
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        writeln!(file, "focus_on_unviewed={}", self.focus_on_unviewed)?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }

    pub fn menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(300.);
        ui.set_max_width(340.);
        ui.strong("设置");
        ui.separator();
        crate::appearance::options(ui);
        ui.separator();
        ui.strong("放大浏览");
        if ui
            .checkbox(&mut self.focus_on_unviewed, "100% 切图时自动定位对焦点")
            .changed()
        {
            self.error = self.save(&Self::path()).err().map(|e| {
                log::warn!("culling preference: {e}");
                format!("本次已生效，设置保存失败：{e}")
            });
        }
        ui.weak("仅对本次尚未浏览的照片生效；再次查看时保留上次位置。");
        ui.weak("无可用对焦点时保留当前位置。设置自动保存。");
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_preference_defaults_off_and_survives_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings/settings.txt");
        let mut settings = Settings::from_path(&path);
        assert!(!settings.focus_on_unviewed);
        for enabled in [true, false, true] {
            settings.focus_on_unviewed = enabled;
            settings.save(&path).unwrap();
            assert_eq!(Settings::from_path(&path).focus_on_unviewed, enabled);
        }
        std::fs::write(&path, "focus_on_unviewed=invalid\nfuture_setting=true\n").unwrap();
        assert!(!Settings::from_path(&path).focus_on_unviewed);
    }
}
