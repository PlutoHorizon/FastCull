//! Lazy focus metadata and a single painter for culling, comparison and RAW.
use eframe::egui::{self, Color32, Rect, Stroke, Vec2};
use fastcull::photo_focus::{self, Info};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

#[derive(Default)]
pub struct Overlay {
    pub enabled: bool,
    cache: HashMap<PathBuf, State>,
    worker: Option<Worker>,
}

struct Worker {
    send: mpsc::Sender<PathBuf>,
    receive: mpsc::Receiver<(PathBuf, Result<Info, String>)>,
}
enum State {
    Loading,
    Ready(Info),
    Failed(String),
}
pub enum Lookup<'a> {
    Loading,
    Ready(&'a Info),
    Unavailable,
}

/// Image-relative pan that centers the visible AF points, using the exact paint UVs.
pub fn focus_anchor(
    info: &Info,
    size: Vec2,
    orientation: u16,
    crop: fastcull::crop::Crop,
) -> Option<Vec2> {
    let raw = crop.raw_uvs(size);
    let uvs = fastcull::crop::order(orientation).map(|i| raw[i]);
    let unit = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
    let mut sum = Vec2::ZERO;
    let mut count = 0;
    for point in info
        .areas
        .iter()
        .filter_map(|area| photo_focus::project(area.center, uvs, unit))
    {
        if point.is_finite() && unit.contains(point) {
            sum += point.to_vec2();
            count += 1;
        }
    }
    (count > 0).then(|| Vec2::splat(0.5) - sum / count as f32)
}

impl Overlay {
    #[cfg(test)]
    pub fn insert_fixture(&mut self, path: PathBuf, info: Info) {
        self.cache.insert(path, State::Ready(info));
    }
    pub fn control(&mut self, ui: &mut egui::Ui) {
        if ui
            .add(egui::Button::new("对焦点").selected(self.enabled))
            .on_hover_text("显示照片中记录的对焦点或对焦框（O）；随旋转、裁切、缩放和拖动对齐。相机未记录或格式不支持时会提示。")
            .clicked()
        {
            self.enabled = !self.enabled;
        }
    }
    pub fn shortcut(&mut self, ctx: &egui::Context) {
        if !ctx.wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::O))
        {
            self.enabled = !self.enabled;
        }
    }
    fn request(&mut self, path: &Path, ctx: &egui::Context) {
        if self.worker.is_none() {
            let (send, receive) = mpsc::channel::<PathBuf>();
            let (done, results) = mpsc::channel();
            let ctx = ctx.clone();
            thread::spawn(move || {
                while let Ok(path) = receive.recv() {
                    let result = photo_focus::read(&path).map_err(|e| format!("{e:#}"));
                    if done.send((path, result)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
            self.worker = Some(Worker {
                send,
                receive: results,
            });
        }
        if !self.cache.contains_key(path) {
            self.cache.insert(path.to_path_buf(), State::Loading);
            let _ = self.worker.as_ref().unwrap().send.send(path.to_path_buf());
        }
    }
    /// Metadata is also available when the visual overlay is switched off.
    pub fn lookup(&mut self, path: &Path, ctx: &egui::Context) -> Lookup<'_> {
        self.request(path, ctx);
        if let Some(worker) = &self.worker {
            while let Ok((path, result)) = worker.receive.try_recv() {
                self.cache.insert(
                    path,
                    match result {
                        Ok(info) => State::Ready(info),
                        Err(error) => State::Failed(error),
                    },
                );
            }
        }
        match self.cache.get(path) {
            Some(State::Ready(info)) => Lookup::Ready(info),
            Some(State::Failed(_)) => Lookup::Unavailable,
            _ => Lookup::Loading,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        painter: &egui::Painter,
        path: &Path,
        viewport: Rect,
        screen: Rect,
        source_size: Vec2,
        orientation: u16,
        crop: fastcull::crop::Crop,
    ) {
        if !self.enabled {
            return;
        }
        self.lookup(path, painter.ctx());
        let message = match self.cache.get(path) {
            Some(State::Ready(info)) if !info.areas.is_empty() => {
                let raw = crop.raw_uvs(source_size);
                let uvs = fastcull::crop::order(orientation).map(|i| raw[i]);
                let clip = viewport.intersect(screen);
                let pen = painter.with_clip_rect(clip);
                for area in &info.areas {
                    if let Some(size) = area.size {
                        let raw_rect = Rect::from_center_size(area.center, size);
                        let points: Option<Vec<_>> = fastcull::crop::corners(raw_rect)
                            .into_iter()
                            .map(|p| photo_focus::project(p, uvs, screen))
                            .collect();
                        if let Some(points) = points {
                            pen.add(egui::Shape::closed_line(
                                points.clone(),
                                Stroke::new(4., Color32::BLACK),
                            ));
                            pen.add(egui::Shape::closed_line(
                                points,
                                Stroke::new(2., Color32::LIGHT_GREEN),
                            ));
                        }
                    } else if let Some(center) = photo_focus::project(area.center, uvs, screen) {
                        // Fixed-size marker: no fabricated AF rectangle dimensions.
                        if clip.contains(center) {
                            let marker = Rect::from_center_size(center, Vec2::splat(16.));
                            pen.rect_stroke(
                                marker,
                                0.,
                                Stroke::new(4., Color32::BLACK),
                                egui::StrokeKind::Middle,
                            );
                            pen.rect_stroke(
                                marker,
                                0.,
                                Stroke::new(2., Color32::LIGHT_GREEN),
                                egui::StrokeKind::Middle,
                            );
                            pen.circle_filled(center, 2., Color32::LIGHT_GREEN);
                        }
                    }
                }
                format!("{} · {}", info.source, info.areas.len())
            }
            Some(State::Ready(_)) => "无可用对焦坐标（未记录或暂不支持）".into(),
            Some(State::Failed(error)) => {
                log::debug!("focus metadata {}: {error}", path.display());
                "无可用对焦坐标（未记录或暂不支持）".into()
            }
            _ => "正在读取对焦信息…".into(),
        };
        let pos = viewport.left_top() + Vec2::new(12., 34.);
        let galley = painter.layout_no_wrap(
            message,
            egui::FontId::proportional(13.),
            Color32::LIGHT_GREEN,
        );
        painter.rect_filled(
            Rect::from_min_size(pos - Vec2::splat(4.), galley.size() + Vec2::splat(8.)),
            3.,
            Color32::from_black_alpha(190),
        );
        painter.galley(pos, galley, Color32::LIGHT_GREEN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_focus_anchor_tracks_all_orientations_and_ignores_cropped_points() {
        let size = Vec2::new(6000., 4000.);
        let point = egui::pos2(0.75, 0.25);
        let info = Info {
            source: "fixture",
            areas: vec![photo_focus::Area {
                center: point,
                size: None,
            }],
        };
        let expected = [
            Vec2::new(-0.25, 0.25),
            Vec2::new(0.25, 0.25),
            Vec2::new(0.25, -0.25),
            Vec2::new(-0.25, -0.25),
            Vec2::new(0.25, -0.25),
            Vec2::new(-0.25, -0.25),
            Vec2::new(-0.25, 0.25),
            Vec2::new(0.25, 0.25),
        ];
        for orientation in 1..=8 {
            assert_eq!(
                focus_anchor(&info, size, orientation, Default::default()),
                Some(expected[orientation as usize - 1])
            );
        }
        let crop = fastcull::crop::Crop {
            enabled: true,
            left: 500_000,
            top: 0,
            right: 1_000_000,
            bottom: 500_000,
            ..Default::default()
        };
        assert_eq!(focus_anchor(&info, size, 1, crop), Some(Vec2::ZERO));
        let crop = fastcull::crop::Crop {
            enabled: true,
            right: 500_000,
            ..Default::default()
        };
        assert!(focus_anchor(&info, size, 1, crop).is_none());
        assert!(focus_anchor(&Info::default(), size, 1, Default::default()).is_none());
        let group = Info {
            source: "fixture",
            areas: [
                egui::pos2(0.6, 0.2),
                egui::pos2(0.8, 0.4),
                egui::pos2(0.2, 0.9),
            ]
            .map(|center| photo_focus::Area { center, size: None })
            .into(),
        };
        let crop = fastcull::crop::Crop {
            enabled: true,
            left: 500_000,
            ..Default::default()
        };
        let anchor = focus_anchor(&group, size, 1, crop).unwrap();
        assert!((anchor - Vec2::new(0.1, 0.2)).length() < 0.0001);
    }
    #[test]
    fn overlay_uses_image_transform_clips_cropped_points_and_leaves_source_untouched() {
        let ctx = egui::Context::default();
        let path = Path::new("fixture.ARW");
        let area = Rect::from_min_size(egui::pos2(0., 0.), Vec2::new(800., 600.));
        let screen = Rect::from_min_size(egui::pos2(100., 100.), Vec2::new(600., 400.));
        let mut overlay = Overlay {
            enabled: true,
            ..Default::default()
        };
        overlay.cache.insert(
            path.into(),
            State::Ready(Info {
                source: "fixture",
                areas: vec![photo_focus::Area {
                    center: egui::pos2(0.75, 0.5),
                    size: None,
                }],
            }),
        );
        for (crop, expected) in [
            (
                fastcull::crop::Crop::default(),
                Some(egui::pos2(400., 200.)),
            ),
            (
                fastcull::crop::Crop {
                    enabled: true,
                    right: 500_000,
                    ..Default::default()
                },
                None,
            ),
        ] {
            let result = ctx.run(Default::default(), |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                overlay.paint(
                    &painter,
                    path,
                    area,
                    screen,
                    Vec2::new(6000., 4000.),
                    8,
                    crop,
                );
            });
            let marker = result.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Circle(c) if c.fill == Color32::LIGHT_GREEN => Some(c.center),
                _ => None,
            });
            assert_eq!(marker, expected);
        }
        let State::Ready(info) = overlay.cache.get(path).unwrap() else {
            panic!("lost metadata")
        };
        assert_eq!(info.areas[0].center, egui::pos2(0.75, 0.5));
    }
    #[test]
    fn focus_shortcut_does_not_consume_text_entry() {
        let ctx = egui::Context::default();
        let mut overlay = Overlay::default();
        for typing in [true, false] {
            let id = egui::Id::new("search");
            if typing {
                ctx.memory_mut(|m| m.request_focus(id));
            } else {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::O,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    }],
                    ..Default::default()
                },
                |ctx| overlay.shortcut(ctx),
            );
            assert_eq!(overlay.enabled, !typing);
        }
    }
}
