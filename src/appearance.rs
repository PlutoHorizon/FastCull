//! Shared flat UI styles and a per-user appearance preference.
use eframe::egui::{self, Color32, Stroke, Theme, ThemePreference, Vec2};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone)]
struct PreferenceState {
    checked: Instant,
    disk: ThemePreference,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub panel: Color32,
    pub rail: Color32,
    pub canvas: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub border: Color32,
    pub hover: Color32,
    pub accent: Color32,
    pub selected: Color32,
    pub danger: Color32,
}
impl Palette {
    pub fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self {
                panel: Color32::from_rgb(255, 255, 255),
                rail: Color32::from_rgb(245, 246, 248),
                canvas: Color32::from_rgb(232, 234, 238),
                text: Color32::from_rgb(32, 39, 51),
                muted: Color32::from_rgb(100, 110, 126),
                border: Color32::from_rgb(224, 228, 234),
                hover: Color32::from_rgb(237, 240, 245),
                accent: Color32::from_rgb(42, 91, 197),
                selected: Color32::from_rgb(231, 238, 253),
                danger: Color32::from_rgb(184, 52, 60),
            },
            Theme::Dark => Self {
                panel: Color32::from_rgb(30, 33, 39),
                rail: Color32::from_rgb(25, 28, 33),
                canvas: Color32::from_rgb(17, 19, 23),
                text: Color32::from_rgb(229, 233, 240),
                muted: Color32::from_rgb(157, 166, 182),
                border: Color32::from_rgb(48, 53, 63),
                hover: Color32::from_rgb(43, 48, 58),
                accent: Color32::from_rgb(139, 177, 255),
                selected: Color32::from_rgb(39, 55, 83),
                danger: Color32::from_rgb(245, 137, 145),
            },
        }
    }
}
pub fn palette(ctx: &egui::Context) -> Palette {
    Palette::for_theme(ctx.theme())
}
fn settings_path() -> PathBuf {
    std::env::var_os("FASTCULL_APPEARANCE_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::platform::data_directory().join("appearance.txt"))
}
fn parse(value: &str) -> ThemePreference {
    match value.trim() {
        "light" => ThemePreference::Light,
        "dark" => ThemePreference::Dark,
        _ => ThemePreference::System,
    }
}
fn load(path: &Path) -> ThemePreference {
    std::fs::read_to_string(path).map_or(ThemePreference::System, |s| parse(&s))
}
fn save(path: &Path, preference: ThemePreference) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(match preference {
        ThemePreference::Light => b"light\n",
        ThemePreference::Dark => b"dark\n",
        ThemePreference::System => b"system\n",
    })?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub fn configure(ctx: &egui::Context) {
    for theme in [Theme::Light, Theme::Dark] {
        let p = Palette::for_theme(theme);
        let mut style = theme.default_style();
        let v = &mut style.visuals;
        v.panel_fill = p.panel;
        v.window_fill = p.panel;
        v.override_text_color = Some(p.text);
        v.weak_text_color = Some(p.muted);
        v.extreme_bg_color = p.rail;
        v.faint_bg_color = p.rail;
        v.code_bg_color = p.rail;
        v.hyperlink_color = p.accent;
        v.error_fg_color = p.danger;
        v.selection.bg_fill = p.selected;
        v.selection.stroke = Stroke::new(1., p.accent);
        v.window_stroke = Stroke::new(1., p.border);
        v.window_corner_radius = egui::CornerRadius::same(10);
        v.window_shadow = egui::epaint::Shadow::NONE;
        v.popup_shadow = egui::epaint::Shadow::NONE;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1., p.border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1., p.muted);
        for (widget, fill, foreground) in [
            (&mut v.widgets.inactive, p.rail, p.text),
            (&mut v.widgets.hovered, p.hover, p.text),
            (&mut v.widgets.active, p.selected, p.text),
            (&mut v.widgets.open, p.selected, p.text),
        ] {
            widget.weak_bg_fill = fill;
            widget.bg_fill = fill;
            widget.bg_stroke = Stroke::NONE;
            widget.fg_stroke = Stroke::new(1.2, foreground);
            widget.corner_radius = egui::CornerRadius::same(6);
            widget.expansion = 0.;
        }
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(19.));
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(13.));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.));
        style.spacing.item_spacing = Vec2::new(7., 7.);
        style.spacing.button_padding = Vec2::new(9., 5.);
        style.spacing.interact_size.y = 28.;
        style.spacing.indent = 14.;
        style.spacing.slider_rail_height = 3.;
        ctx.set_style_of(theme, style);
    }
    let preference = load(&settings_path());
    ctx.set_theme(preference);
    ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::new("fastcull-preference-state"),
            PreferenceState {
                checked: Instant::now(),
                disk: preference,
            },
        )
    });
}
pub fn sync_native_theme(ctx: &egui::Context) {
    // RAW editing runs in its own process. Reflect choices made in another
    // FastCull window without doing filesystem work on every rendered frame.
    let id = egui::Id::new("fastcull-preference-state");
    if let Some(mut state) = ctx.data(|d| d.get_temp::<PreferenceState>(id))
        && state.checked.elapsed() >= Duration::from_secs(1)
    {
        if let Ok(value) = std::fs::read_to_string(settings_path()) {
            let disk = parse(&value);
            if disk != state.disk {
                ctx.set_theme(disk);
                state.disk = disk;
            }
        }
        state.checked = Instant::now();
        ctx.data_mut(|d| d.insert_temp(id, state));
    }
    ctx.request_repaint_after(Duration::from_secs(1));
    let preference = ctx.options(|o| o.theme_preference);
    let id = egui::Id::new("fastcull-native-theme");
    if ctx.data(|d| d.get_temp::<ThemePreference>(id)) != Some(preference) {
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match preference {
            ThemePreference::System => egui::SystemTheme::SystemDefault,
            ThemePreference::Light => egui::SystemTheme::Light,
            ThemePreference::Dark => egui::SystemTheme::Dark,
        }));
        ctx.data_mut(|d| d.insert_temp(id, preference));
    }
}
pub fn control(ui: &mut egui::Ui) {
    let preference = ui.ctx().options(|o| o.theme_preference);
    let label = match preference {
        ThemePreference::System => "跟随系统",
        ThemePreference::Light => "浅色",
        ThemePreference::Dark => "深色",
    };
    ui.menu_button(format!("外观 · {label}"), |ui| {
        options(ui);
    })
    .response
    .on_hover_text("设置浅色、深色或跟随系统外观");
}

/// The same appearance choices can also be embedded in the culler's settings.
pub fn options(ui: &mut egui::Ui) {
    let mut preference = ui.ctx().options(|o| o.theme_preference);
    let before = preference;
    ui.set_min_width(208.);
    ui.label(egui::RichText::new("外观设置").strong());
    ui.weak("即时切换，自动保存");
    ui.add_space(5.);
    for (value, name) in [
        (ThemePreference::Light, "浅色模式"),
        (ThemePreference::Dark, "深色模式"),
        (ThemePreference::System, "跟随系统"),
    ] {
        if ui.selectable_value(&mut preference, value, name).clicked() {
            ui.close();
        }
    }
    let id = egui::Id::new("fastcull-appearance-error");
    if let Some(error) = ui.ctx().data(|d| d.get_temp::<String>(id)) {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if preference != before {
        ui.ctx().set_theme(preference);
        let result = save(&settings_path(), preference);
        if result.is_ok() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::new("fastcull-preference-state"),
                    PreferenceState {
                        checked: Instant::now(),
                        disk: preference,
                    },
                )
            });
        }
        let error = result.err().map(|e| {
            log::warn!("appearance preference: {e}");
            format!("本次已切换，设置保存失败：{e}")
        });
        let id = egui::Id::new("fastcull-appearance-error");
        ui.ctx().data_mut(|d| {
            d.remove::<String>(id);
            if let Some(error) = error {
                d.insert_temp(id, error);
            }
        });
        sync_native_theme(ui.ctx());
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preference_survives_replacement_and_invalid_settings_fall_back_to_system() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings/appearance.txt");
        assert_eq!(load(&path), ThemePreference::System);
        for preference in [
            ThemePreference::Light,
            ThemePreference::Dark,
            ThemePreference::System,
        ] {
            save(&path, preference).unwrap();
            assert_eq!(load(&path), preference);
        }
        std::fs::write(&path, "future-setting").unwrap();
        assert_eq!(load(&path), ThemePreference::System);
    }
    #[test]
    fn system_changes_switch_both_styles_while_explicit_choices_stay_fixed() {
        let ctx = egui::Context::default();
        configure(&ctx);
        ctx.set_theme(ThemePreference::System);
        for theme in [Theme::Light, Theme::Dark, Theme::Light] {
            let _ = ctx.run(
                egui::RawInput {
                    system_theme: Some(theme),
                    ..Default::default()
                },
                |ctx| {
                    assert_eq!(ctx.theme(), theme);
                    assert_eq!(
                        ctx.style().visuals.panel_fill,
                        Palette::for_theme(theme).panel
                    );
                },
            );
        }
        ctx.set_theme(ThemePreference::Light);
        let _ = ctx.run(
            egui::RawInput {
                system_theme: Some(Theme::Dark),
                ..Default::default()
            },
            |ctx| {
                assert_eq!(ctx.theme(), Theme::Light);
            },
        );
    }
}
