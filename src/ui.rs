use crate::cache::Pool;
use anyhow::Result;
use eframe::egui::{self, Color32, Key, Pos2, Rect, Stroke, TextureHandle, TextureOptions, Vec2};
#[cfg(test)]
use fastcull::xmp;
use fastcull::{
    arw::ArwInfo,
    imaging::Level,
    session::Session,
    state, theme,
    xmp::{Updates, Values},
};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};

const COLORS: [(&str, Color32); 5] = [
    ("Red", Color32::from_rgb(206, 75, 66)),
    ("Yellow", Color32::from_rgb(226, 185, 55)),
    ("Green", Color32::from_rgb(119, 171, 79)),
    ("Blue", Color32::from_rgb(90, 121, 165)),
    ("Purple", Color32::from_rgb(162, 127, 165)),
];

struct Texture {
    handle: TextureHandle,
    bytes: usize,
    used: u64,
}
struct TextureCache {
    pinned: Option<usize>,
    items: HashMap<usize, Texture>,
    budget: usize,
    limit: usize,
    clock: u64,
}
impl TextureCache {
    fn new(budget: usize, limit: usize) -> Self {
        Self {
            pinned: None,
            items: HashMap::new(),
            budget,
            limit,
            clock: 0,
        }
    }
    fn get(&mut self, i: usize) -> Option<TextureHandle> {
        self.clock += 1;
        let t = self.items.get_mut(&i)?;
        t.used = self.clock;
        Some(t.handle.clone())
    }
    fn insert(&mut self, i: usize, t: TextureHandle, protected: usize) {
        self.insert_with_priority(i, t, protected, false);
    }
    fn insert_near(&mut self, i: usize, t: TextureHandle, protected: usize) {
        self.insert_with_priority(i, t, protected, true);
    }
    fn insert_with_priority(&mut self, i: usize, t: TextureHandle, protected: usize, near: bool) {
        self.clock += 1;
        let bytes = t.size()[0] * t.size()[1] * 4;
        self.items.insert(
            i,
            Texture {
                handle: t,
                bytes,
                used: self.clock,
            },
        );
        while self.items.len() > self.limit
            || self.items.values().map(|t| t.bytes).sum::<usize>() > self.budget
        {
            let victim = self
                .items
                .iter()
                .filter(|(k, _)| **k != protected && Some(**k) != self.pinned)
                .max_by_key(|(k, t)| {
                    (
                        if near { k.abs_diff(protected) } else { 0 },
                        u64::MAX - t.used,
                    )
                })
                .map(|(k, _)| *k);
            if let Some(k) = victim {
                self.items.remove(&k);
            } else {
                break;
            }
        }
    }
}
#[derive(Default)]
struct Photo {
    develop_loaded: bool,
    crop_edited: bool,
    orientation_edited: bool,
    full_size: Option<Vec2>,
    exposure_base: Option<i32>,
    exposure_delta: f64,
    info: Option<ArwInfo>,
    values: Values,
    loaded: bool,
    image_errors: HashMap<Level, String>,
    metadata_error: Option<String>,
    capture_indexed: bool,
    capture_error: Option<String>,
    fit_edge_loaded: u32,
    ms: f64,
}
impl Photo {
    fn apply_raw_saved(&mut self, values: Values, updates: Updates) {
        if updates.exposure.is_some() {
            self.values.exposure = values.exposure;
            self.exposure_base = values.exposure;
            self.exposure_delta = 0.;
        }
        if updates.rating.is_some() {
            self.values.rating = values.rating;
        }
        if updates.white_balance.is_some() {
            self.values.white_balance = values.white_balance;
        }
        if updates.crop.is_some() {
            self.values.crop = values.crop;
            self.crop_edited = true;
        }
        if updates.orientation.is_some() {
            self.values.orientation = values.orientation;
            self.orientation_edited = true;
        }
        for control in fastcull::xmp::BasicControl::ALL {
            if updates.basic.get(control).is_some() {
                self.values.basic.set(control, values.basic.get(control));
            }
        }
        self.develop_loaded = true;
    }
    fn apply_develop_baseline(&mut self, baseline: crate::exposure::Baseline) {
        if self.exposure_base.is_none() {
            self.exposure_base = baseline.exposure;
            if let Some(e) = baseline.exposure {
                self.values.exposure = Some(e);
            }
        }
        if !self.crop_edited
            && let Some(crop) = baseline.crop
        {
            self.values.crop = Some(crop);
        }
        if !self.orientation_edited
            && let Some(o) = baseline.orientation
        {
            self.values.orientation = Some(o);
        }
        self.develop_loaded = true;
    }
    fn orientation(&self) -> u16 {
        self.values
            .orientation
            .unwrap_or_else(|| self.info.as_ref().map_or(1, |i| i.orientation))
    }
}
pub struct App {
    settings: crate::settings::Settings,
    viewed: std::collections::HashSet<usize>,
    pending_focus: Option<usize>,
    focus_overlay: crate::focus_overlay::Overlay,
    raw_windows: Arc<crate::raw_window::Windows>,
    raw_parent_lock: crate::focus::ParentLock,
    raw_smoke_started: bool,
    raw_smoke_at: Option<Instant>,
    raw_prewarmed: bool,
    raw_prewarm_at: Option<Instant>,
    crop_editor: Option<crate::crop_ui::Editor>,
    crop_requested: bool,
    baselines: crate::exposure::Baselines,
    exposure_preview: Option<Arc<crate::exposure_preview::Preview>>,
    compare: Option<Compare>,
    compare_sizes: [Vec2; 2],
    compare_ppp: f32,
    close_dialog: bool,
    allow_close: bool,
    paths: Arc<Vec<PathBuf>>,
    photos: Vec<Photo>,
    pool: Pool,
    full: TextureCache,
    fit: TextureCache,
    fit_edge: u32,
    thumbs: TextureCache,
    fast_thumbs: TextureCache,
    current: usize,
    visible: Vec<usize>,
    filters: fastcull::filtering::Filters,
    filter_editor: crate::filter_ui::Editor,
    folder_editor: crate::folder_ui::Editor,
    mover: Option<crate::move_jobs::Mover>,
    close_after_moves: bool,
    filter_dirty: bool,
    grid: bool,
    zoom: bool,
    // Fraction of the oriented image dimensions, independent of resolution/DPI.
    pan: Vec2,
    // Photo indices stay stable when files move; retain image-relative loupe anchors.
    loupe_positions: std::collections::HashMap<usize, Vec2>,
    canvas_size: Vec2,
    error: Option<String>,
    started: Instant,
    first_draw: bool,
    grid_scroll: bool,
    smoke: Option<Smoke>,
    prefetched: std::collections::HashSet<(usize, Level)>,
    direction: isize,
    film_wheel: f32,
    stress_remaining: usize,
    session: Option<Arc<Session>>,
    first_frame: bool,
    navigation_at: Instant,
    startup_focus: crate::focus::StartupFocus,
    zoom_started: Option<Instant>,
}
#[derive(Clone, Copy, Default)]
struct View {
    zoom: f32,
    pan: Vec2,
}
struct Compare {
    indices: [usize; 2],
    views: [View; 2],
    active: usize,
    linked: bool,
}
impl Compare {
    fn scale(&mut self, side: usize, target: f32, fit: [f32; 2]) {
        if target <= 0. {
            self.views[side].zoom = 0.;
            if self.linked {
                self.views[1 - side].zoom = 0.;
            }
            return;
        }
        let effective = self.views.map(|v| v.zoom);
        let effective = std::array::from_fn::<_, 2, _>(|i| {
            if effective[i] > 0. {
                effective[i]
            } else {
                fit[i].clamp(0.02, 8.)
            }
        });
        let mut lower: f32 = 0.;
        let mut upper: f32 = f32::INFINITY;
        for (i, scale) in effective.iter().enumerate() {
            if i == side || self.linked {
                lower = lower.max(0.02 / scale);
                upper = upper.min(8. / scale);
            }
        }
        let ratio = (target / effective[side]).clamp(lower, upper);
        for (i, scale) in effective.iter().enumerate() {
            if i == side || self.linked {
                // Pan is an image-relative anchor, so scaling must not replace it.
                self.views[i].zoom = scale * ratio;
            }
        }
    }
    fn translate(&mut self, side: usize, delta: Vec2, limits: [Vec2; 2]) {
        let mut lower = Vec2::splat(f32::NEG_INFINITY);
        let mut upper = Vec2::splat(f32::INFINITY);
        for (i, limit) in limits.iter().enumerate() {
            if i == side || self.linked {
                // Stop BOTH at the first edge. Independent clamping would lose alignment.
                lower = lower.max((-*limit - self.views[i].pan).min(Vec2::ZERO));
                upper = upper.min((*limit - self.views[i].pan).max(Vec2::ZERO));
            }
        }
        let delta = delta.max(lower).min(upper);
        for (i, v) in self.views.iter_mut().enumerate() {
            if i == side || self.linked {
                v.pan += delta;
            }
        }
    }
}
struct Smoke {
    dir: PathBuf,
    step: usize,
    waiting: bool,
    last: Instant,
    prepared: bool,
    cached_zoom_tested: bool,
    relative_link_tested: bool,
}
fn clockwise(o: u16) -> u16 {
    match o {
        1 => 6,
        2 => 7,
        3 => 8,
        4 => 5,
        5 => 2,
        6 => 3,
        7 => 4,
        8 => 1,
        _ => 1,
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        Self::new_session(cc, paths, None)
    }
    fn new_session(
        cc: &eframe::CreationContext<'_>,
        paths: Vec<PathBuf>,
        session: Option<Arc<Session>>,
    ) -> Self {
        configure(&cc.egui_ctx);
        Self::from_context(
            &cc.egui_ctx,
            crate::graphics::exposure_preview(cc),
            paths,
            session,
        )
    }
    fn from_context(
        ctx: &egui::Context,
        exposure_preview: Option<Arc<crate::exposure_preview::Preview>>,
        paths: Vec<PathBuf>,
        session: Option<Arc<Session>>,
    ) -> Self {
        let app_started = Instant::now();
        log::info!("startup app-builder begin");
        log::info!(
            "startup font-config elapsed_ms={:.1}",
            app_started.elapsed().as_secs_f64() * 1000.
        );
        let paths = Arc::new(paths);
        let baselines = crate::exposure::Baselines::new(
            session.as_ref().and_then(|s| s.exposure_file()),
            ctx.clone(),
        );
        let pool = Pool::new(paths.clone(), ctx.clone(), session.clone());
        log::info!(
            "startup worker-pool elapsed_ms={:.1}",
            app_started.elapsed().as_secs_f64() * 1000.
        );
        let count = paths.len();
        let mut folder_editor = crate::folder_ui::Editor::new(&paths, session.as_ref());
        folder_editor.open = false;
        let smoke = std::env::var_os("FASTCULL_SMOKE_DIR").map(|d| Smoke {
            dir: PathBuf::from(d),
            step: 0,
            waiting: false,
            last: Instant::now(),
            prepared: false,
            cached_zoom_tested: false,
            relative_link_tested: false,
        });
        let stress_remaining = if std::env::var_os("FASTCULL_STRESS").is_some() {
            180
        } else {
            0
        };
        Self {
            folder_editor,
            settings: crate::settings::Settings::load(),
            viewed: Default::default(),
            pending_focus: None,
            focus_overlay: Default::default(),
            raw_windows: crate::raw_window::Windows::new(session.clone()),
            raw_parent_lock: Default::default(),
            raw_smoke_started: false,
            raw_smoke_at: None,
            raw_prewarmed: false,
            raw_prewarm_at: None,
            crop_editor: None,
            crop_requested: false,
            mover: None,
            close_after_moves: false,
            baselines,
            exposure_preview,
            compare: None,
            compare_sizes: [Vec2::ZERO; 2],
            compare_ppp: 1.,
            close_dialog: false,
            allow_close: false,
            paths,
            photos: (0..count)
                .map(|i| Photo {
                    values: session
                        .as_ref()
                        .map_or_else(Values::default, |s| s.initial_values(i)),
                    ..Default::default()
                })
                .collect(),
            pool,
            full: TextureCache::new(300 * 1024 * 1024, 4),
            fit: TextureCache::new(96 * 1024 * 1024, 8),
            fit_edge: 2560,
            thumbs: TextureCache::new(64 * 1024 * 1024, 4096),
            // Camera thumbnails are ~76 KB each; this only ever needs to cover
            // the cells on screen until the 200 px previews arrive.
            fast_thumbs: TextureCache::new(8 * 1024 * 1024, 1024),
            current: 0,
            visible: (0..count).collect(),
            filters: Default::default(),
            filter_editor: crate::filter_ui::Editor::opened(),
            filter_dirty: false,
            grid: false,
            zoom: false,
            pan: Vec2::ZERO,
            loupe_positions: Default::default(),
            canvas_size: Vec2::ZERO,
            error: None,
            started: Instant::now(),
            first_draw: false,
            grid_scroll: false,
            smoke,
            prefetched: Default::default(),
            direction: 1,
            film_wheel: 0.,
            stress_remaining,
            session,
            first_frame: false,
            navigation_at: Instant::now(),
            startup_focus: crate::focus::StartupFocus::new(),
            zoom_started: None,
        }
    }
    fn enable_bridge_moves(&mut self, ctx: &egui::Context, directory: PathBuf) {
        self.folder_editor.direct = true;
        self.mover = Some(crate::move_jobs::Mover::new(
            self.pool.locations(),
            ctx.clone(),
            directory,
        ));
    }
    fn moving(&self, index: usize) -> bool {
        self.mover
            .as_ref()
            .is_some_and(|m| m.pending.contains(&index))
    }
    fn submit_moves(&mut self) {
        let requests = self.folder_editor.take_requests();
        let has_requests = !requests.is_empty();
        for request in requests {
            let result = (|| -> Result<()> {
                anyhow::ensure!(!self.close_after_moves, "已请求完成移动后关闭");
                let mover = self
                    .mover
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("实际移动仅用于 Bridge 或独立模式"))?;
                anyhow::ensure!(
                    request.indices.iter().all(|i| !mover.pending.contains(i)),
                    "所选照片已在移动队列中"
                );
                let items =
                    fastcull::relocating::prepare(&self.paths, &request.indices, &request.target)?;
                mover.enqueue(items)?;
                Ok(())
            })();
            match result {
                Ok(()) => {
                    self.folder_editor.last_target = Some(request.target);
                    self.folder_editor.photos.clear();
                    self.error = None;
                    if request.advance && request.indices.contains(&self.current) {
                        let position = self
                            .visible
                            .iter()
                            .position(|&i| i == self.current)
                            .unwrap_or(0);
                        let next = self
                            .visible
                            .iter()
                            .skip(position + 1)
                            .chain(self.visible.iter().take(position))
                            .find(|&&i| {
                                !self.moving(i)
                                    && self
                                        .compare
                                        .as_ref()
                                        .is_none_or(|c| c.indices[1 - c.active] != i)
                            })
                            .copied();
                        if let Some(i) = next {
                            self.go(i);
                            self.folder_editor
                                .select(i, &self.visible, egui::Modifiers::NONE);
                        }
                    }
                }
                Err(e) => {
                    self.error = Some(format!("无法移动：{e:#}"));
                }
            }
        }
        if let Some(mover) = &self.mover {
            self.folder_editor.move_busy = mover.pending.len();
            if has_requests {
                self.folder_editor.queued.clone_from(&mover.pending);
            }
        }
    }
    fn receive_moves(&mut self, ctx: &egui::Context) {
        let mut changed_paths = Vec::new();
        let events = self.mover.as_mut().map(|m| m.take()).unwrap_or_default();
        let has_events = !events.is_empty();
        for event in events {
            match event {
                crate::move_jobs::Event::Photo { index, path } => {
                    Arc::make_mut(&mut self.paths)[index] = path.clone();
                    changed_paths.push((index, path));
                    self.filter_dirty = true;
                }
                crate::move_jobs::Event::Finished { error: Some(e), .. } => {
                    log::error!("Bridge move: {e}");
                    self.error = Some(format!("移动未全部完成：{e}"));
                    self.close_after_moves = false;
                }
                _ => {}
            }
        }
        self.folder_editor.relocated(changed_paths);
        if let Some(mover) = &self.mover {
            self.folder_editor.move_busy = mover.pending.len();
            if has_events {
                self.folder_editor.queued.clone_from(&mover.pending);
            }
            self.folder_editor.move_status.clone_from(&mover.status);
        }
        if self.close_after_moves && self.mover.as_ref().is_none_or(|m| m.jobs == 0) {
            self.close_after_moves = false;
            self.finish_close(ctx, false);
        }
    }
    fn go(&mut self, i: usize) {
        if self.crop_editor.is_some() {
            return;
        }
        self.crop_requested = false;
        if !self.visible.contains(&i) {
            return;
        }
        let i = i.min(self.paths.len() - 1);
        if i == self.current {
            return;
        }
        if let Some(c) = &self.compare
            && c.indices[1 - c.active] == i
        {
            self.activate_compare(1 - c.active);
            return;
        }
        if let Some(c) = &mut self.compare {
            if self.settings.focus_on_unviewed && c.views[c.active].zoom > 0. {
                self.loupe_positions
                    .insert(self.current, c.views[c.active].pan);
                if let Some(pan) = self.loupe_positions.get(&i) {
                    c.views[c.active].pan = *pan;
                }
            }
            c.indices[c.active] = i;
        }
        if i != self.current {
            if !self.grid {
                self.viewed.insert(self.current);
            }
            let at_100 = self
                .compare
                .as_ref()
                .map_or(self.zoom, |c| (c.views[c.active].zoom - 1.).abs() < 0.0001);
            self.pending_focus = (self.settings.focus_on_unviewed
                && at_100
                && !self.grid
                && !self.viewed.contains(&i)
                && !self.loupe_positions.contains_key(&i))
            .then_some(i);
            if self.compare.is_none() && self.zoom {
                self.loupe_positions.insert(self.current, self.pan);
                if let Some(pan) = self.loupe_positions.get(&i) {
                    self.pan = *pan;
                }
            }
            self.direction = if i > self.current { 1 } else { -1 };
            self.current = i;
            self.navigation_at = Instant::now();
            self.pool.navigate(i);
            self.prefetched.clear();
            if !self.zoom {
                self.pan = Vec2::ZERO;
            }
            self.grid_scroll = true;
            self.zoom_started = self.zoom.then(Instant::now);
        }
    }
    fn next(&mut self, d: isize) {
        if let Some(i) = fastcull::filtering::step_visible(&self.visible, self.current, d) {
            if self
                .compare
                .as_ref()
                .is_some_and(|c| c.indices[1 - c.active] == i)
            {
                if let Some(j) = fastcull::filtering::step_visible(&self.visible, i, d) {
                    self.folder_editor
                        .select(j, &self.visible, egui::Modifiers::NONE);
                    self.go(j);
                }
                return;
            }
            self.folder_editor
                .select(i, &self.visible, egui::Modifiers::NONE);
            self.go(i);
        }
    }
    fn refilter(&mut self) {
        self.filter_editor.invalidate();
        let visible: Vec<_> = self
            .photos
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                (self.folder_editor.allows(i)
                    && self.filters.matches(
                        &fastcull::filtering::Row {
                            path: &self.paths[i],
                            values: &p.values,
                            capture: p.info.as_ref().map(|v| &v.capture),
                            indexed: p.capture_indexed || p.info.is_some(),
                        },
                        None,
                    ))
                .then_some(i)
            })
            .collect();
        let changed = visible != self.visible;
        self.visible = visible;
        if self
            .compare
            .as_ref()
            .is_some_and(|c| c.indices.iter().any(|i| !self.visible.contains(i)))
        {
            self.compare = None;
        }
        if let Some(i) = fastcull::filtering::reconcile_visible(&self.visible, self.current) {
            self.go(i);
        }
        self.filter_dirty = false;
        self.grid_scroll |= changed;
    }
    fn set_zoom(&mut self, zoom: bool) {
        self.pending_focus = None;
        if let Some(c) = &self.compare {
            self.zoom_compare(c.active, if zoom { 1. } else { 0. });
        }
        self.zoom = zoom;
        self.pan = Vec2::ZERO;
        self.zoom_started = zoom.then(Instant::now);
        if zoom {
            log::info!(
                "100% requested image={} cached={}",
                self.current,
                self.full.items.contains_key(&self.current)
            );
        }
    }
    fn save(&mut self, u: Updates) {
        if self.visible.is_empty() || !self.photos[self.current].loaded || self.moving(self.current)
        {
            return;
        }
        let saved_updates = u.clone();
        match self.pool.write_xmp(self.current, &u) {
            Ok(_) => {
                self.photos[self.current].crop_edited |= u.crop.is_some();
                self.photos[self.current].orientation_edited |= u.orientation.is_some();
                let v = &mut self.photos[self.current].values;
                if let Some(r) = u.rating {
                    v.rating = Some(r);
                }
                if let Some(l) = u.label {
                    v.label = Some(l);
                }
                if let Some(f) = u.flag {
                    v.flag = Some(f);
                }
                if let Some(o) = u.orientation {
                    v.orientation = Some(o);
                }
                if let Some(e) = u.exposure {
                    v.exposure = Some(e);
                }
                if let Some(crop) = u.crop {
                    v.crop = Some(crop);
                }
                self.error = None;
                self.filter_dirty = true;
                if let Some(session) = &self.session
                    && let Err(e) = session.record(self.current, &saved_updates)
                {
                    self.error = Some(format!(
                        "旁车已保存，但会话同步记录保存失败：{e:#}。请重试保存，详情见日志。"
                    ));
                    log::error!("session record: {e:#}");
                }
            }
            Err(e) => {
                let msg = format!("无法保存 {}：{e:#}", self.paths[self.current].display());
                log::error!("{msg}");
                self.error = Some(msg);
            }
        }
    }
    fn rating(&mut self, r: u8) {
        self.save(Updates {
            rating: Some(r),
            ..Default::default()
        });
    }
    fn flag(&mut self, f: &str) {
        self.save(Updates {
            flag: Some(f.to_owned()),
            ..Default::default()
        });
    }
    fn label(&mut self, l: &str) {
        let value = if self.photos[self.current]
            .values
            .label
            .as_deref()
            .is_some_and(|v| v.eq_ignore_ascii_case(l))
        {
            ""
        } else {
            l
        };
        self.save(Updates {
            label: Some(value.to_owned()),
            ..Default::default()
        });
    }
    fn rotate(&mut self, d: i32) {
        self.pending_focus = None;
        if self.develop_pending(self.current) {
            self.error = Some("正在读取 Lightroom 裁切和旋转，请稍后再试。".into());
            return;
        }
        let p = &self.photos[self.current];
        if p.info.is_some() {
            let mut o = p.orientation();
            for _ in 0..d.rem_euclid(4) {
                o = clockwise(o);
            }
            self.save(Updates {
                orientation: Some(o),
                ..Default::default()
            });
            if self.error.is_none() {
                self.pan = Vec2::ZERO;
            }
        }
    }
    fn keyboard(&mut self, ctx: &egui::Context) {
        if self.crop_editor.is_some() {
            if ctx.wants_keyboard_input() || egui::Popup::is_any_open(ctx) {
                return;
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                self.crop_editor = None;
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                self.apply_crop();
            } else if ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
                    Key::Z,
                )
            }) {
                self.crop_editor.as_mut().unwrap().redo();
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Z)) {
                self.crop_editor.as_mut().unwrap().undo();
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Y)) {
                self.crop_editor.as_mut().unwrap().redo();
            }
            return;
        }
        if self.close_dialog || egui::Popup::is_any_open(ctx) {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F)) {
            self.filter_editor.open = !self.filter_editor.open;
            if self.filter_editor.open {
                self.folder_editor.open = false;
            }
        }
        if self.filter_editor.editing(ctx) || self.folder_editor.editing(ctx) {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::A)) {
            self.folder_editor.photos = self.visible.iter().copied().collect();
            return;
        }
        // Preserve physical event order, including repeated keys in one frame.
        // Otherwise "5, right" grades the next photo instead of the original.
        let keys = ctx.input_mut(|input| {
            let mut keys = Vec::new();
            input.events.retain(|event| {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                    && *modifiers == egui::Modifiers::NONE
                    && matches!(
                        key,
                        Key::Escape
                            | Key::Q
                            | Key::G
                            | Key::C
                            | Key::D
                            | Key::E
                            | Key::Enter
                            | Key::F
                            | Key::R
                            | Key::Home
                            | Key::End
                            | Key::PageDown
                            | Key::Space
                            | Key::PageUp
                            | Key::ArrowRight
                            | Key::ArrowLeft
                            | Key::ArrowDown
                            | Key::ArrowUp
                            | Key::P
                            | Key::X
                            | Key::U
                            | Key::Num0
                            | Key::Num1
                            | Key::Num2
                            | Key::Num3
                            | Key::Num4
                            | Key::Num5
                            | Key::Num6
                            | Key::Num7
                            | Key::Num8
                            | Key::Num9
                            | Key::OpenBracket
                            | Key::CloseBracket
                            | Key::L
                            | Key::M
                            | Key::T
                    )
                {
                    keys.push(*key);
                    false
                } else {
                    true
                }
            });
            keys
        });
        for key in keys {
            if matches!(key, Key::Escape | Key::Q) {
                self.close_dialog = true;
                break;
            }
            if self.visible.is_empty() {
                continue;
            }
            match key {
                Key::G => {
                    self.grid = true;
                    self.grid_scroll = true;
                }
                Key::D | Key::E | Key::Enter => {
                    self.grid = false;
                    self.compare = None;
                }
                Key::C => self.enter_compare(),
                Key::F => self.set_zoom(!self.zoom),
                Key::R => self.retry_current(),
                Key::T => self.begin_crop(),
                Key::M if self.folder_editor.direct => {
                    self.folder_editor.repeat_move(self.current, &self.visible);
                    self.submit_moves();
                }
                Key::Home => {
                    if let Some(&i) = self.visible.first() {
                        self.go(i);
                    }
                }
                Key::End => {
                    if let Some(&i) = self.visible.last() {
                        self.go(i);
                    }
                }
                Key::PageDown | Key::Space | Key::ArrowRight => self.next(1),
                Key::PageUp | Key::ArrowLeft => self.next(-1),
                Key::ArrowDown if self.grid => self.next(6),
                Key::ArrowUp if self.grid => self.next(-6),
                Key::ArrowDown if self.zoom => {
                    self.move_view(Vec2::new(0., -100.), ctx.pixels_per_point())
                }
                Key::ArrowUp if self.zoom => {
                    self.move_view(Vec2::new(0., 100.), ctx.pixels_per_point())
                }
                Key::P => self.flag("pick"),
                Key::X => self.flag("reject"),
                Key::U => self.flag(""),
                Key::Num0 => self.rating(0),
                Key::Num1 => self.rating(1),
                Key::Num2 => self.rating(2),
                Key::Num3 => self.rating(3),
                Key::Num4 => self.rating(4),
                Key::Num5 => self.rating(5),
                Key::Num6 => self.label("Red"),
                Key::Num7 => self.label("Yellow"),
                Key::Num8 => self.label("Green"),
                Key::Num9 => self.label("Blue"),
                Key::OpenBracket => self.rotate(-1),
                Key::CloseBracket => self.rotate(1),
                Key::L => {
                    let _ = fastcull::platform::open_directory(&crate::logger::directory());
                }
                _ => {}
            }
        }
    }
    fn move_view(&mut self, delta: Vec2, ppp: f32) {
        // A late metadata response must never override the user's own pan.
        self.pending_focus = None;
        let Some(c) = &self.compare else {
            if let Some(size) = self.native_size(self.current) {
                let shown = self.displayed_size(self.current, size) / ppp;
                let limits = (shown - self.canvas_size).max(Vec2::ZERO) * 0.5 / shown;
                self.pan = (self.pan + delta / shown).clamp(-limits, limits);
            }
            return;
        };
        let side = c.active;
        let geometry = self.compare_geometry(ppp);
        let Some(shown) = geometry[side] else {
            return;
        };
        if c.linked && geometry[1 - side].is_none() {
            return;
        }
        let limits = std::array::from_fn(|i| {
            geometry[i].map_or(Vec2::ZERO, |shown| {
                ((shown - self.compare_sizes[i]).max(Vec2::ZERO) * 0.5) / shown
            })
        });
        self.compare
            .as_mut()
            .unwrap()
            .translate(side, delta / shown, limits);
    }
    fn apply_auto_focus(&mut self, i: usize, viewport: Vec2, ctx: &egui::Context) {
        if self.pending_focus != Some(i) {
            return;
        }
        let side = self
            .compare
            .as_ref()
            .and_then(|c| c.indices.iter().position(|j| *j == i));
        let at_100 = side.map_or(self.zoom, |side| {
            (self.compare.as_ref().unwrap().views[side].zoom - 1.).abs() < 0.0001
        });
        if !self.settings.focus_on_unviewed || !at_100 || self.grid {
            self.pending_focus = None;
            return;
        }
        // Start metadata I/O independently of the overlay toggle.
        self.focus_overlay.lookup(&self.paths[i], ctx);
        let photo = &self.photos[i];
        if !photo.loaded || photo.info.is_none() || self.develop_pending(i) {
            return;
        }
        let Some(size) = self.native_size(i) else {
            return;
        };
        let crop = photo.values.crop.unwrap_or_default();
        let orientation = photo.orientation();
        let info = match self.focus_overlay.lookup(&self.paths[i], ctx) {
            crate::focus_overlay::Lookup::Loading => return,
            crate::focus_overlay::Lookup::Unavailable => {
                self.pending_focus = None;
                return;
            }
            crate::focus_overlay::Lookup::Ready(info) => info,
        };
        let anchor = crate::focus_overlay::focus_anchor(info, size, orientation, crop);
        self.pending_focus = None;
        if let Some(anchor) = anchor {
            let shown = self.displayed_size(i, size) / ctx.pixels_per_point();
            let limits = (shown - viewport).max(Vec2::ZERO) * 0.5 / shown;
            let pan = anchor.clamp(-limits, limits);
            if let Some(side) = side {
                // Replacing a candidate must leave the reference photo where the user put it.
                self.compare.as_mut().unwrap().views[side].pan = pan;
            } else {
                self.pan = pan;
            }
        }
    }
    fn compare_geometry(&self, ppp: f32) -> [Option<Vec2>; 2] {
        let c = self.compare.as_ref().unwrap();
        std::array::from_fn(|side| {
            let i = c.indices[side];
            let size = self.displayed_size(i, self.native_size(i)?);
            (c.views[side].zoom > 0.).then_some(size * c.views[side].zoom / ppp)
        })
    }
    fn zoom_compare(&mut self, side: usize, target: f32) {
        self.pending_focus = None;
        let c = self.compare.as_ref().unwrap();
        let fit = std::array::from_fn(|side| {
            let i = c.indices[side];
            self.native_size(i).map_or(1., |size| {
                let size = self.displayed_size(i, size);
                let area = self.compare_sizes[side];
                if area.x <= 0. || area.y <= 0. {
                    return 1.;
                }
                (area.x / size.x).min(area.y / size.y) * self.compare_ppp
            })
        });
        self.compare.as_mut().unwrap().scale(side, target, fit);
        self.zoom = self.compare.as_ref().unwrap().views[side].zoom > 0.;
    }
    fn develop_pending(&self, i: usize) -> bool {
        !self.photos[i].develop_loaded
            && self
                .session
                .as_ref()
                .is_some_and(|s| s.exposure_file().is_some())
    }
    fn receive_develop(&mut self) {
        for i in 0..self.photos.len() {
            let photo = &mut self.photos[i];
            if !photo.loaded || (i == self.current && self.crop_editor.is_some()) {
                continue;
            }
            if self.session.is_none() {
                photo
                    .exposure_base
                    .get_or_insert(photo.values.exposure.unwrap_or(0));
            } else if !photo.develop_loaded
                && let Some(baseline) = self.baselines.get(&self.paths[i])
            {
                photo.apply_develop_baseline(baseline);
            }
        }
    }
    fn receive_metadata(&mut self) {
        for message in self.pool.take_metadata() {
            self.filter_dirty = true;
            let photo = &mut self.photos[message.index];
            match message.result {
                Ok(values) => {
                    if !photo.loaded {
                        photo.values = values;
                        photo.develop_loaded = false;
                    }
                    photo.loaded = true;
                    photo.metadata_error = None;
                }
                Err(e) => {
                    log::error!("metadata {}: {e}", self.paths[message.index].display());
                    photo.metadata_error = Some(e);
                    photo.loaded = false;
                }
            }
        }
    }
    fn receive_index(&mut self) {
        for message in self.pool.take_indexed() {
            let photo = &mut self.photos[message.index];
            photo.capture_indexed = true;
            match message.result {
                Ok(info) => {
                    photo.info = Some(info);
                    photo.capture_error = None;
                }
                Err(error) => photo.capture_error = Some(error),
            }
            self.filter_dirty = true;
        }
    }
    fn receive(&mut self, ctx: &egui::Context) {
        for (i, size) in self.pool.take_full_sizes() {
            self.photos[i].full_size = Some(Vec2::new(size[0] as f32, size[1] as f32));
        }
        // Drain small parallel previews in batches, with a short UI time budget.
        // Full-size uploads still stop the batch after one texture.
        let uploads_started = Instant::now();
        let mut full_indices = Vec::with_capacity(2);
        if let Some(c) = &self.compare {
            for side in 0..2 {
                if c.views[side].zoom > 0. {
                    full_indices.push(c.indices[side]);
                }
            }
        } else if self.zoom || self.crop_requested || self.crop_editor.is_some() {
            full_indices.push(self.current);
        }
        for slot in 0..16 {
            if slot > 0 && uploads_started.elapsed() >= std::time::Duration::from_millis(5) {
                ctx.request_repaint();
                break;
            }
            let Some(message) = self.pool.take_decoded_for(&full_indices) else {
                break;
            };
            let i = message.task.index;
            if let Level::Fit(edge) = message.task.level
                && self.fit.items.contains_key(&i)
                && self.photos[i].fit_edge_loaded > edge
            {
                continue;
            }
            match message.result {
                Ok((info, p)) => {
                    let photo = &mut self.photos[i];
                    self.filter_dirty |= photo.info.is_none();
                    photo.info = Some(info);
                    photo.ms = message.ms;
                    photo.image_errors.remove(&message.task.level);
                    let t = ctx.load_texture(
                        format!("{}-{:?}", i, message.task.level),
                        p,
                        TextureOptions::LINEAR,
                    );
                    if message.task.level == Level::Full {
                        self.photos[i].full_size = Some(t.size_vec2());
                        self.full.insert_near(i, t, self.current);
                        ctx.request_repaint();
                        break;
                    } else if let Level::Fit(edge) = message.task.level {
                        self.photos[i].fit_edge_loaded = edge;
                        self.fit.insert_near(i, t, self.current);
                    } else if message.task.level == Level::FastThumb {
                        self.fast_thumbs.insert(i, t, self.current);
                    } else {
                        self.thumbs.insert(i, t, self.current);
                    }
                }
                Err(e) => {
                    log::error!("decode {}: {e}", self.paths[i].display());
                    self.photos[i].image_errors.insert(message.task.level, e);
                }
            }
        }
    }
    fn request(&mut self, i: usize, level: Level) {
        if self.photos[i].image_errors.contains_key(&level) {
            return;
        }
        let cached = match level {
            Level::Full => self.full.items.contains_key(&i),
            Level::Fit(edge) => {
                self.full.items.contains_key(&i)
                    || (self.fit.items.contains_key(&i) && self.photos[i].fit_edge_loaded >= edge)
            }
            Level::FastThumb => self.fast_thumbs.items.contains_key(&i),
            Level::Thumb => self.thumbs.items.contains_key(&i),
        };
        if !cached {
            self.pool.request(i, level);
        }
    }
    fn fast_thumb_available(&self, i: usize) -> bool {
        self.photos[i].info.as_ref().is_some_and(|info| {
            info.bitmap_thumbnail.is_some()
                || (info.thumbnail.len > 0 && info.thumbnail.offset != info.preview.offset)
        })
    }
    fn retry_current(&mut self) {
        self.photos[self.current].image_errors.clear();
        self.photos[self.current].metadata_error = None;
        self.photos[self.current].loaded = false;
        self.pool.retry_metadata(self.current);
        self.pool.cancel(self.current);
        self.pool.navigate(self.current);
        self.prefetched.clear();
        self.error = None;
    }
    fn preview_for(&self, i: usize) -> Option<(Arc<crate::exposure_preview::Preview>, f32)> {
        let p = &self.photos[i];
        p.exposure_base?;
        let delta = p.values.exposure?;
        let preview = self.exposure_preview.as_ref()?.clone();
        (delta != 0).then_some((preview, delta as f32 / 1_000_000.))
    }
    fn finish_close(&mut self, ctx: &egui::Context, sync: bool) {
        if self.mover.as_ref().is_some_and(|m| m.jobs > 0) {
            self.close_after_moves = true;
            self.close_dialog = false;
            self.folder_editor.move_status = "完成移动后自动关闭".into();
            return;
        }
        let result = (|| -> Result<()> {
            if sync || !self.folder_editor.blocked {
                self.folder_editor.persist()?;
            }
            if let Some(s) = &self.session {
                s.record(self.current, &Updates::default())?;
                s.sync_on_exit
                    .store(sync, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.allow_close = true;
                self.close_dialog = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.error = Some(format!("会话保存失败，请重试：{e:#}")),
        }
    }
    fn activate_compare(&mut self, side: usize) {
        self.pending_focus = None;
        let c = self.compare.as_mut().unwrap();
        if c.active == side && self.current == c.indices[side] {
            return;
        }
        c.active = side;
        self.current = c.indices[side];
        self.navigation_at = Instant::now();
        self.zoom = c.views[side].zoom > 0.;
        self.pool.navigate(self.current);
        self.prefetched.clear();
    }
    fn adjust_exposure(&mut self, thirds: i32) {
        let p = &self.photos[self.current];
        let Some(base) = p.exposure_base else {
            return;
        };
        let delta = p.exposure_delta + thirds as f64 / 3.;
        let target =
            ((base as f64 / 1_000_000. + delta).clamp(-5., 5.) * 1_000_000.).round() as i32;
        self.save(Updates {
            exposure: Some(target),
            ..Default::default()
        });
        if self.photos[self.current].values.exposure == Some(target) {
            self.photos[self.current].exposure_delta = if target.abs() == 5_000_000 {
                (target - base) as f64 / 1_000_000.
            } else {
                delta
            };
        }
    }
    fn compare_canvas(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.compare_ppp = ctx.pixels_per_point();
        ui.columns(2, |columns| {
            for (side, ui) in columns.iter_mut().enumerate() {
                let i = self.compare.as_ref().unwrap().indices[side];
                let active = self.compare.as_ref().unwrap().active == side;
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(
                            active,
                            if side == 0 {
                                "A · 参考"
                            } else {
                                "B · 候选"
                            },
                        )
                        .on_hover_text("点击图片切换适配 / 100%；滚轮缩放，拖动平移")
                        .clicked()
                    {
                        self.activate_compare(side);
                    }
                    ui.add(
                        egui::Label::new(
                            self.paths[i]
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy(),
                        )
                        .truncate(),
                    );
                });
                ui.horizontal(|ui| {
                    for (label, zoom) in [("适配", 0.), ("100%", 1.)] {
                        let selected =
                            (self.compare.as_ref().unwrap().views[side].zoom - zoom).abs() < 0.0001;
                        if ui
                            .add(egui::Button::new(label).small().selected(selected))
                            .clicked()
                        {
                            self.activate_compare(side);
                            self.zoom_compare(side, zoom);
                        }
                    }
                    let zoom = self.compare.as_ref().unwrap().views[side].zoom;
                    if zoom > 0. && (zoom - 1.).abs() > 0.0001 {
                        ui.label(format!("{:.0}%", zoom * 100.));
                    }
                });
                let (area, response) =
                    ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
                // The photo keeps a margin inside the pane. Everything that
                // reads the pane size - the fit zoom and the pan limits - uses
                // the same area, so the linked zoom and pan stay in step.
                let photo_area = area.shrink(theme::SP_M);
                self.compare_sizes[side] = photo_area.size();
                self.fit_edge = preview_edge(area.size(), ctx.pixels_per_point());
                let painter = ui.painter_at(area);
                painter.rect_filled(area, 0., theme::canvas());
                self.request(i, Level::Fit(self.fit_edge));
                let mut view = self.compare.as_ref().unwrap().views[side];
                if view.zoom > 0. {
                    self.request(i, Level::Full);
                }
                let full = self.full.get(i);
                let loaded_full = full.is_some();
                let native = self.native_size(i);
                let source = self.source_size(i, view.zoom > 0.);
                let texture = full
                    .or_else(|| self.fit.get(i))
                    .or_else(|| self.thumbs.get(i))
                    .filter(|_| view.zoom <= 0. || native.is_some());
                if let Some(t) = texture {
                    self.viewed.insert(i);
                    if response.clicked()
                        || response.drag_started()
                        || response.dragged()
                        || (response.hovered()
                            && ctx.input(|input| input.smooth_scroll_delta.y != 0.))
                    {
                        self.pending_focus = None;
                    }
                    self.apply_auto_focus(i, area.size(), ctx);
                    view = self.compare.as_ref().unwrap().views[side];
                    let size = self.displayed_size(i, source.unwrap_or_else(|| t.size_vec2()));
                    let native_size = size;
                    let fit_zoom = (photo_area.width() / native_size.x)
                        .min(photo_area.height() / native_size.y)
                        * ctx.pixels_per_point();
                    let wheel = if response.hovered() {
                        ctx.input(|input| input.smooth_scroll_delta.y)
                    } else {
                        0.
                    };
                    if response.clicked() || response.drag_started() || wheel != 0. {
                        self.activate_compare(side);
                    }
                    if response.clicked() {
                        self.zoom_compare(side, if view.zoom > 0. { 0. } else { 1. });
                        if view.zoom == 0.
                            && let Some(pos) = response.interact_pointer_pos()
                        {
                            let screen = Rect::from_center_size(
                                area.center(),
                                native_size * fit_zoom / ctx.pixels_per_point(),
                            );
                            if screen.contains(pos) {
                                let anchor = (screen.center() - pos) / screen.size();
                                let c = self.compare.as_mut().unwrap();
                                let delta = anchor - c.views[side].pan;
                                c.views[side].pan = anchor;
                                if c.linked {
                                    c.views[1 - side].pan += delta;
                                }
                            }
                        }
                    }
                    if wheel != 0. {
                        let target = if native.is_some() {
                            (if view.zoom > 0. { view.zoom } else { fit_zoom }
                                * (wheel * 0.002).exp())
                            .clamp(0.02, 8.)
                        } else {
                            1.
                        };
                        self.zoom_compare(side, target);
                    }
                    if native.is_some() && response.dragged() {
                        self.move_view(response.drag_delta(), ctx.pixels_per_point());
                    }
                    if response.clicked() || response.dragged() || wheel != 0. {
                        ctx.request_repaint();
                    }
                    view = self.compare.as_ref().unwrap().views[side];
                    let actual_zoom = view.zoom;
                    let layout = photo_layout(
                        &t,
                        photo_area,
                        self.photos[i].orientation(),
                        PhotoView {
                            zoom: actual_zoom,
                            pan: view.pan * native_size * actual_zoom / ctx.pixels_per_point(),
                            ppp: ctx.pixels_per_point(),
                        },
                        self.source_size(i, actual_zoom > 0.),
                        self.photos[i].values.crop,
                    );
                    paint_photo_shadow(&painter, layout.rect);
                    paint_photo(&painter, &t, area, &layout, self.preview_for(i));
                    paint_photo_edge(&painter, area, layout.rect);
                    let screen = layout.rect;
                    self.focus_overlay.paint(
                        &painter,
                        &self.paths[i],
                        area,
                        screen,
                        source.unwrap_or_else(|| t.size_vec2()),
                        self.photos[i].orientation(),
                        self.photos[i].values.crop.unwrap_or_default(),
                    );
                    if view.zoom > 0. && !loaded_full {
                        status_plate(
                            &painter,
                            area.left_top() + Vec2::splat(12.),
                            StatusAnchor::TopLeft,
                            "正在加载大图…",
                            theme::FS_LABEL,
                            theme::text(),
                        );
                    }
                } else {
                    let error = self.photos[i].image_errors.get(&Level::Fit(self.fit_edge));
                    status_plate(
                        &painter,
                        area.center(),
                        StatusAnchor::Center,
                        error.map_or("正在加载预览…", String::as_str),
                        theme::FS_TITLE,
                        theme::text(),
                    );
                }
                if self.compare.as_ref().unwrap().active == side {
                    painter.rect_stroke(
                        area,
                        0.,
                        Stroke::new(2., theme::accent()),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        });
    }
    fn enter_compare(&mut self) {
        if self.visible.len() < 2 {
            return;
        }
        if self.compare.is_none() {
            let other = self
                .visible
                .iter()
                .copied()
                .find(|i| *i > self.current)
                .or_else(|| self.visible.iter().copied().find(|i| *i != self.current))
                .unwrap();
            self.compare = Some(Compare {
                indices: [self.current, other],
                views: [View::default(); 2],
                active: 1,
                linked: true,
            });
            self.go(other);
            self.zoom = false;
        }
        self.grid = false;
    }
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        // Keep the photo canvas usable when the sidebar leaves a narrow viewport.
        if ui.available_width() < 600. {
            ui.spacing_mut().button_padding = Vec2::new(4., 2.);
            ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
            ui.spacing_mut().interact_size.y = 20.;
        }
        // Separate selection controls from viewing/develop controls. Each row
        // allocates only its own height and wraps naturally on narrow windows.
        let enabled = !self.visible.is_empty()
            && self.photos[self.current].loaded
            && !self.moving(self.current);
        ui.horizontal_wrapped(|ui| {
            for (label, value, hint) in [
                ("拾取", "pick", "拾取（P）"),
                ("拒绝", "reject", "拒绝（X）"),
                ("未标记", "", "清除旗标（U）"),
            ] {
                let selected = self.photos[self.current]
                    .values
                    .flag
                    .as_deref()
                    .unwrap_or("")
                    == value;
                // Pick and reject are the two decisive actions, so they carry
                // the semantic colour; only the flag that is actually set gets
                // the solid fill, and clearing the flag stays a quiet button.
                let button = match value {
                    "pick" => theme::flag_action(label, theme::success(), selected),
                    "reject" => theme::flag_action(label, theme::danger(), selected),
                    _ => theme::secondary(label),
                };
                if ui
                    .add_enabled(enabled, button)
                    .on_hover_text(hint)
                    .clicked()
                {
                    self.flag(value);
                }
            }
            group_divider(ui);
            control_group(ui, |ui| {
                let mut rating = self.photos[self.current].values.rating.unwrap_or(0);
                if rating_control(ui, &mut rating, enabled) {
                    self.rating(rating);
                }
            });
            group_divider(ui);
            // The chips are a nested group, and egui cannot wrap one: a group
            // wider than the rest of the row is cut off at the window edge
            // instead of moving down to the next line. Measuring the group
            // first lets the row wrap it as a whole.
            let chips: Vec<(bool, Color32, &'static str)> = COLORS
                .iter()
                .zip([
                    "红色（6）",
                    "黄色（7）",
                    "绿色（8）",
                    "蓝色（9）",
                    "紫色",
                ])
                .map(|((label, color), name)| {
                    (
                        self.photos[self.current]
                            .values
                            .label
                            .as_deref()
                            .is_some_and(|v| v.eq_ignore_ascii_case(label)),
                        *color,
                        name,
                    )
                })
                .collect();
            let chip_width = measure_group(
                ui.ctx(),
                ui.spacing(),
                "fastcull-measure-colors",
                |ui| {
                    for (selected, color, _) in &chips {
                        colour_chip(ui, enabled, *color, *selected);
                    }
                },
            );
            control_group_sized(ui, chip_width, |ui| {
                for ((selected, color, name), (label, _)) in chips.iter().zip(COLORS) {
                    if colour_chip(ui, enabled, *color, *selected)
                        .on_hover_text(*name)
                        .clicked()
                    {
                        self.label(label);
                    }
                }
            });
            group_divider(ui);
            // The overflow menu belongs at the far end of the row: the row is
            // a strip of tools, and its last tool sits on the edge.
            let more = "更多";
            let more_width = ui
                .painter()
                .layout_no_wrap(
                    more.to_owned(),
                    theme::font(theme::FS_BODY),
                    theme::text_secondary(),
                )
                .size()
                .x
                + theme::SP_S * 2.
                + 2.;
            let spacer = (ui.max_rect().right() - ui.cursor().left() - more_width).max(0.);
            ui.add_space(spacer);
            let _ = egui::containers::menu::MenuButton::from_button(theme::secondary(more)).ui(
                ui,
                |ui| {
                    if ui
                        .add_enabled(enabled, egui::Button::new("向左旋转  ["))
                        .clicked()
                    {
                        self.rotate(-1);
                        ui.close();
                    }
                    if ui
                        .add_enabled(enabled, egui::Button::new("向右旋转  ]"))
                        .clicked()
                    {
                        self.rotate(1);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            !self.visible.is_empty(),
                            egui::Button::new("重试当前照片  R"),
                        )
                        .clicked()
                    {
                        self.retry_current();
                        ui.close();
                    }
                    if ui.button("打开日志  L").clicked() {
                        let _ = fastcull::platform::open_directory(&crate::logger::directory());
                        ui.close();
                    }
                },
            );
        });
        ui.horizontal_wrapped(|ui| {
            let single = !self.grid && self.compare.is_none();
            let single_button = if single {
                theme::secondary_selected("单图 E")
            } else {
                theme::secondary("单图 E")
            };
            if ui
                .add(single_button)
                .on_hover_text("单图浏览（E）")
                .clicked()
            {
                self.grid = false;
                self.compare = None;
            }
            let grid_button = if self.grid {
                theme::secondary_selected("网格 G")
            } else {
                theme::secondary("网格 G")
            };
            if ui.add(grid_button).on_hover_text("网格浏览（G）").clicked() {
                self.grid = true;
                self.grid_scroll = true;
            }
            let compare_button = if !self.grid && self.compare.is_some() {
                theme::secondary_selected("对比 C")
            } else {
                theme::secondary("对比 C")
            };
            if ui
                .add_enabled(self.visible.len() >= 2, compare_button)
                .on_hover_text("选择当前照片和另一张进行对比（C）")
                .clicked()
            {
                self.enter_compare();
            }
            group_divider(ui);
            if ui
                .add_enabled(!self.visible.is_empty(), theme::secondary("◀"))
                .on_hover_text("上一张（←）")
                .clicked()
            {
                self.next(-1);
            }
            if ui
                .add_enabled(!self.visible.is_empty(), theme::secondary("▶"))
                .on_hover_text("下一张（→）")
                .clicked()
            {
                self.next(1);
            }
            if !self.grid {
                self.focus_overlay.control(ui);
                if let Some(c) = &mut self.compare {
                    ui.checkbox(&mut c.linked, "联动").on_hover_text(
                        "保留各自对准的位置，按相同比例缩放和移动；取消后可分别对准主体",
                    );
                } else {
                    let label = if self.zoom { "100%" } else { "适配" };
                    if ui
                        .add(theme::toggle(label, self.zoom))
                        .on_hover_text("适配 / 100%（F）")
                        .clicked()
                    {
                        self.set_zoom(!self.zoom);
                    }
                }
            }
            // The group is measured before it is placed: a nested group asks
            // for the whole remaining width, so one that does not fit would be
            // cut off at the window edge instead of moving to the next row.
            // The rule in front of the group travels with it, so a group that
            // moves down does not leave a dangling separator behind.
            let width = measure_group(ui.ctx(), ui.spacing(), "fastcull-measure-exposure", |ui| {
                group_divider(ui);
                self.exposure_controls(ui, enabled);
            });
            // The exposure group sits at the far end of the row, opposite the
            // viewing controls: the row reads as two ends, not as a list.
            let spacer = (ui.max_rect().right() - ui.cursor().left() - width - 8.).max(0.);
            ui.add_space(spacer);
            control_group_sized(ui, width, |ui| {
                group_divider(ui);
                let buttons = self.exposure_controls(ui, enabled);
                for (response, thirds) in buttons.iter().zip([-3, -1, 1, 3]) {
                    if response.clicked() {
                        self.adjust_exposure(thirds);
                    }
                }
            });
        });
    }
    /// The exposure buttons and the readout.
    ///
    /// The buttons are built apart from the click handling so the toolbar can
    /// measure the group before it places it; only the real pass may change
    /// the photo.
    fn exposure_controls(&self, ui: &mut egui::Ui, enabled: bool) -> Vec<egui::Response> {
        ui.label("曝光");
        let baseline = self.photos[self.current].exposure_base;
        let available = baseline.is_some() && enabled && self.exposure_preview.is_some();
        let mut buttons = Vec::new();
        for label in ["−1", "−⅓", "+⅓", "+1"] {
            buttons.push(
                ui.add_enabled(available, theme::secondary(label).small())
                    .on_hover_text("曝光快调；JPEG 为近似预览，写回 LR 请同步选片会话")
                    .on_disabled_hover_text("等待有效曝光参数或 GPU 预览"),
            );
        }
        if self.visible.is_empty() {
            ui.weak("—");
        } else if let Some(e) = self.photos[self.current]
            .values
            .exposure
            .or(baseline)
            .filter(|_| baseline.is_some())
        {
            // The token table puts the exposure readout on WARNING, so it
            // reads as a value instead of body copy.
            ui.monospace(
                egui::RichText::new(format!("{:+.2}", e as f64 / 1_000_000.)).color(theme::warning()),
            )
            .on_hover_text("曝光值（EV）");
        } else {
            ui.weak("等待参数")
                .on_hover_text("等待 LR 的有效曝光参数；旧处理版本或未解析的自动曝光可能不可用");
        }
        buttons
    }
    fn cell(&mut self, ui: &mut egui::Ui, i: usize, size: Vec2, film: bool) {
        self.request(i, Level::Thumb);
        // The 200 px preview can take 20–30 ms per cell on Canon/Nikon/JPEG
        // sources. Show the camera JPEG (sub-millisecond) first and let the
        // preview replace it in the same slot when it arrives.
        if !self.thumbs.items.contains_key(&i) && self.fast_thumb_available(i) {
            self.request(i, Level::FastThumb);
        }
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let current = i == self.current;
        let picked = self.folder_editor.photos.contains(&i);
        // The mat behind the photo. A grid cell needs one to carry the file
        // name; in the filmstrip the photo stands on its own, the way it does
        // on the canvas.
        let mat = if !film {
            // A grid cell needs a panel for the file name.
            if current {
                theme::accent_soft()
            } else {
                theme::surface_2()
            }
        } else if picked {
            theme::surface_2()
        } else {
            // In the filmstrip the photo stands on the panel itself, the way
            // it does on the canvas.
            Color32::TRANSPARENT
        };
        if mat != Color32::TRANSPARENT {
            painter.rect_filled(rect, theme::radius(theme::RADIUS_PHOTO), mat);
        }
        let image_rect = Rect::from_min_max(
            rect.min + Vec2::splat(5.),
            Pos2::new(rect.max.x - 5., rect.max.y - if film { 18. } else { 48. }),
        );
        // What sits behind the photo decides the colour of the rounded
        // corners that are painted over the image.
        let behind = if mat == Color32::TRANSPARENT {
            theme::surface()
        } else {
            mat
        };
        // The thumbnail is the same material as the photo on the canvas: a
        // rounded rectangle with a hairline and a soft shadow under it. The
        // shadow, the corners and the selection ring all follow the photo
        // itself, not the box it is fitted into, so a portrait shot in a
        // landscape cell still gets a ring that hugs it.
        let thumb = self.thumbs.get(i).or_else(|| self.fast_thumbs.get(i));
        let mut painted = image_rect;
        if let Some(t) = thumb {
            let layout = photo_layout(
                &t,
                image_rect,
                self.photos[i].orientation(),
                PhotoView {
                    zoom: 0.,
                    pan: Vec2::ZERO,
                    ppp: 1.,
                },
                // The thumbnail's image plane must stay stable as full images
                // arrive/leave the cache (their aspect ratios may differ).
                None,
                self.photos[i].values.crop,
            );
            painted = layout.rect;
            painter.add(
                egui::epaint::Shadow {
                    offset: [0, 2],
                    blur: 9,
                    spread: 0,
                    color: theme::card_shadow(),
                }
                .as_shape(painted, theme::radius(theme::RADIUS_PHOTO)),
            );
            paint_photo(&painter, &t, image_rect, &layout, None);
        } else {
            painter.text(
                image_rect.center(),
                egui::Align2::CENTER_CENTER,
                if self.photos[i].image_errors.contains_key(&Level::Thumb) {
                    "错误"
                } else {
                    "…"
                },
                theme::font(theme::FS_BODY),
                theme::text_muted(),
            );
        }
        round_image_corners(
            &painter,
            painted,
            theme::RADIUS_PHOTO,
            &|_corner, _ex, _ey| Some(behind),
        );
        if current {
            painter.rect_stroke(
                painted.shrink(1.),
                theme::radius(theme::RADIUS_PHOTO),
                Stroke::new(2., theme::accent()),
                egui::StrokeKind::Inside,
            );
        } else if response.hovered() {
            // A thumbnail has no other hover cue, so the pointer needs one to
            // say which one it is over.
            painter.rect_stroke(
                painted.shrink(1.),
                theme::radius(theme::RADIUS_PHOTO),
                Stroke::new(1., theme::accent()),
                egui::StrokeKind::Inside,
            );
        }
        if self.photos[i].values.flag.as_deref() == Some("reject") {
            painter.line_segment(
                [image_rect.left_top(), image_rect.right_bottom()],
                Stroke::new(2., theme::danger()),
            );
            painter.line_segment(
                [image_rect.right_top(), image_rect.left_bottom()],
                Stroke::new(2., theme::danger()),
            );
        }
        let photo = &self.photos[i];
        let flag = match photo.values.flag.as_deref() {
            Some("pick") => "P ",
            Some("reject") => "X ",
            _ => "",
        };
        let flag_color = match flag {
            "P " => theme::success(),
            "X " => theme::danger(),
            _ => theme::text_secondary(),
        };
        let stars = photo.values.rating.unwrap_or(0) as usize;
        let baseline = Pos2::new(rect.min.x + 5., rect.max.y - if film { 14. } else { 39. });
        painter.text(
            baseline,
            egui::Align2::LEFT_TOP,
            flag,
            theme::font(theme::FS_CAPTION),
            flag_color,
        );
        // The stars follow the flag as a row of marks, not as text.
        for n in 0..stars {
            draw_star(
                &painter,
                baseline + Vec2::new(12. + n as f32 * 8., 7.),
                3.5,
                true,
                theme::star(),
            );
        }
        if let Some((_, color)) = COLORS.iter().find(|(l, _)| {
            photo
                .values
                .label
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case(l))
        }) {
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(rect.max.x - 10., rect.max.y - 13.),
                    Vec2::splat(6.),
                ),
                theme::radius(theme::RADIUS_SM),
                *color,
            );
        }
        if !film {
            let name = self.paths[i]
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            painter.text(
                Pos2::new(rect.min.x + 5., rect.max.y - 20.),
                egui::Align2::LEFT_TOP,
                name,
                theme::font(theme::FS_SMALL),
                theme::text_secondary(),
            );
        }
        if response.clicked() {
            self.folder_editor
                .select(i, &self.visible, ui.input(|input| input.modifiers));
            self.go(i);
        }
        if response.drag_started() {
            if !self.folder_editor.photos.contains(&i) {
                self.folder_editor
                    .select(i, &self.visible, egui::Modifiers::NONE);
                self.go(i);
            }
            response.dnd_set_drag_payload(self.folder_editor.selection(i, &self.visible));
        }
        if response.double_clicked() {
            self.go(i);
            self.grid = false;
            self.compare = None;
        }
        if self.moving(i) {
            painter.text(
                rect.center_top() + Vec2::new(0., 5.),
                egui::Align2::CENTER_TOP,
                "移动中",
                theme::font(theme::FS_SMALL),
                theme::accent(),
            );
        }
    }
    fn film_panel(&mut self, ctx: &egui::Context) -> Rect {
        let initial_height = self
            .smoke
            .as_ref()
            .and_then(|_| std::env::var("FASTCULL_SMOKE_FILM_HEIGHT").ok())
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(94.);
        egui::TopBottomPanel::bottom("film")
            .frame(
                egui::Frame::new()
                    .fill(crate::appearance::palette(ctx).panel)
                    .inner_margin(egui::Margin::symmetric(10, 7)),
            )
            .resizable(true)
            .default_height(initial_height)
            .height_range(84. ..=(ctx.content_rect().height() * 0.35).clamp(104., 280.))
            .frame(
                egui::Frame::new()
                    .fill(theme::raised())
                    .inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 8 }),
            )
            .show(ctx, |ui| {
                ui.add_space(8.);
                self.film(ui);
                // The tray begins in a hairline: it is a shelf of its own.
                let rect = ui.max_rect();
                ui.painter().hline(
                    rect.x_range(),
                    rect.top() + 0.5,
                    Stroke::new(1., theme::line()),
                );
            })
            .response
            .rect
    }
    fn film(&mut self, ui: &mut egui::Ui) {
        let active = ui.rect_contains_pointer(ui.max_rect())
            && !self.close_dialog
            && !self.folder_editor.editing(ui.ctx())
            && !egui::Popup::is_any_open(ui.ctx())
            && !ui.input(|i| i.pointer.any_down());
        if active {
            let delta = ui.input_mut(|input| {
                let mut steps = 0.;
                input.events.retain(|event| {
                    if let egui::Event::MouseWheel {
                        unit,
                        delta,
                        modifiers,
                    } = event
                    {
                        if modifiers.ctrl || modifiers.command || modifiers.alt {
                            return true;
                        }
                        let amount = if delta.y != 0. { delta.y } else { delta.x };
                        steps -= amount
                            / match unit {
                                egui::MouseWheelUnit::Point => 40.,
                                egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => 1.,
                            };
                        return false;
                    }
                    true
                });
                if steps != 0. {
                    input.raw_scroll_delta = Vec2::ZERO;
                    input.smooth_scroll_delta = Vec2::ZERO;
                }
                steps
            });
            if delta != 0. && self.film_wheel.signum() != delta.signum() {
                self.film_wheel = 0.;
            }
            self.film_wheel += delta;
            let steps = self.film_wheel.trunc().clamp(-8., 8.) as isize;
            if steps != 0 {
                self.film_wheel = self.film_wheel.fract();
                for _ in 0..steps.unsigned_abs() {
                    self.next(steps.signum());
                }
            }
        } else {
            self.film_wheel = 0.;
        }
        let height = ui.available_height().clamp(64., 248.);
        let width = (height * 1.15)
            .max(76.)
            .min((ui.available_width() - 64.).max(40.));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.;
            if ui
                .add_sized([24., height], egui::Button::new("◀").frame(false))
                .clicked()
            {
                self.next(-4);
            }
            let count = ((ui.available_width() - 28.) / (width + 4.))
                .floor()
                .max(1.) as usize;
            let position = self
                .visible
                .iter()
                .position(|i| *i == self.current)
                .unwrap_or(0);
            let start = position
                .saturating_sub(count / 2)
                .min(self.visible.len().saturating_sub(count));
            for position in start..(start + count).min(self.visible.len()) {
                let i = self.visible[position];
                self.cell(ui, i, Vec2::new(width, height), true);
            }
            if ui
                .add_sized([24., height], egui::Button::new("▶").frame(false))
                .clicked()
            {
                self.next(4);
            }
        });
    }
    fn zoom_at(&mut self, screen: Rect, pos: Option<Pos2>) {
        let anchor = pos
            .filter(|p| screen.contains(*p))
            .map(|p| (screen.center() - p) / screen.size());
        let entering = !self.zoom;
        self.set_zoom(!self.zoom);
        if entering && let Some(anchor) = anchor {
            self.pan = anchor;
        }
    }
    fn native_size(&self, i: usize) -> Option<Vec2> {
        self.full
            .items
            .get(&i)
            .map(|t| t.handle.size_vec2())
            .or(self.photos[i].full_size)
    }
    fn source_size(&self, i: usize, zoomed: bool) -> Option<Vec2> {
        if zoomed {
            // At 100%, actual decoded pixels remain authoritative.
            return self.native_size(i);
        }
        self.photos[i]
            .info
            .as_ref()
            .and_then(|info| info.image_size)
            .map(|(w, h)| Vec2::new(w as f32, h as f32))
            .or_else(|| self.native_size(i))
    }
    fn displayed_size(&self, i: usize, size: Vec2) -> Vec2 {
        self.photos[i]
            .values
            .crop
            .unwrap_or_default()
            .geometry(size, self.photos[i].orientation())
            .rect
            .size()
    }
    fn begin_crop(&mut self) {
        self.pending_focus = None;
        if self.visible.is_empty() || !self.photos[self.current].loaded || self.moving(self.current)
        {
            return;
        }
        if self.develop_pending(self.current) {
            self.crop_requested = true;
            return;
        }
        if let Some(size) = self.native_size(self.current) {
            let geometry = self.photos[self.current]
                .values
                .crop
                .unwrap_or_default()
                .geometry(size, self.photos[self.current].orientation());
            if geometry.rect.width() <= 0. || geometry.rect.height() <= 0. {
                self.error =
                    Some("现有裁切坐标无效，已保留原 XMP。请先在 Camera Raw 中检查。".into());
                self.crop_requested = false;
                return;
            }
            self.crop_editor = Some(crate::crop_ui::Editor::new(
                self.photos[self.current].values.crop.unwrap_or_default(),
                size,
                self.photos[self.current].orientation(),
            ));
            self.crop_requested = false;
            self.close_dialog = false;
            self.grid = false;
            self.compare = None;
            self.zoom = false;
            self.pan = Vec2::ZERO;
        } else {
            self.crop_requested = true;
            self.request(self.current, Level::Full);
        }
    }
    fn begin_raw(&mut self, ctx: &egui::Context) {
        if self.visible.is_empty() || self.moving(self.current) {
            return;
        }
        self.raw_windows.open(
            self.paths[self.current].clone(),
            self.current,
            self.session.as_ref().and_then(|s| s.raw_settings_source()),
            ctx,
        );
    }
    fn receive_raw(&mut self) {
        for event in self.raw_windows.poll() {
            match event {
                crate::raw_window::Event::Saved(path, saved) => {
                    let (values, updates) = *saved;
                    if let Some(index) = self.paths.iter().position(|p| p == &path) {
                        self.photos[index].apply_raw_saved(values, updates);
                        self.pool.retry_metadata(index);
                        self.filter_dirty = true;
                    }
                }
                crate::raw_window::Event::Error(error) => self.error = Some(error),
            }
        }
    }
    fn apply_crop(&mut self) {
        let Some(editor) = &self.crop_editor else {
            return;
        };
        let value = editor.value();
        let full = fastcull::crop::Geometry::full(editor.display());
        let reset = editor.geometry.angle == 0. && editor.geometry.rect == full.rect;
        self.save(Updates {
            crop: Some(if reset { Default::default() } else { value }),
            orientation: Some(editor.orientation),
            ..Default::default()
        });
        if self.error.is_none() {
            self.crop_editor = None;
            self.pan = Vec2::ZERO;
        }
    }
    fn crop_workspace(&mut self, ctx: &egui::Context) {
        use crate::crop_ui::Action;
        let mut editor = self.crop_editor.take().unwrap();
        let mut action = Action::None;
        egui::TopBottomPanel::top("crop-title").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                crate::appearance::control(ui);
                ui.strong("裁切照片");
                ui.label(
                    self.paths[self.current]
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                );
                ui.weak("应用后保存到 XMP，可随时重新裁切");
            });
            if let Some(e) = &self.error {
                ui.colored_label(theme::danger(), e);
            }
        });
        egui::SidePanel::right("crop-controls")
            .exact_width(280.)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    action = editor.controls(ui);
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::canvas())
                    .inner_margin(theme::SP_L),
            )
            .show(ctx, |ui| {
                let (area, response) =
                    ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
                self.fit_edge = preview_edge(area.size(), ctx.pixels_per_point());
                self.request(self.current, Level::Fit(self.fit_edge));
                if let Some(t) = self
                    .full
                    .get(self.current)
                    .or_else(|| self.fit.get(self.current))
                {
                    let display = editor.display();
                    let (s, c) = editor.geometry.angle.to_radians().sin_cos();
                    let bounds = Vec2::new(
                        c.abs() * display.x + s.abs() * display.y,
                        s.abs() * display.x + c.abs() * display.y,
                    );
                    let scale = (area.width() / bounds.x).min(area.height() / bounds.y) * 0.94;
                    let rect = Rect::from_center_size(area.center(), display * scale);
                    let g = fastcull::crop::Geometry {
                        rect: Rect::from_center_size((display * 0.5).to_pos2(), bounds),
                        angle: editor.geometry.angle,
                    };
                    let backdrop = Rect::from_center_size(area.center(), bounds * scale);
                    let uv = g.uvs(editor.size, editor.orientation);
                    let painter = ui.painter_at(area);
                    if let Some(preview) = &self.exposure_preview {
                        let delta = self.preview_for(self.current).map_or(0., |(_, d)| d);
                        preview.paint(&painter, area, backdrop, t.id(), uv, delta);
                    } else {
                        paint_uv(&painter, &t, backdrop, uv);
                    }
                    editor.overlay(ui, &response, rect);
                }
            });
        self.crop_editor = Some(editor);
        match action {
            Action::Apply => self.apply_crop(),
            Action::Cancel => self.crop_editor = None,
            Action::None => {}
        }
    }
    fn central(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // The canvas is the darkest surface in the window, and it carries a
        // slow gradient so the picture has something to stand on.
        paint_canvas(ui.painter(), ui.max_rect());
        if self.compare.is_some() && !self.visible.is_empty() && !self.grid {
            self.compare_canvas(ui, ctx);
            return;
        }
        if self.visible.is_empty() {
            // The empty state is a card with a way out, not a line of text in
            // an empty window.
            let size = Vec2::new(360., 150.);
            let (rect, _response) = ui
                .centered_and_justified(|ui| {
                    ui.allocate_exact_size(size, egui::Sense::hover())
                })
                .inner;
            ui.painter()
                .rect_filled(rect, theme::radius(theme::RADIUS), theme::surface_2());
            ui.painter().rect_stroke(
                rect,
                theme::radius(theme::RADIUS),
                Stroke::new(1., theme::line()),
                egui::StrokeKind::Inside,
            );
            let mut clear = false;
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(22.);
                    ui.label(
                        egui::RichText::new("没有符合当前条件的照片")
                            .size(theme::FS_LABEL)
                            .color(theme::text()),
                    );
                    ui.label(
                        egui::RichText::new("清除筛选条件后即可重新浏览这一批照片。")
                            .size(theme::FS_SMALL)
                            .color(theme::text_secondary()),
                    );
                    ui.add_space(14.);
                    if ui.add(theme::primary("清除筛选条件")).clicked() {
                        clear = true;
                    }
                });
            });
            if clear {
                self.filter_editor.clear(&mut self.filters);
                self.filter_dirty = true;
            }
            return;
        }
        if self.grid {
            // The column count follows the available width: a fixed six turned
            // the cells into narrow strips in a small window, so the
            // thumbnails shrank to colour blocks and the labels were cut.
            let available = ui.available_width();
            let cols = ((available / 140.).floor() as usize).clamp(1, 6);
            let width = (available / cols as f32 - 4.).max(32.);
            let height = (width * 1.37).clamp(150., 220.);
            let row_height = height + 4.;
            let mut area = egui::ScrollArea::vertical().id_salt("grid");
            if self.grid_scroll {
                let position = self
                    .visible
                    .iter()
                    .position(|i| *i == self.current)
                    .unwrap_or(0);
                area = area.vertical_scroll_offset((position / cols) as f32 * row_height);
                self.grid_scroll = false;
            }
            area.show_rows(
                ui,
                row_height,
                self.visible.len().div_ceil(cols),
                |ui, rows| {
                    for row in rows {
                        ui.horizontal(|ui| {
                            for position in row * cols..((row + 1) * cols).min(self.visible.len()) {
                                let i = self.visible[position];
                                self.cell(ui, i, Vec2::new(width, height), false);
                            }
                        });
                    }
                },
            );
            return;
        }
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        self.canvas_size = rect.size();
        self.fit_edge = preview_edge(rect.size(), ctx.pixels_per_point());
        self.request(self.current, Level::Fit(self.fit_edge));
        if self.zoom {
            self.request(self.current, Level::Full);
        }
        let painter = ui.painter_at(rect);
        // The window base is painted once, over everything, so the photo
        // stands on the same surface the cards float on.
        let full = self.full.get(self.current);
        let actual_full = full.is_some();
        let native = self.source_size(self.current, self.zoom);
        let texture = full
            .or_else(|| self.fit.get(self.current))
            .or_else(|| self.thumbs.get(self.current))
            .filter(|_| !self.zoom || native.is_some());
        if let Some(t) = texture {
            self.viewed.insert(self.current);
            if response.clicked() || response.drag_started() || response.dragged() {
                self.pending_focus = None;
            }
            self.apply_auto_focus(self.current, rect.size(), ctx);
            if self.zoom && response.dragged() {
                self.move_view(response.drag_delta(), ctx.pixels_per_point());
            }
            let shown = self.displayed_size(self.current, native.unwrap_or_else(|| t.size_vec2()))
                / ctx.pixels_per_point();
            // The photo keeps a generous margin inside the canvas. The dark
            // around it is what makes it read as a print on a table.
            let margin = (rect.height() * 0.055).clamp(10., 34.);
            let photo_area = rect.shrink(margin);
            let layout = photo_layout(
                &t,
                photo_area,
                self.photos[self.current].orientation(),
                PhotoView {
                    zoom: if self.zoom { 1. } else { 0. },
                    pan: self.pan * shown,
                    ppp: ctx.pixels_per_point(),
                },
                native,
                self.photos[self.current].values.crop,
            );
            paint_photo_shadow(&painter, layout.rect);
            paint_photo_glow(&painter, layout.rect);
            paint_photo(&painter, &t, rect, &layout, self.preview_for(self.current));
            paint_photo_edge(&painter, rect, layout.rect);
            let screen = layout.rect;
            self.focus_overlay.paint(
                &painter,
                &self.paths[self.current],
                rect,
                screen,
                native.unwrap_or_else(|| t.size_vec2()),
                self.photos[self.current].orientation(),
                self.photos[self.current].values.crop.unwrap_or_default(),
            );
            if !self.first_draw {
                log::info!(
                    "first loupe {:.1}ms canvas={}x{} image={}x{}",
                    self.started.elapsed().as_secs_f64() * 1000.,
                    rect.width(),
                    rect.height(),
                    screen.width(),
                    screen.height()
                );
                self.first_draw = rect.width() > 100.
                    && rect.height() > 100.
                    && screen.width() > 100.
                    && screen.height() > 100.;
            }
            if response.clicked() {
                self.zoom_at(screen, response.interact_pointer_pos());
            }
            if self.zoom
                && actual_full
                && let Some(start) = self.zoom_started.take()
            {
                log::info!(
                    "100% ready image={} latency_ms={:.1} texture={}x{}",
                    self.current,
                    start.elapsed().as_secs_f64() * 1000.,
                    t.size()[0],
                    t.size()[1]
                );
            }
        } else if let Some(e) = self.photos[self.current]
            .image_errors
            .get(&Level::Fit(self.fit_edge))
            .cloned()
        {
            status_plate(
                &painter,
                rect.center(),
                StatusAnchor::Center,
                &format!("预览加载失败：{e}\n按 R 重试，或按右箭头继续选片"),
                theme::FS_TITLE,
                theme::danger(),
            );
        } else {
            status_plate(
                &painter,
                rect.center(),
                StatusAnchor::Center,
                "正在加载内嵌 JPEG…",
                theme::FS_TITLE,
                theme::text(),
            );
        }
        if (self.zoom || self.crop_requested) && !actual_full {
            let message = self.photos[self.current]
                .image_errors
                .get(&Level::Full)
                .map_or(
                    if self.crop_requested {
                        "正在准备裁切…"
                    } else {
                        "正在加载 100% 清晰图…"
                    }
                    .to_owned(),
                    |e| format!("100% 加载失败：{e}；按 R 重试"),
                );
            status_plate(
                &painter,
                rect.left_top() + Vec2::splat(12.),
                StatusAnchor::TopLeft,
                &message,
                theme::FS_LABEL,
                theme::warning(),
            );
        }
        if !actual_full
            && !self.fit.items.contains_key(&self.current)
            && let Some(e) = self.photos[self.current]
                .image_errors
                .get(&Level::Fit(self.fit_edge))
        {
            status_plate(
                &painter,
                rect.left_bottom() - Vec2::new(-12., 24.),
                StatusAnchor::BottomLeft,
                &format!("适屏预览失败：{e}；按 R 重试"),
                theme::FS_LABEL,
                theme::warning(),
            );
        }
    }
    fn smoke(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            if let egui::Event::Screenshot { image, .. } = ev
                && let Some(s) = &mut self.smoke
            {
                let path = s.dir.join(format!("{:02}.png", s.step));
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                if let Err(e) = save_png(&path, image.size, &bytes) {
                    log::error!("screenshot: {e}");
                }
                s.step += 1;
                s.waiting = false;
                s.prepared = false;
                s.last = Instant::now();
            }
        }
        let Some(s) = &self.smoke else {
            return;
        };
        if std::env::var_os("FASTCULL_SETTINGS_SMOKE").is_some() {
            let step = s.step;
            if step >= 5 {
                std::fs::write(s.dir.join("settings-ok.txt"), "Settings menus, first-visit AF positioning, remembered pan and missing-AF fallback passed").unwrap();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            assert!(
                self.started.elapsed().as_secs() < 45,
                "settings smoke timeout at {step}"
            );
            if s.waiting || s.last.elapsed().as_millis() < 500 {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            if !s.prepared {
                match step {
                    0 => {
                        ctx.set_theme(egui::ThemePreference::Light);
                        egui::Popup::open_id(ctx, egui::Id::new("culling-settings"));
                    }
                    1 => {
                        ctx.set_theme(egui::ThemePreference::Dark);
                        egui::Popup::open_id(ctx, egui::Id::new("culling-settings"));
                    }
                    2 => {
                        egui::Popup::close_id(ctx, egui::Id::new("culling-settings"));
                        ctx.set_theme(egui::ThemePreference::Light);
                        self.settings.focus_on_unviewed = true;
                        self.set_zoom(true);
                        self.pan = Vec2::new(0.05, -0.05);
                        self.go(1);
                    }
                    3 => {
                        self.move_view(Vec2::new(60., 30.), ctx.pixels_per_point());
                        let manual = self.pan;
                        self.go(0);
                        self.go(1);
                        assert_eq!(self.pan, manual);
                        assert!(self.pending_focus.is_none());
                    }
                    4 => {
                        let pan = self.pan;
                        self.go(2);
                        assert_eq!(self.pan, pan);
                    }
                    _ => unreachable!(),
                }
                let s = self.smoke.as_mut().unwrap();
                s.prepared = true;
                s.last = Instant::now();
                ctx.request_repaint();
                return;
            }
            if self.pending_focus.is_some()
                || !self.photos[self.current].loaded
                || self.native_size(self.current).is_none()
            {
                self.request(self.current, Level::Full);
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            if step == 2 {
                let unit = Vec2::new(-0.25, 0.25);
                let shown = self
                    .displayed_size(self.current, self.native_size(self.current).unwrap())
                    / ctx.pixels_per_point();
                let limits = (shown - self.canvas_size).max(Vec2::ZERO) * 0.5 / shown;
                assert!((self.pan - unit.clamp(-limits, limits)).length() < 0.0001);
                assert!(!self.focus_overlay.enabled);
            }
            if step == 4 {
                assert_eq!(self.pan, *self.loupe_positions.get(&1).unwrap());
            }
            self.smoke.as_mut().unwrap().waiting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            return;
        }
        if std::env::var_os("FASTCULL_CROP_SMOKE").is_some() {
            let step = s.step;
            let waiting = s.waiting;
            let prepared = s.prepared;
            let elapsed = s.last.elapsed();
            if self.started.elapsed().as_secs() > 60 {
                panic!("crop smoke timeout at {step}");
            }
            if step >= 8 {
                assert!(self.error.is_none(), "{:?}", self.error);
                std::fs::write(
                    s.dir.join("crop-ok.txt"),
                    "Crop editor, rotation, XMP roundtrip and preview passed",
                )
                .unwrap();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            if waiting || elapsed.as_millis() < 500 {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            if self.native_size(self.current).is_none()
                || !self.photos[self.current].loaded
                || self.develop_pending(self.current)
            {
                self.request(self.current, Level::Full);
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            if !prepared {
                match step {
                    0 => {
                        if std::env::var_os("FASTCULL_IMPORT_CROP_SMOKE").is_some() {
                            let p = &self.photos[self.current];
                            assert_eq!(p.orientation(), 6);
                            assert_eq!(
                                p.values.crop,
                                Some(fastcull::crop::Crop {
                                    enabled: true,
                                    left: 200000,
                                    top: 100000,
                                    right: 800000,
                                    bottom: 900000,
                                    angle: 0,
                                })
                            );
                            if self.session.is_some() {
                                // Catalog-only crop must not be persisted just by viewing.
                                assert!(
                                    fastcull::xmp::read(&self.paths[self.current])
                                        .unwrap()
                                        .crop
                                        .is_none()
                                );
                            }
                        }
                    }
                    1 => self.begin_crop(),
                    2 => {
                        let e = self.crop_editor.as_mut().unwrap();
                        e.select_ratio("16 : 9".into(), Some(16. / 9.));
                        e.geometry.angle = 12.;
                        e.geometry.constrain(e.display());
                    }
                    3 => {
                        let e = self.crop_editor.as_mut().unwrap();
                        e.rotate(crate::crop_ui::clockwise(e.orientation));
                    }
                    4 => {
                        self.apply_crop();
                        assert!(self.crop_editor.is_none(), "{:?}", self.error);
                        assert_eq!(
                            fastcull::xmp::read(&self.paths[self.current]).unwrap().crop,
                            self.photos[self.current].values.crop
                        );
                    }
                    5 => self.begin_crop(),
                    6 => {
                        self.crop_editor = None;
                        self.grid = true;
                    }
                    7 => {
                        self.grid = false;
                        self.set_zoom(true);
                    }
                    _ => unreachable!(),
                }
                let s = self.smoke.as_mut().unwrap();
                s.prepared = true;
                s.last = Instant::now();
                ctx.request_repaint();
                return;
            }
            self.smoke.as_mut().unwrap().waiting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            ctx.request_repaint();
            return;
        }
        if self.started.elapsed().as_secs() > 60 {
            log::error!("smoke timeout");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if s.step == 5 && self.stress_remaining > 0 {
            self.stress_remaining -= 1;
            self.zoom = self.stress_remaining > 90;
            self.go((self.stress_remaining * 37) % self.paths.len());
            self.grid = self.stress_remaining.is_multiple_of(9);
            log::info!(
                "stress remaining={} current={}",
                self.stress_remaining,
                self.current
            );
            ctx.request_repaint();
            return;
        }
        if s.waiting
            || s.last.elapsed().as_millis() < 500
            || !(if self.zoom {
                self.full.items.contains_key(&self.current)
            } else {
                self.fit.items.contains_key(&self.current)
                    || self.full.items.contains_key(&self.current)
            })
            || !(self.photos[self.current].loaded
                || self.photos[self.current].metadata_error.is_some())
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        let step = s.step;
        let prepared = s.prepared;
        let features = std::env::var_os("FASTCULL_FEATURE_SMOKE").is_some();
        let folders = std::env::var_os("FASTCULL_FOLDER_SMOKE").is_some();
        let bridge_moves = std::env::var_os("FASTCULL_BRIDGE_MOVE_SMOKE").is_some();
        if bridge_moves && step >= 14 && self.mover.as_ref().is_some_and(|m| m.jobs > 0) {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
            return;
        }
        if features && prepared {
            let ready = self.compare.as_ref().is_none_or(|c| {
                c.indices.iter().enumerate().all(|(side, i)| {
                    if c.views[side].zoom > 0. {
                        self.full.items.contains_key(i)
                    } else {
                        self.fit.items.contains_key(i) || self.full.items.contains_key(i)
                    }
                })
            });
            if !ready || (step == 10 && self.photos.iter().any(|p| !p.capture_indexed)) {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            if step == 9 && !self.smoke.as_ref().unwrap().relative_link_tested {
                let c = self.compare.as_ref().unwrap();
                let offset = c.views[0].pan - c.views[1].pan;
                let ratio = c.views[0].zoom / c.views[1].zoom;
                self.zoom_compare(1, 1.25);
                self.move_view(Vec2::new(40., 20.), ctx.pixels_per_point());
                let c = self.compare.as_ref().unwrap();
                assert!((c.views[0].pan - c.views[1].pan - offset).length() < 0.00001);
                assert!((c.views[0].zoom / c.views[1].zoom - ratio).abs() < 0.00001);
                let views = c.views;
                self.compare.as_mut().unwrap().linked = false;
                self.move_view(Vec2::new(-20., 0.), ctx.pixels_per_point());
                assert_eq!(self.compare.as_ref().unwrap().views[0].pan, views[0].pan);
                self.compare.as_mut().unwrap().views = views;
                self.compare.as_mut().unwrap().linked = true;
                let s = self.smoke.as_mut().unwrap();
                std::fs::write(s.dir.join("relative-link.json"), format!(
                    "{{\"verified\":true,\"a\":{{\"zoom\":{},\"pan\":[{},{}]}},\"b\":{{\"zoom\":{},\"pan\":[{},{}]}},\"offset\":[{},{}]}}",
                    views[0].zoom, views[0].pan.x, views[0].pan.y,
                    views[1].zoom, views[1].pan.x, views[1].pan.y, offset.x, offset.y,
                )).unwrap();
                s.relative_link_tested = true;
                ctx.request_repaint();
                return;
            }
        }
        if step == 1 && self.zoom && self.zoom_started.is_none() && !s.cached_zoom_tested {
            self.set_zoom(false);
            self.set_zoom(true);
            self.smoke.as_mut().unwrap().cached_zoom_tested = true;
            ctx.request_repaint();
            return;
        }
        if step
            >= if bridge_moves {
                21
            } else if folders {
                19
            } else if features {
                13
            } else {
                5
            }
        {
            let dir = s.dir.clone();
            if bridge_moves {
                std::fs::write(
                    dir.join("moved-paths.txt"),
                    self.paths
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            format!("{i}\t{}\n", fastcull::handoff::encode(&p.to_string_lossy()))
                        })
                        .collect::<String>(),
                )
                .unwrap();
                assert!(self.error.is_none(), "{:?}", self.error);
                assert!(self.folder_editor.plan.moves.is_empty());
            }
            let _ = std::fs::write(
                dir.join("result.txt"),
                format!(
                    "first_draw={} current={} photos={} full={} fit={} thumbs={} fast_thumbs={} error={:?} stress_remaining={} elapsed_ms={:.0}",
                    self.first_draw,
                    self.current,
                    self.paths.len(),
                    self.full.items.len(),
                    self.fit.items.len(),
                    self.thumbs.items.len(),
                    self.fast_thumbs.items.len(),
                    self.error,
                    self.stress_remaining,
                    self.started.elapsed().as_secs_f64() * 1000.
                ),
            );
            if features {
                self.finish_close(ctx, self.session.is_some());
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        if !prepared {
            match step {
                1 => {
                    self.set_zoom(true);
                    self.pan = Vec2::new(0.09, 0.03);
                    if self.paths.len() > 1 {
                        let before = self.current;
                        for (key, expected) in [
                            (Key::ArrowRight, (before + 1).min(self.paths.len() - 1)),
                            (Key::ArrowLeft, before),
                        ] {
                            ctx.input_mut(|i| {
                                i.events.push(egui::Event::Key {
                                    key,
                                    physical_key: None,
                                    pressed: true,
                                    repeat: false,
                                    modifiers: egui::Modifiers::NONE,
                                })
                            });
                            self.keyboard(ctx);
                            assert_eq!(self.current, expected, "native 100% arrow navigation");
                            assert!(self.zoom, "navigation must preserve 100% zoom");
                            assert_eq!(
                                self.pan,
                                Vec2::new(0.09, 0.03),
                                "navigation must preserve relative anchor"
                            );
                        }
                        log::info!("native 100% arrow navigation verified");
                    }
                }
                2 => {
                    self.zoom = false;
                    self.grid = true;
                }
                3 => {
                    self.grid = false;
                    self.go(self.paths.len() - 1);
                }
                4 => {
                    self.rating(4);
                    self.label("Purple");
                    self.flag("reject");
                    self.rotate(1);
                }
                5 if features => {
                    self.zoom = false;
                    self.compare = None;
                }
                6 if features => {
                    let p = &self.photos[self.current];
                    let target = p.exposure_base.expect("feature smoke needs baseline") + 1_000_000;
                    self.save(Updates {
                        exposure: Some(target),
                        ..Default::default()
                    });
                }
                7 if features => {
                    let current = self.current;
                    self.compare = Some(Compare {
                        indices: [0, current],
                        views: [View::default(); 2],
                        active: 1,
                        linked: false,
                    });
                    self.grid = false;
                    self.zoom = false;
                }
                8 if features => {
                    let c = self.compare.as_mut().unwrap();
                    c.views[0] = View {
                        zoom: 1.,
                        pan: Vec2::new(0.05, 0.02),
                    };
                }
                9 if features => {
                    let c = self.compare.as_mut().unwrap();
                    c.views[1] = View {
                        zoom: 1.,
                        pan: Vec2::new(-0.06, 0.08),
                    };
                    // Alignment belongs to each photo. Enabling linking leaves it intact.
                    c.linked = true;
                    self.zoom = true;
                }
                10 if features => {
                    self.compare = None;
                    self.zoom = false;
                    self.filter_editor.open = true;
                    self.pool.start_index();
                    self.filters.selected.insert(
                        fastcull::filtering::Field::Rating,
                        [Some("4".into())].into(),
                    );
                    self.filter_dirty = true;
                }
                11 if features => {
                    self.filters.selected.insert(
                        fastcull::filtering::Field::Rating,
                        [Some("5".into())].into(),
                    );
                    self.filter_dirty = true;
                }
                12 if features => {
                    self.close_dialog = true;
                }
                13 if bridge_moves => {
                    assert!(self.folder_editor.direct && self.paths.len() >= 4);
                    self.close_dialog = false;
                    self.filter_editor.clear(&mut self.filters);
                    self.filter_editor.open = false;
                    self.folder_editor.open = true;
                    self.folder_editor.selected = None;
                    self.refilter();
                    self.go(0);
                    let root = self.paths[0].parent().unwrap().parent().unwrap().to_owned();
                    let target = self.folder_editor.create_folder(&root, "精选").unwrap();
                    self.folder_editor.stage(&[0], &target);
                    self.submit_moves();
                    assert_eq!(self.current, 1);
                }
                14 if bridge_moves => {
                    assert_eq!(
                        self.paths[0].parent(),
                        self.folder_editor.last_target.as_deref()
                    );
                    self.folder_editor.repeat_move(self.current, &self.visible);
                    self.submit_moves();
                    assert_eq!(self.current, 2);
                }
                15 if bridge_moves => {
                    let root = self.paths[0].parent().unwrap().parent().unwrap().to_owned();
                    let target = self.folder_editor.create_folder(&root, "批量").unwrap();
                    self.folder_editor.photos = [2, 3].into();
                    self.folder_editor.stage(&[2, 3], &target);
                    self.submit_moves();
                }
                16 if bridge_moves => {
                    self.go(0);
                    self.folder_editor.advance = false;
                    self.compare = Some(Compare {
                        indices: [0, 3],
                        views: [
                            View {
                                zoom: 1.,
                                pan: Vec2::new(0.05, 0.02),
                            },
                            View {
                                zoom: 1.,
                                pan: Vec2::new(-0.06, 0.08),
                            },
                        ],
                        active: 0,
                        linked: true,
                    });
                    let root = self.paths[0].parent().unwrap().parent().unwrap().to_owned();
                    let target = self.folder_editor.create_folder(&root, "再次移动").unwrap();
                    self.folder_editor.stage(&[0], &target);
                    self.submit_moves();
                }
                17 if bridge_moves => {
                    assert_eq!(self.compare.as_ref().unwrap().indices, [0, 3]);
                    assert_eq!(
                        self.compare.as_ref().unwrap().views[0].pan,
                        Vec2::new(0.05, 0.02)
                    );
                    self.rating(5);
                    self.adjust_exposure(1);
                    assert_eq!(fastcull::xmp::read(&self.paths[0]).unwrap().rating, Some(5));
                    assert!(self.paths[0].with_extension("xmp").is_file());
                    self.folder_editor.selected = self.paths[0].parent().map(PathBuf::from);
                    self.refilter();
                    assert_eq!(self.visible, [0]);
                }
                18 if bridge_moves => {
                    self.folder_editor.selected = None;
                    self.refilter();
                    self.folder_editor
                        .move_dialog(vec![2, 3], self.folder_editor.last_target.clone().unwrap());
                }
                19 if bridge_moves => {
                    self.folder_editor.dismiss_dialog();
                    if let Some(target) = std::env::var_os("FASTCULL_MOVE_SMOKE_CROSS") {
                        self.compare = None;
                        self.go(1);
                        self.rating(3);
                        self.go(0);
                        let target = PathBuf::from(target);
                        std::fs::create_dir_all(&target).unwrap();
                        self.folder_editor.stage(&[1], &target);
                        self.submit_moves();
                    }
                }
                20 if bridge_moves => {
                    self.compare = None;
                    self.go(1);
                    self.zoom = false;
                    if let Some(target) = std::env::var_os("FASTCULL_MOVE_SMOKE_CROSS") {
                        assert_eq!(
                            self.paths[1].parent(),
                            Some(PathBuf::from(target).as_path())
                        );
                    }
                    self.close_dialog = true;
                }
                13 if folders => {
                    self.close_dialog = false;
                    self.filter_editor.clear(&mut self.filters);
                    self.folder_editor.open = true;
                    self.folder_editor.selected = None;
                    let root = self.paths[0].parent().unwrap().parent().unwrap().to_owned();
                    let target = self.folder_editor.create_folder(&root, "精选").unwrap();
                    let indices = vec![0, self.paths.len() - 1];
                    self.folder_editor.photos = indices.iter().copied().collect();
                    assert!(self.folder_editor.stage(&indices, &target));
                    self.filter_dirty = true;
                    self.zoom = false;
                }
                14 if folders => {
                    let parent = self.paths[0].parent().unwrap().parent().unwrap().to_owned();
                    self.folder_editor.new_dialog(parent);
                }
                15 if folders => {
                    self.folder_editor.dismiss_dialog();
                    let target = self
                        .folder_editor
                        .plan
                        .moves
                        .values()
                        .next()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .to_owned();
                    self.folder_editor.move_dialog(
                        self.folder_editor.selection(self.current, &self.visible),
                        target,
                    );
                }
                16 if folders => {
                    self.folder_editor.dismiss_dialog();
                    self.folder_editor.pending_dialog();
                }
                17 if folders => {
                    self.folder_editor.dismiss_dialog();
                    self.folder_editor.selected = Some(
                        self.folder_editor
                            .plan
                            .moves
                            .values()
                            .next()
                            .unwrap()
                            .parent()
                            .unwrap()
                            .to_owned(),
                    );
                    self.filter_dirty = true;
                }
                18 if folders => {
                    assert_eq!(self.visible.len(), 2);
                    self.close_dialog = true;
                }
                _ => {}
            }
            let s = self.smoke.as_mut().unwrap();
            s.prepared = true;
            s.last = Instant::now();
            ctx.request_repaint();
            return;
        }
        // Capture the next rendered frame after these state changes.
        if std::env::var_os("FASTCULL_TREE_SMOKE").is_some() && (step == 1 || step == 2) {
            let cursor = ctx.output(|o| o.cursor_icon);
            assert_eq!(
                cursor,
                if step == 1 {
                    egui::CursorIcon::Move
                } else {
                    egui::CursorIcon::NotAllowed
                }
            );
            assert!(self.folder_editor.take_requests().is_empty());
            log::info!("native folder-tree drag step={step} cursor={cursor:?}; no move submitted");
        }
        let s = self.smoke.as_mut().unwrap();
        s.waiting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        ctx.request_repaint();
    }
}
/// A group of related controls.
///
/// The group takes the width its content needs: a fixed width left holes in
/// the toolbar whenever the content was narrower than the reservation.
fn control_group(ui: &mut egui::Ui, controls: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.;
        controls(ui);
    });
}

/// Lay a toolbar group out off-screen to learn the width it needs.
///
/// egui cannot wrap a nested group: `ui.horizontal()` asks for the whole
/// remaining width, so a group whose content is wider than the rest of the
/// row is cut off at the window edge instead of moving to the next row.
/// Measuring the group first lets the toolbar place it with the width it
/// needs, and the toolbar can then move the whole group down.
fn measure_group(
    ctx: &egui::Context,
    spacing: &egui::Spacing,
    id: &'static str,
    build: impl FnOnce(&mut egui::Ui),
) -> f32 {
    let rect = Rect::from_min_size(Pos2::new(-20_000., -20_000.), Vec2::new(10_000., 100.));
    let mut ui = egui::Ui::new(
        ctx.clone(),
        egui::Id::new(id),
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .style(ctx.style()),
    );
    *ui.spacing_mut() = spacing.clone();
    ui.spacing_mut().item_spacing.x = 4.;
    build(&mut ui);
    ui.min_rect().width() + 1.
}

/// Place a toolbar group with the width it measured.
///
/// The group asks for its own width, so the toolbar can wrap it to the next
/// row when the rest of the row is too narrow for it.
fn control_group_sized(ui: &mut egui::Ui, width: f32, controls: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.;
            controls(ui);
        },
    );
}

/// A colour label chip.
///
/// An unset chip is a hollow square in the colour itself; a set one is filled
/// and carries a check. The two states differ in shape and not only in shade,
/// so the chip still reads without colour vision.
fn colour_chip(ui: &mut egui::Ui, enabled: bool, color: Color32, selected: bool) -> egui::Response {
    let color = if enabled {
        color
    } else {
        color.gamma_multiply(0.45)
    };
    let chip = egui::Button::new("")
        .fill(if selected {
            color
        } else {
            color.gamma_multiply(0.16)
        })
        .stroke(if selected {
            Stroke::NONE
        } else {
            Stroke::new(1., color.gamma_multiply(0.72))
        })
        .min_size(Vec2::splat(20.))
        .corner_radius(theme::radius(theme::RADIUS_SM));
    let response = ui.add_enabled(enabled, chip);
    if selected {
        let rect = response.rect;
        ui.painter().add(egui::Shape::line(
            vec![
                rect.left_top() + Vec2::new(4.5, 10.5),
                rect.left_top() + Vec2::new(8.5, 14.5),
                rect.left_top() + Vec2::new(15.5, 5.5),
            ],
            Stroke::new(1.8, Color32::WHITE),
        ));
    }
    response
}

/// A visible 1px rule between two toolbar groups, with 8px on each side.
///
/// `ui.separator()` draws its line in the non-interactive colour, which is
/// almost invisible on the dark toolbar.
fn group_divider(ui: &mut egui::Ui) {
    let height = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(1. + 2. * theme::SP_S, height),
        egui::Sense::hover(),
    );
    let line = Rect::from_center_size(rect.center(), Vec2::new(1., height));
    ui.painter().rect_filled(line, 0., theme::line_2());
}
/// Where a status plate is anchored.
#[derive(Clone, Copy)]
enum StatusAnchor {
    /// The plate is centred on the anchor point.
    Center,
    /// The plate's top-left corner sits on the anchor point.
    TopLeft,
    /// The plate's bottom-left corner sits on the anchor point.
    BottomLeft,
}

/// Paint a status message on a translucent rounded plate.
///
/// The messages sit on top of the photo, so plain text was unreadable on a
/// bright frame. The plate keeps the text legible without hiding the image.
fn status_plate(
    painter: &egui::Painter,
    at: Pos2,
    anchor: StatusAnchor,
    text: &str,
    size: f32,
    color: Color32,
) {
    let galley = painter.layout_no_wrap(text.to_owned(), theme::font(size), color);
    let pad = Vec2::splat(8.);
    let plate = galley.size() + pad * 2.;
    let min = match anchor {
        StatusAnchor::Center => at - plate / 2.,
        StatusAnchor::TopLeft => at,
        StatusAnchor::BottomLeft => at - Vec2::new(0., plate.y),
    };
    let rect = Rect::from_min_size(min, plate);
    painter.rect_filled(rect, theme::RADIUS_SM, Color32::from_black_alpha(200));
    painter.galley(rect.left_top() + pad, galley, color);
}

/// The height of the status bar at the foot of the filter panel.
const SIDEBAR_FOOTER: f32 = 56.;

/// The status bar at the foot of the filter panel.
///
/// The panel used to stop at the last facet and leave the rest of the card
/// blank. The bar says how much of the set the current filters leave and how
/// the photos are marked, and it carries the reset action next to the numbers
/// it would undo. It returns whether the reset was asked for.
fn sidebar_status(
    ui: &mut egui::Ui,
    status: SidebarStatus,
    can_reset: bool,
) -> bool {
    let rect = ui.max_rect();
    ui.painter().rect_filled(
        Rect::from_min_size(rect.left_top(), Vec2::new(rect.width(), 1.)),
        0.,
        theme::line(),
    );
    // The bar has a fixed height, so the theme's item spacing would push the
    // last row past the panel edge and clip it.
    ui.spacing_mut().item_spacing.y = 0.;
    ui.add_space(9.);
    let mut reset = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("显示 {} / {} 张", status.shown, status.total))
                .size(theme::FS_BODY)
                .color(theme::text()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(can_reset, theme::ghost_small("重置"))
                .on_hover_text("清除全部筛选条件")
                .clicked()
            {
                reset = true;
            }
        });
    });
    // The marks reuse the colours of the toolbar, so the two places that
    // talk about a mark look the same. They are counts, so they take the
    // small size and stay clear of the panel edge.
    let mut job = egui::text::LayoutJob::default();
    for (label, count, color) in [
        ("拾取 ", status.picks, theme::success()),
        ("   拒绝 ", status.rejects, theme::danger()),
        ("   评分 ", status.rated, theme::star()),
    ] {
        egui::RichText::new(label)
            .size(theme::FS_SMALL)
            .color(theme::text_muted())
            .append_to(
                &mut job,
                ui.style(),
                egui::FontSelection::Default,
                egui::Align::Center,
            );
        egui::RichText::new(count.to_string())
            .size(theme::FS_SMALL)
            .color(color)
            .strong()
            .append_to(
                &mut job,
                ui.style(),
                egui::FontSelection::Default,
                egui::Align::Center,
            );
    }
    ui.add_space(2.);
    ui.label(egui::WidgetText::from(job));
    reset
}

/// What the filter panel's status bar reports.
#[derive(Clone, Copy)]
struct SidebarStatus {
    /// How many photos the current filters leave.
    shown: usize,
    /// How many photos there are in all.
    total: usize,
    picks: usize,
    rejects: usize,
    rated: usize,
}

/// Draw a five-pointed star.
///
/// A star inside its own chip read as a row of five buttons. Drawn free of any
/// frame it reads as a rating, and the empty slot keeps the row's rhythm.
fn draw_star(painter: &egui::Painter, center: Pos2, radius: f32, filled: bool, color: Color32) {
    let mut points = Vec::with_capacity(10);
    for i in 0..10 {
        let r = if i % 2 == 0 { radius } else { radius * 0.42 };
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.;
        points.push(center + Vec2::new(r * angle.cos(), r * angle.sin()));
    }
    if filled {
        // A star is concave, so it cannot be filled as a convex polygon: egui
        // fans the triangles from the first point, and the five notches fill
        // in with five flaps across the shape. The star is filled as a ring of
        // triangles around its centre instead.
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(center, color);
        for point in &points {
            mesh.colored_vertex(*point, color);
        }
        for i in 0..points.len() {
            let a = 1 + i as u32;
            let b = 1 + ((i + 1) % points.len()) as u32;
            mesh.indices.extend_from_slice(&[0, a, b]);
        }
        painter.add(egui::Shape::mesh(mesh));
        // The outline keeps the edge of the shape readable against a light
        // surface.
        let mut outline = points.clone();
        outline.push(points[0]);
        painter.add(egui::Shape::line(outline, Stroke::new(1., color)));
    } else {
        // The outline has to close: a polyline alone leaves the last edge
        // open, which shows as a notch at the top point.
        let mut outline = points.clone();
        outline.push(points[0]);
        painter.add(egui::Shape::line(outline, Stroke::new(1.0, color)));
    }
}

pub(crate) fn rating_control(ui: &mut egui::Ui, rating: &mut u8, enabled: bool) -> bool {
    let before = *rating;
    // Every star is a 22×22 chip, so the click target is visible in the
    // toolbar instead of a bare glyph. A rated chip is filled with ACCENT and
    // an empty one keeps an ACCENT outline, which keeps the rating a primary
    // looking control even when nothing is rated yet.
    let padding = ui.spacing().button_padding;
    let spacing = ui.spacing().item_spacing;
    ui.spacing_mut().button_padding = Vec2::new(2., padding.y);
    ui.spacing_mut().item_spacing.x = 2.;
    for r in 1..=5 {
        let rated = r <= *rating;
        // The click target is invisible: the star itself carries the state.
        let response = ui.add_enabled(
            enabled,
            egui::Button::new("")
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::NONE)
                .frame_when_inactive(false)
                .min_size(Vec2::splat(24.)),
        );
        let rect = response.rect;
        let color = if rated {
            theme::star()
        } else if response.hovered() {
            theme::text_secondary()
        } else {
            theme::text_muted()
        };
        draw_star(&ui.painter_at(rect), rect.center(), 9., rated, color);
        if response
            .on_hover_text(format!("{r} 星（{r}），再次点击清星"))
            .clicked()
        {
            *rating = if r == *rating { 0 } else { r };
        }
    }
    ui.spacing_mut().button_padding = padding;
    ui.spacing_mut().item_spacing = spacing;
    // The readout stands clear of the stars instead of touching the last one.
    ui.add_space(6.);
    // The readout keeps the unit while it fits; a narrow toolbar drops it
    // rather than pushing the number past the edge of the window.
    let room = ui.available_width();
    let readout = if room >= 24. {
        format!("{rating} 星")
    } else if room >= 10. {
        format!("{rating}")
    } else {
        String::new()
    };
    if !readout.is_empty() {
        ui.label(
            egui::RichText::new(readout)
                .size(theme::FS_SMALL)
                .color(theme::text_secondary()),
        );
    }
    *rating != before
}
#[derive(Clone, Copy)]
enum CloseChoice {
    Sync,
    Direct,
    Cancel,
}
fn close_confirmation(
    ctx: &egui::Context,
    session: bool,
    organizing: bool,
    error: Option<&str>,
) -> Option<CloseChoice> {
    let mut choice = None;
    egui::Modal::new(egui::Id::new("close-confirmation"))
        .frame(egui::Frame::popup(&ctx.style()).inner_margin(22.))
        .show(ctx, |ui| {
            ui.set_width(440_f32.min((ctx.content_rect().width() - 88.).max(240.)));
            ui.spacing_mut().item_spacing = Vec2::new(12., 10.);
            ui.label(egui::RichText::new("关闭选片器").size(theme::FS_DISPLAY).strong());
            ui.add_space(4.);
            ui.label(egui::RichText::new(if !session {
                "星级、旗标、颜色、曝光、裁切与旋转已保存到照片元数据。"
            } else if organizing {
                "是否同步本次修改及文件整理计划到 Lightroom？"
            } else { "退出后自动同步本次全部修改到 Lightroom。" }).size(theme::FS_TITLE));
            ui.label(
                egui::RichText::new(if !session { "关闭后修改仍会保留。\n在其他软件中查看时，请刷新或重新读取照片元数据。" }
                    else if organizing { "同步星级、旗标、颜色、曝光、裁切与旋转，并建立待整理分组。\n实际移动请在 LR 文件夹面板完成，保留原照片记录。" }
                    else { "包含星级、旗标、颜色、曝光、裁切与旋转。\n只同步本次修改的字段，保留其他 LR 显影设置。" })
                    .size(theme::FS_LABEL)
                    .color(theme::text_secondary()),
            );
            ui.add_space(10.);
            ui.horizontal_wrapped(|ui| {
                for (text, action, width, enabled) in [
                    (if organizing { "同步并整理后关闭" } else { "同步全部并关闭" }, CloseChoice::Sync, 176., session),
                    (if session { "仅关闭，稍后同步" } else { "直接关闭" }, CloseChoice::Direct, 152., true),
                    ("取消", CloseChoice::Cancel, 88., true),
                ] {
                    if matches!(action, CloseChoice::Sync) && !session { continue; }
                    let button = egui::Button::new(egui::RichText::new(text).size(theme::FS_BODY))
                        .min_size(Vec2::new(width, 40.));
                    if ui.add_enabled(enabled, button).clicked() {
                        choice = Some(action);
                    }
                }
            });
            if let Some(e) = error {
                ui.colored_label(theme::danger(), egui::RichText::new(e).size(theme::FS_LABEL));
            }
        });
    choice
}
#[cfg(test)]
fn shortcut(ctx: &egui::Context, key: Key) -> bool {
    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key))
}
impl eframe::App for App {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        // Native visual regression on private fixtures only; no actual drop.
        if std::env::var_os("FASTCULL_TREE_SMOKE").is_none() {
            return;
        }
        let Some(smoke) = &self.smoke else {
            return;
        };
        let step = smoke.step;
        let Some(parent) = self
            .paths
            .get(if step == 1 { 10 } else { 0 })
            .and_then(|p| p.parent())
        else {
            return;
        };
        let Some(row) = ctx.read_response(egui::Id::new(("folder-row", &parent.to_path_buf())))
        else {
            return;
        };
        if step == 1 || step == 2 {
            if !egui::DragAndDrop::has_payload_of_type::<Vec<usize>>(ctx) {
                egui::DragAndDrop::set_payload(ctx, vec![0usize]);
                input.events.push(egui::Event::PointerButton {
                    pos: row.rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            input
                .events
                .push(egui::Event::PointerMoved(row.rect.center()));
        } else {
            egui::DragAndDrop::clear_payload(ctx);
            input.events.push(egui::Event::PointerMoved(if step == 0 {
                row.rect.right_center() + Vec2::new(10., 0.)
            } else {
                egui::pos2(500., 30.)
            }));
            if step == 3 {
                input.events.push(egui::Event::PointerButton {
                    pos: egui::pos2(500., 30.),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(p) = &self.exposure_preview {
            p.destroy();
        }
    }
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        crate::appearance::sync_native_theme(ctx);
        theme::refresh(ctx);
        self.folder_editor.drop_cursor = None;
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && self.smoke.is_none()
            && std::env::var_os("FASTCULL_RAW_EDIT_SMOKE_DIR").is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_dialog = true;
        }
        self.full.pinned = self.compare.as_ref().map(|c| c.indices[1 - c.active]);
        self.pool.reference(self.full.pinned);
        self.fit.pinned = self.full.pinned;
        self.startup_focus.update(ctx, frame);
        if !self.first_frame {
            log::info!(
                "first UI frame {:.1}ms",
                self.started.elapsed().as_secs_f64() * 1000.
            );
            self.first_frame = true;
            ctx.request_repaint();
        }
        self.receive_moves(ctx);
        self.receive_metadata();
        self.focus_overlay.shortcut(ctx);
        self.receive_develop();
        self.receive_index();
        // Start the session editor while the user is still examining the frame.
        // Its window/device/pipeline initialization is the dominant cold-open
        // cost; the open itself then reuses the same private stdin pipe.
        let prewarm_eligible = !self.raw_prewarmed
            && !self.raw_windows.busy()
            && self
                .photos
                .get(self.current)
                .is_some_and(|photo| photo.loaded)
            && self
                .paths
                .get(self.current)
                .is_some_and(|path| crate::raw_editor::supported(path));
        if prewarm_eligible {
            let delay = std::env::var("FASTCULL_RAW_PREWARM_DELAY_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(800);
            let started = *self.raw_prewarm_at.get_or_insert_with(Instant::now);
            if started.elapsed() >= std::time::Duration::from_millis(delay) {
                self.raw_prewarmed = true;
                self.raw_windows.prewarm();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        } else if !self.raw_prewarmed && !self.raw_windows.busy() {
            self.raw_prewarm_at = None;
        }
        if std::env::var_os("FASTCULL_RAW_EDIT_SMOKE_DIR").is_some()
            && !self.raw_smoke_started
            && self.photos[self.current].loaded
        {
            let delay = std::env::var("FASTCULL_RAW_SMOKE_DELAY_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            let started = *self.raw_smoke_at.get_or_insert_with(Instant::now);
            if started.elapsed() >= std::time::Duration::from_millis(delay) {
                self.raw_smoke_started = true;
                self.begin_raw(ctx);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        self.receive_raw();
        let raw_busy = self.raw_windows.busy();
        self.raw_parent_lock.update(frame, raw_busy);
        if raw_busy {
            self.close_dialog = false;
            ctx.input_mut(|i| i.events.clear());
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            egui::CentralPanel::default()
            // No inner margin: the canvas gradient has to reach the window
            // edge, otherwise a flat ring of frame colour shows around it.
            .frame(
                egui::Frame::new()
                    .fill(theme::canvas())
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label("RAW 编辑中 · 请在独立编辑窗口完成操作\n关闭编辑窗口时自动保存 XMP，之后恢复选片器");
                });
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        if self.filter_dirty {
            self.refilter();
        }
        self.keyboard(ctx);
        self.receive(ctx);
        if self.crop_requested {
            self.begin_crop();
        }
        if self.crop_editor.is_some() {
            self.crop_workspace(ctx);
            self.smoke(ctx);
            return;
        }
        if self.filter_dirty {
            self.refilter();
        }
        // The window is a set of bands, not a pile of cards: the rail and the
        // sidebar are recessed, the title bar, the toolbar and the filmstrip
        // are raised, and the canvas between them is the darkest surface in
        // the window. The depth of the interface is carried by that value
        // ladder, not by a shadow under every rectangle.
        egui::SidePanel::left("activity-rail")
            .exact_width(64.)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::band())
                    .inner_margin(egui::Margin { left: 8, right: 0, top: 10, bottom: 0 }),
            )
            .show(ctx, |ui| {
                if navigation_button(
                    ui,
                    "文件夹",
                    self.folder_editor.open,
                    NavigationIcon::Folder,
                )
                .on_hover_text("文件夹树与照片整理")
                .clicked()
                {
                    self.folder_editor.open = !self.folder_editor.open;
                    if self.folder_editor.open {
                        self.filter_editor.open = false;
                    }
                }
                ui.add_space(4.);
                if navigation_button(ui, "筛选", self.filter_editor.open, NavigationIcon::Filter)
                    .on_hover_text("照片筛选 · Ctrl/Cmd+F")
                    .clicked()
                {
                    self.filter_editor.open = !self.filter_editor.open;
                    if self.filter_editor.open {
                        self.folder_editor.open = false;
                    }
                }
                ui.add_space(4.);
                let crop_enabled = !self.visible.is_empty()
                    && self.photos[self.current].loaded
                    && self.photos[self.current].metadata_error.is_none()
                    && !self.moving(self.current);
                if ui
                    .add_enabled_ui(crop_enabled, |ui| {
                        navigation_button(ui, "裁切", self.crop_requested, NavigationIcon::Crop)
                            .on_hover_text("裁切 · T\n自由裁切、锁定比例与旋转")
                    })
                    .inner
                    .clicked()
                {
                    self.begin_crop();
                }
                if self.filters.active() {
                    ui.add_space(10.);
                    // A pill rather than a floating line of coloured text.
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(44., 20.), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 10., theme::accent_soft());
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "已筛选",
                        theme::font(theme::FS_CAPTION),
                        theme::accent(),
                    );
                }
                ui.add_space(4.);
                let raw_enabled =
                    crop_enabled && crate::raw_editor::supported(&self.paths[self.current]);
                if ui
                    .add_enabled_ui(raw_enabled, |ui| {
                        navigation_button(ui, "RAW 编辑", false, NavigationIcon::Raw).on_hover_text(
                            "独立 RAW 窗口 · 本地 LibRaw · 曝光、白平衡与色调 · sRGB",
                        )
                    })
                    .inner
                    .on_disabled_hover_text("当前照片暂不能编辑")
                    .clicked()
                {
                    self.begin_raw(ctx);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    let id = egui::Id::new("culling-settings");
                    let response = navigation_button(
                        ui,
                        "设置",
                        egui::Popup::is_id_open(ctx, id),
                        NavigationIcon::Settings,
                    )
                    .on_hover_text("外观与放大浏览设置");
                    egui::Popup::menu(&response)
                        .id(id)
                        .align(egui::RectAlign::RIGHT_END)
                        .show(|ui| self.settings.menu(ui));
                });
                // The rail ends in a hairline: the panels beside it are made
                // of another material.
                let rect = ui.max_rect();
                ui.painter().vline(
                    rect.right() - 0.5,
                    rect.y_range(),
                    Stroke::new(1., theme::line()),
                );
            });
        egui::TopBottomPanel::top("hud")
            .min_height(50.)
            .frame(
                egui::Frame::new()
                    .fill(theme::raised())
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| {
                ui.add_space(9.);
                let width = ui.available_width();
                let row = 32.;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    // Left: a new window, and where this photo sits in the set.
                    ui.allocate_ui_with_layout(
                        Vec2::new(width * 0.26, row),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if self.session.is_none()
                                && ui
                                    .add(theme::ghost("＋ 新窗口"))
                                    .on_hover_text("打开另一批照片或文件夹")
                                    .clicked()
                            {
                                match std::env::current_exe().and_then(|exe| {
                                    std::process::Command::new(exe)
                                        .stdin(std::process::Stdio::null())
                                        .stdout(std::process::Stdio::null())
                                        .stderr(std::process::Stdio::null())
                                        .spawn()
                                }) {
                                    Ok(_) => {}
                                    Err(e) => self.error = Some(format!("打开新窗口失败：{e}")),
                                }
                            }
                            let index = self
                                .visible
                                .iter()
                                .position(|i| *i == self.current)
                                .map_or(0, |i| i + 1);
                            let visible = self.visible.len();
                            let total = self.paths.len();
                            ui.label(
                                egui::RichText::new(format!("{index}"))
                                    .size(theme::FS_LABEL)
                                    .strong()
                                    .color(theme::accent()),
                            );
                            ui.label(
                                egui::RichText::new(format!("/ {visible}"))
                                    .size(theme::FS_SMALL)
                                    .color(theme::text_muted()),
                            );
                            if visible != total {
                                ui.label(
                                    egui::RichText::new(format!("共 {total} 张"))
                                        .size(theme::FS_CAPTION)
                                        .color(theme::text_muted()),
                                );
                            }
                        },
                    );
                    // Centre: the file name. The bar is about the photo, so
                    // the name is centred in the window and is the largest
                    // type in the interface.
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(width * 0.48, row), egui::Sense::hover());
                    if !self.visible.is_empty() {
                        let name = self.paths[self.current]
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let name = fit_to_width(
                            ui.painter(),
                            &name,
                            &theme::font(theme::FS_HERO),
                            theme::text(),
                            rect.width(),
                        );
                        let galley =
                            ui.painter()
                                .layout_no_wrap(name, theme::font(theme::FS_HERO), theme::text());
                        let size = galley.size();
                        ui.painter().galley(
                            rect.center() - size / 2. - Vec2::new(0., size.y * 0.06),
                            galley,
                            theme::text(),
                        );
                        let _ = response
                            .on_hover_text(self.paths[self.current].display().to_string());
                    }
                    // Right: the capture summary. The camera, lens and capture
                    // time stay in the tooltip.
                    ui.allocate_ui_with_layout(
                        Vec2::new(width * 0.26, row),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if self.visible.is_empty() {
                                return;
                            }
                            let (summary, hint) = match &self.photos[self.current].info {
                                Some(info) => {
                                    let capture = &info.capture;
                                    let mut details = Vec::new();
                                    if let Some(iso) = capture.iso {
                                        details.push(format!("ISO {iso}"));
                                    }
                                    if let Some(aperture) = capture.aperture {
                                        details.push(format!("f/{aperture:.1}"));
                                    }
                                    if let Some(shutter) = capture.shutter.filter(|v| *v > 0.) {
                                        details.push(if shutter < 1. {
                                            format!("1/{:.0}s", 1. / shutter)
                                        } else {
                                            format!("{shutter:.1}s")
                                        });
                                    }
                                    if let Some(focal) = capture.focal {
                                        details.push(format!("{focal:.0}mm"));
                                    }
                                    let summary = if details.is_empty() {
                                        "未记录拍摄参数".to_owned()
                                    } else {
                                        details.join(" · ")
                                    };
                                    (
                                        summary,
                                        format!(
                                            "{}\n{}\n{}",
                                            capture.camera.as_deref().unwrap_or(""),
                                            capture.lens.as_deref().unwrap_or(""),
                                            capture.captured_at.as_deref().unwrap_or("")
                                        ),
                                    )
                                }
                                None => ("正在读取拍摄信息…".to_owned(), String::new()),
                            };
                            let summary = fit_to_width(
                                ui.painter(),
                                &summary,
                                &theme::font(theme::FS_SMALL),
                                theme::text_secondary(),
                                ui.available_width() - 8.,
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(summary)
                                        .size(theme::FS_SMALL)
                                        .color(theme::text_secondary()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(hint);
                        },
                    );
                });
                if let Some(e) = &self.error {
                    ui.colored_label(theme::danger(), e);
                }
                if self.close_after_moves {
                    ui.label("正在完成移动，完成后自动关闭选片器");
                }
                if let Some(e) = &self.photos[self.current].metadata_error {
                    ui.colored_label(
                        theme::danger(),
                        format!(
                            "元数据读取/初始化失败，当前图片仍可浏览；评分暂不可用。按 R 重试：{e}"
                        ),
                    );
                }
                if let Some(session) = &self.session {
                    let stats = session.stats();
                    let done = stats.wrote + stats.kept + stats.error;
                    if stats.error > 0 {
                        ui.colored_label(
                            theme::danger(),
                            format!("{} 张照片未能读入 LR 评分，详情见日志", stats.error),
                        );
                    } else if done < session.len() {
                        ui.label(format!("后台读取 LR 评分：{done}/{}", session.len()));
                    }
                }
                // The bar ends in a hairline: the canvas below it is made of
                // another material.
                let rect = ui.max_rect();
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    Stroke::new(1., theme::line()),
                );
            });
        if self.filter_editor.open || self.folder_editor.open {
            if self.filter_editor.open {
                self.pool.start_index();
            }
            let rows: Vec<_> = self
                .paths
                .iter()
                .zip(&self.photos)
                .map(|(path, p)| fastcull::filtering::Row {
                    path,
                    values: &p.values,
                    capture: p.info.as_ref().map(|v| &v.capture),
                    indexed: p.capture_indexed || p.info.is_some(),
                })
                .collect();
            // A narrow window cannot spare 260 px for the sidebar: at 640 px
            // it took 40% of the width and squeezed the canvas into two thin
            // grid columns.
            let narrow = ctx.content_rect().width() < 780.;
            // The status bar reads the set as it is now, before the panel is
            // laid out: the filters are applied after the panels close.
            let status = SidebarStatus {
                shown: self.visible.len(),
                total: self.paths.len(),
                picks: self
                    .visible
                    .iter()
                    .filter(|i| self.photos[**i].values.flag.as_deref() == Some("pick"))
                    .count(),
                rejects: self
                    .visible
                    .iter()
                    .filter(|i| self.photos[**i].values.flag.as_deref() == Some("reject"))
                    .count(),
                rated: self
                    .visible
                    .iter()
                    .filter(|i| self.photos[**i].values.rating.unwrap_or(0) > 0)
                    .count(),
            };
            let can_reset = self.filters.active();
            egui::SidePanel::left("filters")
                .default_width(if narrow { 208. } else { 272. })
                .width_range(if narrow { 190.0..=260.0 } else { 230.0..=360.0 })
                .frame(
                    egui::Frame::new()
                        .fill(theme::band())
                        .inner_margin(egui::Margin { left: 16, right: 0, top: 14, bottom: 0 }),
                )
                .show(ctx, |ui| {
                    // The panel is split in two: the filter tree takes the room
                    // it needs, and a status bar sits on the floor of the
                    // panel. The space under the last facet used to be blank;
                    // it now says what the current selection holds.
                    let full = ui.max_rect();
                    let footer = Rect::from_min_max(
                        Pos2::new(full.left(), full.bottom() - SIDEBAR_FOOTER),
                        Pos2::new(full.right() - 16., full.bottom()),
                    );
                    let body = Rect::from_min_max(
                        full.left_top(),
                        Pos2::new(full.right() - 16., footer.top() - 10.),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                        if self.folder_editor.open {
                            let height = if self.filter_editor.open {
                                ui.available_height() * 0.35
                            } else {
                                ui.available_height() - 220.
                            };
                            if self
                                .folder_editor
                                .show(ui, self.current, &self.visible, height)
                            {
                                self.filter_dirty = true;
                            }
                            if self.filter_editor.open {
                                ui.separator();
                            }
                        }
                        if self.filter_editor.open {
                            egui::ScrollArea::vertical()
                                .id_salt("metadata-filters")
                                .show(ui, |ui| {
                                    if self.filter_editor.show(ui, &mut self.filters, &rows) {
                                        self.filter_dirty = true;
                                    }
                                });
                        }
                    });
                    let reset = ui
                        .scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
                            sidebar_status(ui, status, can_reset)
                        })
                        .inner;
                    if reset {
                        self.filter_editor.clear(&mut self.filters);
                        self.filter_dirty = true;
                    }
                    // The panel ends in a hairline: the canvas beside it is
                    // made of another material.
                    let rect = ui.max_rect();
                    ui.painter().vline(
                        rect.right() - 0.5,
                        rect.y_range(),
                        Stroke::new(1., theme::line()),
                    );
                });
        }
        if self.filter_dirty {
            self.refilter();
        }
        self.film_panel(ctx);
        egui::TopBottomPanel::bottom("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(theme::raised())
                    .inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 8 }),
            )
            .show(ctx, |ui| {
                ui.add_space(9.);
                self.toolbar(ui);
                ui.add_space(9.);
                // The toolbar begins in a hairline: it is a shelf of its own.
                let rect = ui.max_rect();
                ui.painter().hline(
                    rect.x_range(),
                    rect.top() + 0.5,
                    Stroke::new(1., theme::line()),
                );
            });
        if self.filter_dirty {
            self.refilter();
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| self.central(ui, ctx));
        if let Some(e) = self.exposure_preview.as_ref().and_then(|p| p.error()) {
            self.error = Some(format!("曝光预览不可用，参数仍可保存：{e}"));
        }
        if self.folder_editor.dialogs(ctx) {
            self.filter_dirty = true;
        }
        self.submit_moves();
        if self.close_dialog {
            match close_confirmation(
                ctx,
                self.session.is_some(),
                !self.folder_editor.plan.moves.is_empty(),
                self.error.as_deref(),
            ) {
                Some(CloseChoice::Sync) => self.finish_close(ctx, true),
                Some(CloseChoice::Direct) => self.finish_close(ctx, false),
                Some(CloseChoice::Cancel) => self.close_dialog = false,
                None => {}
            }
        }
        // Foreground first; small preview work for visible cells; full prefetch only while loupe is active.
        if !self.grid && !self.visible.is_empty() {
            self.request(self.current, Level::Fit(self.fit_edge));
            if self.zoom {
                self.request(self.current, Level::Full);
            }
            // A brief pause on an already visible photo warms 100% on the
            // background worker. The foreground decoder remains available for
            // the next photo, even when this RAW needs sensor rendering.
            if !self.zoom
                && self.compare.is_none()
                && self.fit.items.contains_key(&self.current)
                && !self.full.items.contains_key(&self.current)
                && !self.photos[self.current]
                    .image_errors
                    .contains_key(&Level::Full)
            {
                let idle = self.navigation_at.elapsed();
                let delay = std::time::Duration::from_millis(300);
                if idle >= delay {
                    self.pool.warm_full(self.current);
                } else {
                    ctx.request_repaint_after(delay - idle);
                }
            }
            let level = if self.zoom {
                Level::Full
            } else {
                Level::Fit(self.fit_edge)
            };
            let offsets: &[isize] = if self.compare.is_some() {
                &[]
            } else if self.zoom {
                &[1, -1]
            } else {
                &[1, -1, 2, 3]
            };
            for &step in offsets {
                if let Some(i) = fastcull::filtering::step_visible(
                    &self.visible,
                    self.current,
                    self.direction * step,
                ) && i != self.current
                    && self.prefetched.insert((i, level))
                {
                    self.request(i, level);
                }
            }
        }
        if egui::DragAndDrop::has_payload_of_type::<Vec<usize>>(ctx) {
            ctx.set_cursor_icon(
                self.folder_editor
                    .drop_cursor
                    .unwrap_or(egui::CursorIcon::NotAllowed),
            );
        }
        // Opt-in end-to-end probe: lets the test observe input processing while
        // the independent RAW process is decoding, without timing GUI screenshots.
        if let Some(path) = std::env::var_os("FASTCULL_RAW_WINDOW_QA_FILE") {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let _ = std::fs::write(path, format!("{stamp}\t{}\t{}", self.current, self.grid));
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.smoke(ctx);
    }
}
fn preview_edge(size: Vec2, pixels_per_point: f32) -> u32 {
    // Keep 25% headroom over physical display pixels, with stable buckets so
    // resizing does not schedule a new decode for every single pixel change.
    let pixels = size.x.max(size.y) * pixels_per_point * 1.25;
    ((pixels / 256.).ceil() as u32 * 256).clamp(1024, 4096)
}
#[allow(clippy::too_many_arguments)]
/// Where a photo ends up on the canvas.
///
/// `rect` is the photo's screen rectangle and `uvs` the texture corners that
/// belong to it, in the order the mesh expects them. The layout maths is the
/// one the app has always used: only the area the photo is fitted into
/// changed, so zoom, pan and the focus overlay keep their coordinates.
struct PhotoLayout {
    rect: Rect,
    uvs: [Pos2; 4],
}

/// The view state a photo is drawn with.
///
/// The zoom factor, the pan offset and the pixel ratio they are measured in
/// always travel together, so they travel as one value.
#[derive(Clone, Copy)]
struct PhotoView {
    /// The zoom factor; 0 means "fit".
    zoom: f32,
    /// The pan offset, in photo pixels.
    pan: Vec2,
    /// Pixels per point.
    ppp: f32,
}

fn photo_layout(
    t: &TextureHandle,
    area: Rect,
    orientation: u16,
    view: PhotoView,
    native_size: Option<Vec2>,
    crop: Option<fastcull::crop::Crop>,
) -> PhotoLayout {
    let original = native_size.unwrap_or_else(|| t.size_vec2());
    let crop = crop.unwrap_or_default();
    let size = crop.geometry(original, orientation).rect.size();
    let scale = if view.zoom > 0. {
        view.zoom / view.ppp
    } else {
        (area.width() / size.x).min(area.height() / size.y)
    };
    let extent = (size * scale - area.size()).max(Vec2::ZERO) * 0.5;
    let pan = Vec2::new(
        view.pan.x.clamp(-extent.x, extent.x),
        view.pan.y.clamp(-extent.y, extent.y),
    );
    let rect = Rect::from_center_size(area.center() + pan, size * scale);
    let source = crop.raw_uvs(original);
    let uvs = fastcull::crop::order(orientation).map(|i| source[i]);
    PhotoLayout { rect, uvs }
}

/// Paint a laid out photo.
///
/// `canvas` is the area the GPU exposure preview is clipped to. It is the
/// whole canvas, not the padded area the photo was fitted into, so a photo
/// that reaches past the padding is still previewed over its whole area.
fn paint_photo(
    painter: &egui::Painter,
    t: &TextureHandle,
    canvas: Rect,
    layout: &PhotoLayout,
    preview: Option<(Arc<crate::exposure_preview::Preview>, f32)>,
) {
    paint_uv(painter, t, layout.rect, layout.uvs);
    if let Some((preview, delta)) = preview {
        preview.paint(painter, canvas, layout.rect, t.id(), layout.uvs, delta);
    }
}

/// The soft shadow below a photo. It has to be painted before the photo.
fn paint_photo_shadow(painter: &egui::Painter, screen: Rect) {
    let radius = theme::radius(theme::RADIUS_PHOTO);
    // Two layers: a wide ambient shadow that lifts the photo off the canvas
    // and a tight contact shadow that anchors it. One flat shadow read as a
    // grey outline instead of depth.
    let ambient = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 36,
        spread: 0,
        color: theme::shadow_ambient(),
    };
    painter.add(ambient.as_shape(screen, radius));
    let contact = egui::epaint::Shadow {
        offset: [0, 2],
        blur: 9,
        spread: 0,
        color: theme::shadow_contact(),
    };
    painter.add(contact.as_shape(screen, radius));
}

/// The pool of light the photo stands in.
///
/// The canvas is the darkest surface in the window. A soft halo behind the
/// picture lifts it off that surface without drawing a frame around it.
fn paint_photo_glow(painter: &egui::Painter, screen: Rect) {
    let radius = theme::radius(theme::RADIUS_PHOTO);
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 0],
            blur: 80,
            spread: 26,
            color: theme::photo_glow(),
        }
        .as_shape(screen, radius),
    );
}

/// The canvas colour at a given height, matching `paint_canvas`.
fn canvas_color_at(rect: Rect, y: f32) -> Color32 {
    let t = if rect.height() > 0. {
        ((y - rect.top()) / rect.height()).clamp(0., 1.)
    } else {
        0.
    };
    theme::canvas_top().lerp_to_gamma(theme::canvas_bottom(), t)
}

/// Paint the photo canvas.
///
/// A flat fill made the photo look pasted onto a panel. A slow vertical
/// gradient gives the canvas a direction and lets the photo's shadow read.
fn paint_canvas(painter: &egui::Painter, rect: Rect) {
    let steps = 20;
    let height = rect.height() / steps as f32;
    for i in 0..steps {
        let t = i as f32 / (steps - 1) as f32;
        let top = rect.top() + height * i as f32;
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.left(), top),
                Pos2::new(rect.right(), top + height + 1.),
            ),
            0.,
            theme::canvas_top().lerp_to_gamma(theme::canvas_bottom(), t),
        );
    }
}

/// Round the photo's corners and give it a hairline border.
///
/// egui cannot clip a textured mesh to a rounded rectangle: the photo is a
/// quad with its own texture corners (crop and orientation), so the corners
/// are painted over with the canvas colour and the border follows the same
/// radius. A corner is only rounded while it is fully inside the canvas.
fn paint_photo_edge(painter: &egui::Painter, canvas: Rect, screen: Rect) {
    // A photo that runs past the canvas edge keeps its square corners there:
    // rounding them would cut the picture off against the window edge.
    let radius = theme::RADIUS_PHOTO
        .min(screen.width() * 0.5)
        .min(screen.height() * 0.5);
    round_image_corners(painter, screen, radius, &|corner, ex, ey| {
        let square = Rect::from_min_size(
            corner + Vec2::new(ex.x.min(0.) * radius, ey.y.min(0.) * radius),
            Vec2::splat(radius),
        );
        canvas
            .contains_rect(square)
            .then(|| canvas_color_at(canvas, corner.y))
    });
}

/// Round the corners of a painted image and give it a hairline border.
///
/// egui cannot clip a textured mesh to a rounded rectangle: the corners are
/// painted over with the colour that sits behind the image. `background` is
/// asked for that colour at each corner and may decline, which leaves the
/// corner square.
fn round_image_corners(
    painter: &egui::Painter,
    screen: Rect,
    radius: f32,
    background: &dyn Fn(Pos2, Vec2, Vec2) -> Option<Color32>,
) {
    let radius = radius.min(screen.width() * 0.5).min(screen.height() * 0.5);
    if radius < 1. {
        return;
    }
    for (corner, ex, ey) in [
        (screen.left_top(), Vec2::new(1., 0.), Vec2::new(0., 1.)),
        (screen.right_top(), Vec2::new(-1., 0.), Vec2::new(0., 1.)),
        (
            screen.right_bottom(),
            Vec2::new(-1., 0.),
            Vec2::new(0., -1.),
        ),
        (screen.left_bottom(), Vec2::new(1., 0.), Vec2::new(0., -1.)),
    ] {
        let Some(color) = background(corner, ex, ey) else {
            continue;
        };
        let mut points = vec![corner];
        for step in 0..=8 {
            let angle =
                -std::f32::consts::FRAC_PI_2 - (step as f32 / 8.) * std::f32::consts::FRAC_PI_2;
            let local = Vec2::new(radius + radius * angle.cos(), radius + radius * angle.sin());
            points.push(corner + local.x * ex + local.y * ey);
        }
        if signed_area(&points) < 0. {
            // Keep the corner first: the fill fans out from it.
            points[1..].reverse();
        }
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    }
    painter.rect_stroke(
        screen,
        theme::radius(radius),
        Stroke::new(1., theme::photo_rim()),
        egui::StrokeKind::Inside,
    );
}

/// Signed area of a polygon, positive when the points run clockwise.
fn signed_area(points: &[Pos2]) -> f32 {
    let mut area = 0.;
    for i in 0..points.len() {
        let p = points[i];
        let q = points[(i + 1) % points.len()];
        area += p.x * q.y - q.x * p.y;
    }
    area
}

/// Trim a label to the width it may use, so a long summary cannot push the
/// file name out of the top bar.
fn fit_to_width(
    painter: &egui::Painter,
    text: &str,
    font: &egui::FontId,
    color: Color32,
    max: f32,
) -> String {
    if max <= 0. {
        return String::new();
    }
    if painter
        .layout_no_wrap(text.to_owned(), font.clone(), color)
        .size()
        .x
        <= max
    {
        return text.to_owned();
    }
    let mut out = text.to_owned();
    while !out.is_empty() {
        out.pop();
        let mut candidate = out.clone();
        candidate.push('…');
        if painter
            .layout_no_wrap(candidate.clone(), font.clone(), color)
            .size()
            .x
            <= max
        {
            return candidate;
        }
    }
    String::new()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_uv(painter: &egui::Painter, t: &TextureHandle, rect: Rect, uvs: [Pos2; 4]) {
    let mut mesh = egui::Mesh::with_texture(t.id());
    for (pos, uv) in fastcull::crop::corners(rect).into_iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv,
            color: Color32::WHITE,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}
fn save_png(path: &std::path::Path, size: [usize; 2], rgba: &[u8]) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(file, size[0] as u32, size[1] as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}
enum NavigationIcon {
    Folder,
    Filter,
    Crop,
    Raw,
    Settings,
}
fn navigation_button(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    icon: NavigationIcon,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(48., 62.), egui::Sense::click());
    let painter = ui.painter();
    let accent = theme::accent();
    let color = if selected {
        accent
    } else {
        theme::text_secondary()
    };
    // The icon sits on a square plate. The selected item fills it, so the
    // rail reads as a set of buttons rather than four loose glyphs.
    let plate = Rect::from_center_size(
        rect.center_top() + Vec2::new(0., 19.),
        Vec2::splat(36.),
    );
    if selected {
        painter.rect_filled(plate, 10., theme::accent_soft());
    } else if response.hovered() {
        painter.rect_filled(plate, 10., theme::surface_2());
    }
    if selected {
        // The active item is marked in the rail itself: a short accent bar
        // against the edge of the window. The eye finds it without reading
        // the labels.
        let rail = ui.max_rect();
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(rail.left() - 5., plate.center().y - 11.),
                Vec2::new(3., 22.),
            ),
            2.,
            accent,
        );
    }
    let origin = plate.center() - Vec2::new(11., 11.);
    let stroke = Stroke::new(1.9, color);
    if let NavigationIcon::Raw = icon {
        // A framed picture reads as "the photo itself"; the old line chart
        // looked like a statistics panel.
        painter.rect_stroke(
            Rect::from_min_size(origin, Vec2::new(22., 17.)),
            2.,
            stroke,
            egui::StrokeKind::Inside,
        );
        painter.circle_filled(origin + Vec2::new(6., 5.), 2., color);
        painter.add(egui::Shape::line(
            vec![
                origin + Vec2::new(2., 15.),
                origin + Vec2::new(8., 8.),
                origin + Vec2::new(12., 12.),
                origin + Vec2::new(15., 8.),
                origin + Vec2::new(20., 15.),
            ],
            stroke,
        ));
    } else {
        let points = match icon {
            NavigationIcon::Raw => Vec::new(),
            NavigationIcon::Settings => {
                // A gear for the appearance menu: a ring with short teeth.
                let center = origin + Vec2::splat(11.);
                painter.circle_stroke(center, 4., Stroke::new(1.6, color));
                (0..=32)
                    .map(|step| {
                        let angle = std::f32::consts::TAU * step as f32 / 32.;
                        let radius = if matches!(step % 4, 1 | 2) { 11. } else { 8.5 };
                        center + Vec2::angled(angle) * radius
                    })
                    .collect()
            }
            NavigationIcon::Filter => vec![
                origin,
                origin + Vec2::new(22., 0.),
                origin + Vec2::new(13., 10.),
                origin + Vec2::new(13., 19.),
                origin + Vec2::new(9., 21.),
                origin + Vec2::new(9., 10.),
                origin,
            ],
            NavigationIcon::Folder => vec![
                origin + Vec2::new(0., 3.),
                origin + Vec2::new(8., 3.),
                origin + Vec2::new(11., 7.),
                origin + Vec2::new(23., 7.),
                origin + Vec2::new(23., 21.),
                origin + Vec2::new(0., 21.),
                origin + Vec2::new(0., 3.),
            ],
            NavigationIcon::Crop => {
                painter.add(egui::Shape::line(
                    vec![
                        origin + Vec2::new(0., 5.),
                        origin + Vec2::new(17., 5.),
                        origin + Vec2::new(17., 24.),
                    ],
                    stroke,
                ));
                vec![
                    origin + Vec2::new(5., 0.),
                    origin + Vec2::new(5., 19.),
                    origin + Vec2::new(24., 19.),
                ]
            }
        };
        painter.add(egui::Shape::line(points, stroke));
    }
    painter.text(
        rect.center_bottom() - Vec2::new(0., 9.),
        egui::Align2::CENTER_BOTTOM,
        label,
        theme::font(theme::FS_SMALL),
        if selected { accent } else { theme::text_secondary() },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// The faces the interface draws with.
///
/// The platform's own faces come first, the Chinese one after the Latin one:
/// they were designed to sit together, and egui's bundled face does not. Any
/// face that is missing simply leaves the bundled one in place.
fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(font) = latin_font() {
        fonts.font_data.insert("latin".into(), font.clone());
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "latin".into());
    }
    if let Some(font) = cjk_font() {
        fonts.font_data.insert("cjk".into(), font.clone());
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .push("cjk".into());
    }
    if let Some(font) = mono_font() {
        fonts.font_data.insert("mono".into(), font.clone());
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .insert(0, "mono".into());
    }
    fonts
}

pub(crate) fn configure(ctx: &egui::Context) {
    // The upstream appearance module owns the light/dark preference and the
    // base styles; the redesign layer paints its own materials on top.
    crate::appearance::configure(ctx);
    theme::apply(ctx);
    ctx.set_fonts(font_definitions());
}
/// Read the first font that exists among the given paths.
///
/// A font that cannot be parsed is skipped rather than fatal: the interface
/// falls back to egui's bundled face instead of failing to start.
fn read_font(paths: &[std::path::PathBuf]) -> Option<Arc<egui::FontData>> {
    paths
        .iter()
        .find_map(|path| std::fs::read(path).ok())
        .map(|bytes| Arc::new(egui::FontData::from_owned(bytes)))
}

/// The platform's Latin face, used ahead of egui's bundled one.
fn latin_font() -> &'static Option<Arc<egui::FontData>> {
    static FONT: std::sync::OnceLock<Option<Arc<egui::FontData>>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| read_font(&fastcull::platform::latin_font_paths()))
}

/// The platform's Chinese face.
fn cjk_font() -> &'static Option<Arc<egui::FontData>> {
    static FONT: std::sync::OnceLock<Option<Arc<egui::FontData>>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| read_font(&fastcull::platform::cjk_font_paths()))
}

/// The platform's monospaced face, for numbers that have to line up.
fn mono_font() -> &'static Option<Arc<egui::FontData>> {
    static FONT: std::sync::OnceLock<Option<Arc<egui::FontData>>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| read_font(&fastcull::platform::mono_font_paths()))
}
fn preload_fonts() {
    // Overlap font I/O with native window and graphics-driver initialization.
    std::thread::spawn(latin_font);
    std::thread::spawn(cjk_font);
    std::thread::spawn(mono_font);
}

// Create the window before filesystem enumeration. The same shell is also the
// standalone entry point, so a double click does not require command-line input.
struct Shell {
    app: Option<App>,
    preview: Option<Arc<crate::exposure_preview::Preview>>,
    pending: Option<(Option<PathBuf>, Vec<PathBuf>)>,
    receiver: Option<std::sync::mpsc::Receiver<Result<Option<Vec<PathBuf>>>>>,
    error: Option<String>,
    frames: usize,
    focus: crate::focus::StartupFocus,
}
impl Shell {
    fn load(
        &mut self,
        ctx: &egui::Context,
        request: Option<PathBuf>,
        roots: Vec<PathBuf>,
        picker: Option<bool>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.receiver = Some(rx);
        self.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = (|| -> Result<Option<Vec<PathBuf>>> {
                let roots = if let Some(file) = request {
                    let roots = crate::bridge_list::read(&file)?;
                    let _ = std::fs::remove_file(file);
                    roots
                } else if let Some(folder) = picker {
                    #[cfg(any(windows, target_os = "macos"))]
                    {
                        let dialog =
                            rfd::FileDialog::new().set_title("FastCull · 选择照片或文件夹");
                        let selected = if folder {
                            dialog.pick_folders()
                        } else {
                            dialog
                                .add_filter("RAW / JPEG", fastcull::formats::EXTENSIONS)
                                .pick_files()
                        };
                        let Some(paths) = selected else {
                            return Ok(None);
                        };
                        paths
                    }
                    #[cfg(not(any(windows, target_os = "macos")))]
                    {
                        let _ = folder;
                        anyhow::bail!("请将照片或文件夹拖入窗口");
                    }
                } else {
                    roots
                };
                crate::bridge_list::expand(roots).map(Some)
            })();
            log::info!(
                "startup discovery elapsed_ms={:.1}",
                started.elapsed().as_secs_f64() * 1000.
            );
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}
impl eframe::App for Shell {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if let Some(app) = &mut self.app {
            app.raw_input_hook(ctx, input);
        }
    }
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(app) = &mut self.app {
            app.on_exit(gl);
        }
    }
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        crate::appearance::sync_native_theme(ctx);
        theme::refresh(ctx);
        if let Some(app) = &mut self.app {
            app.update(ctx, frame);
            return;
        }
        self.focus.update(ctx, frame);
        self.frames += 1;
        if self.frames == 1 {
            log::info!("startup shell-first-frame");
        }
        if let Some((request, roots)) = self.pending.take() {
            self.load(ctx, request, roots, None);
        }
        if let Some(result) = self.receiver.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.receiver = None;
            match result {
                Ok(Some(paths)) => {
                    let mut app = App::from_context(ctx, self.preview.clone(), paths, None);
                    // Loading must not start another foreground-stealing window.
                    std::mem::swap(&mut app.startup_focus, &mut self.focus);
                    app.enable_bridge_moves(ctx, crate::logger::directory().join("moves"));
                    self.app = Some(app);
                    ctx.request_repaint();
                }
                Ok(None) => {}
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
        egui::TopBottomPanel::top("welcome-header")
            .frame(
                egui::Frame::new()
                    .fill(crate::appearance::palette(ctx).panel)
                    .inner_margin(egui::Margin::symmetric(24, 14)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("FastCull");
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        crate::appearance::control,
                    );
                });
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.30).max(40.));
                ui.label(
                    egui::RichText::new("FastCull")
                        .size(theme::FS_DISPLAY)
                        .strong(),
                );
                ui.add_space(14.);
                ui.label(egui::RichText::new("快速浏览，专注选片").size(theme::FS_TITLE));
                ui.add_space(30.);
                if self.receiver.is_some() || self.pending.is_some() || self.app.is_some() {
                    ui.spinner();
                    ui.label("正在整理照片列表…");
                } else {
                    let size = egui::vec2(180., 44.);
                    if ui
                        .add_sized(
                            size,
                            egui::Button::new(
                                egui::RichText::new("打开照片").color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(49, 102, 211)),
                        )
                        .clicked()
                    {
                        self.load(ctx, None, Vec::new(), Some(false));
                    }
                    ui.add_space(12.);
                    if ui
                        .add_sized(size, egui::Button::new("打开文件夹"))
                        .clicked()
                    {
                        self.load(ctx, None, Vec::new(), Some(true));
                    }
                    ui.add_space(22.);
                    ui.weak("也可以将照片或文件夹拖到这里");
                    ui.weak("ARW · CR2 · NEF · DNG · JPG / JPEG");
                    if let Some(error) = &self.error {
                        ui.add_space(16.);
                        ui.colored_label(theme::danger(), error);
                    }
                }
            });
        });
        if self.receiver.is_none() && self.app.is_none() {
            let dropped = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .collect::<Vec<_>>()
            });
            if !dropped.is_empty() {
                self.load(ctx, None, dropped, None);
            }
        }
        if self.frames == 1 {
            ctx.request_repaint();
        }
        if std::env::var_os("FASTCULL_LAUNCHER_SMOKE").is_some() {
            for event in ctx.input(|i| i.raw.events.clone()) {
                if let egui::Event::Screenshot { image, .. } = event {
                    if let Some(dir) = std::env::var_os("FASTCULL_SMOKE_DIR") {
                        let bytes: Vec<u8> =
                            image.pixels.iter().flat_map(|p| p.to_array()).collect();
                        let _ =
                            save_png(&PathBuf::from(dir).join("launcher.png"), image.size, &bytes);
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            if self.frames == 2 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
        }
    }
}
fn initial_window_size() -> [f32; 2] {
    if std::env::var_os("FASTCULL_SMOKE_DIR").is_some() {
        std::env::var("FASTCULL_SMOKE_SIZE")
            .ok()
            .and_then(|s| {
                s.split_once('x')
                    .and_then(|(w, h)| Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?]))
            })
            .filter(|s| s.iter().all(|v| v.is_finite() && *v >= 480. && *v <= 4096.))
            .unwrap_or([1280., 850.])
    } else {
        [1280., 850.]
    }
}
fn run_shell(request: Option<PathBuf>, paths: Vec<PathBuf>) -> Result<()> {
    preload_fonts();
    if let Some(dir) = std::env::var_os("FASTCULL_SMOKE_DIR") {
        std::fs::create_dir_all(dir)?;
    }
    let pending = (request.is_some() || !paths.is_empty()).then_some((request, paths));
    log::info!("startup native-window begin");
    eframe::run_native(
        "FastCull",
        crate::graphics::native_options(
            egui::ViewportBuilder::default()
                .with_active(true)
                .with_inner_size(initial_window_size())
                .with_min_inner_size([640., 480.]),
        ),
        Box::new(move |cc| {
            configure(&cc.egui_ctx);
            let mut shell = Shell {
                app: None,
                preview: crate::graphics::exposure_preview(cc),
                pending,
                receiver: None,
                error: None,
                frames: 0,
                focus: crate::focus::StartupFocus::new(),
            };
            if let Some((request, roots)) = shell.pending.take() {
                shell.load(&cc.egui_ctx, request, roots, None);
            }
            Ok(Box::new(shell))
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI: {e}"))
}

pub fn run(paths: Vec<PathBuf>) -> Result<()> {
    run_standalone(paths)
}
pub fn run_bridge_request(file: PathBuf) -> Result<()> {
    run_shell(Some(file), Vec::new())
}
pub fn run_standalone(paths: Vec<PathBuf>) -> Result<()> {
    run_shell(None, paths)
}
pub fn run_state(items: Vec<state::Item>, manifest: Option<PathBuf>) -> Result<()> {
    preload_fonts();
    let started = Instant::now();
    let paths = items.iter().map(|i| i.path.clone()).collect();
    let session = Arc::new(Session::new(items));
    if let Some(path) = manifest {
        session.attach_manifest(path)?;
    }
    log::info!(
        "startup session-attached elapsed_ms={:.1}",
        started.elapsed().as_secs_f64() * 1000.
    );
    let result = run_session(paths, Some(session.clone()), false);
    session.close()?;
    if result.is_ok()
        && session
            .sync_on_exit
            .load(std::sync::atomic::Ordering::Relaxed)
    {
        session.request_sync()?;
    }
    result
}
fn run_session(paths: Vec<PathBuf>, session: Option<Arc<Session>>, bridge: bool) -> Result<()> {
    anyhow::ensure!(!paths.is_empty(), "empty photo list");
    if let Some(dir) = std::env::var_os("FASTCULL_SMOKE_DIR") {
        std::fs::create_dir_all(dir)?;
    }
    let keep_session = session.is_some();
    let windows = crate::raw_window::Windows::new(session.clone());
    let render_windows = windows.clone();
    let options = crate::graphics::native_options(
        egui::ViewportBuilder::default()
            .with_active(true)
            .with_inner_size(initial_window_size())
            .with_min_inner_size([640., 480.]),
    );
    log::info!("startup native-window begin");
    let result = eframe::run_native(
        "FastCull · RAW/JPEG",
        options,
        Box::new(move |cc| {
            let mut app = match session {
                Some(session) => App::new_session(cc, paths, Some(session)),
                None => App::new(cc, paths),
            };
            app.raw_windows = render_windows;
            if bridge {
                app.enable_bridge_moves(&cc.egui_ctx, crate::logger::directory().join("moves"));
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI: {e}"));
    if keep_session {
        windows.finish()?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interface_draws_with_the_platform_faces() {
        // The bundled Latin face and the platform's Chinese face have to
        // cover both scripts, on every platform the app ships on.
        let fonts = font_definitions();
        let proportional = &fonts.families[&egui::FontFamily::Proportional];
        if fastcull::platform::latin_font_paths()
            .iter()
            .any(|path| path.exists())
        {
            assert_eq!(
                proportional.first().map(String::as_str),
                Some("latin"),
                "the platform's own face has to come first"
            );
        }
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        // The faces are only built on the first frame.
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let font = egui::FontId::proportional(theme::FS_BODY);
        assert!(
            ctx.fonts_mut(|f| f.has_glyphs(&font, "FastCull 0123")),
            "no Latin glyphs in the configured fonts"
        );
        if fastcull::platform::cjk_font_paths()
            .iter()
            .any(|path| path.exists())
        {
            assert!(
                ctx.fonts_mut(|f| f.has_glyphs(&font, "照片筛选")),
                "no Chinese glyphs in the configured fonts"
            );
        }
    }

    #[test]
    fn focus_overlay_projects_onto_the_padded_photo_rect() {
        // The photo is fitted into an area with a 12px margin. The focus
        // overlay has to use the photo's own screen rectangle, so its markers
        // stay on the photo and do not drift onto the canvas.
        let ctx = egui::Context::default();
        let image = egui::ColorImage::new([64, 48], vec![egui::Color32::WHITE; 64 * 48]);
        let texture = ctx.load_texture("photo", image, egui::TextureOptions::default());
        let canvas = Rect::from_min_size(egui::pos2(0., 0.), Vec2::new(400., 300.));
        let padded = canvas.shrink(theme::SP_M);
        let layout = photo_layout(
            &texture,
            padded,
            1,
            PhotoView {
                zoom: 0.,
                pan: Vec2::ZERO,
                ppp: 1.,
            },
            None,
            None,
        );
        assert!(padded.contains_rect(layout.rect));
        // The limiting axis keeps the full margin.
        assert!((canvas.max.y - layout.rect.max.y - theme::SP_M).abs() < 0.001);
        let point = fastcull::photo_focus::project(egui::pos2(0.25, 0.75), layout.uvs, layout.rect)
            .expect("focus projection");
        let expected = layout.rect.min + Vec2::new(0.25, 0.75) * layout.rect.size();
        assert!((point - expected).length() < 0.001);
    }
    #[test]
    fn catalog_crop_overrides_stale_xmp_but_never_later_user_edits() {
        let catalog = crate::exposure::Baseline {
            white_balance: None,
            exposure: None, // Older process version must not disable crop import.
            crop: Some(fastcull::crop::Crop {
                enabled: true,
                left: 100000,
                top: 200000,
                right: 900000,
                bottom: 800000,
                angle: 3000000,
            }),
            orientation: Some(6),
        };
        let mut photo = Photo::default();
        photo.apply_develop_baseline(catalog);
        assert_eq!(photo.values.crop, catalog.crop);
        assert_eq!(photo.orientation(), 6);
        assert!(photo.develop_loaded);
        photo.crop_edited = true;
        photo.values.crop = Some(Default::default());
        photo.orientation_edited = true;
        photo.values.orientation = Some(8);
        photo.apply_develop_baseline(catalog);
        assert_eq!(photo.values.crop, Some(Default::default()));
        assert_eq!(photo.orientation(), 8);
        photo.crop_edited = false;
        photo.apply_develop_baseline(crate::exposure::Baseline {
            crop: Some(Default::default()),
            ..Default::default()
        });
        assert_eq!(photo.values.crop, Some(Default::default()));
    }
    #[test]
    fn asynchronous_catalog_crop_waits_for_metadata_and_does_not_create_an_edit() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("catalog.ARW");
        std::fs::write(&raw, b"original RAW").unwrap();
        let paths = vec![raw.clone()];
        let manifest = dir.path().join("session.txt");
        fastcull::handoff::Manifest::new("crop-import".into(), &paths)
            .unwrap()
            .write(&manifest)
            .unwrap();
        let session = Arc::new(Session::new(vec![state::Item {
            path: raw.clone(),
            updates: Updates::default(),
        }]));
        session.attach_manifest(manifest.clone()).unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = App::new_session(&cc, paths, Some(session));
        app.photos[0].full_size = Some(Vec2::new(6000., 4000.));
        app.photos[0].loaded = true;
        app.begin_crop();
        assert!(app.crop_requested && app.crop_editor.is_none());
        app.photos[0].loaded = false;
        std::fs::write(
            manifest.with_extension("exposure"),
            format!(
                "{}\t-\t6\t1\t100000\t200000\t900000\t800000\t0\nEND\n",
                fastcull::handoff::encode(raw.to_str().unwrap())
            ),
        )
        .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while app.baselines.get(&raw).is_none() && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.baselines.get(&raw).is_some());
        app.receive_develop();
        assert!(app.photos[0].values.crop.is_none());
        app.photos[0].loaded = true;
        app.receive_develop();
        let crop = app.photos[0].values.crop.unwrap();
        assert_eq!((crop.left, crop.bottom), (100000, 800000));
        app.begin_crop();
        let editor = app.crop_editor.as_ref().unwrap();
        assert_eq!(editor.orientation, 6);
        assert_eq!(editor.value(), crop);
        assert_eq!(
            fastcull::handoff::Manifest::read(&manifest)
                .unwrap()
                .revision,
            0
        );
        assert!(!raw.with_extension("xmp").exists());
        assert_eq!(std::fs::read(raw).unwrap(), b"original RAW");
    }
    #[test]
    fn thumbnail_bounds_do_not_change_when_full_geometry_arrives() {
        for film in [false, true] {
            for orientation in [1, 6] {
                for crop in [
                    None,
                    Some(fastcull::crop::Crop {
                        enabled: true,
                        left: 200_000,
                        top: 100_000,
                        right: 800_000,
                        bottom: 900_000,
                        angle: 12_000_000,
                    }),
                ] {
                    let ctx = egui::Context::default();
                    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                    let mut app = App::new(&cc, vec![PathBuf::from("fixture.ARW")]);
                    app.photos[0].values.orientation = Some(orientation);
                    app.photos[0].values.crop = crop;
                    let thumb = ctx.load_texture(
                        "thumb",
                        egui::ColorImage::filled([160, 120], Color32::WHITE),
                        TextureOptions::LINEAR,
                    );
                    let id = thumb.id();
                    app.thumbs.insert(0, thumb, 0);
                    let mut vertices = Vec::new();
                    for full_size in [None, Some(Vec2::new(6192., 4128.))] {
                        app.photos[0].full_size = full_size;
                        let output = ctx.run(egui::RawInput::default(), |ctx| {
                            egui::CentralPanel::default()
                                .show(ctx, |ui| app.cell(ui, 0, Vec2::new(200., 180.), film));
                        });
                        let mesh = output
                            .shapes
                            .iter()
                            .find_map(|s| match &s.shape {
                                egui::Shape::Mesh(m) if m.texture_id == id => Some(m),
                                _ => None,
                            })
                            .unwrap();
                        vertices.push(
                            mesh.vertices
                                .iter()
                                .map(|v| (v.pos, v.uv))
                                .collect::<Vec<_>>(),
                        );
                    }
                    assert_eq!(
                        vertices[0], vertices[1],
                        "film={film}, orientation={orientation}, crop={crop:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn fit_preview_tracks_physical_canvas_with_headroom_and_stable_buckets() {
        assert_eq!(preview_edge(Vec2::new(944., 612.), 1.), 1280);
        assert_eq!(preview_edge(Vec2::new(945., 613.), 1.), 1280);
        assert_eq!(preview_edge(Vec2::new(944., 612.), 2.), 2560);
        assert_eq!(preview_edge(Vec2::new(3800., 2100.), 1.), 4096);
    }
    fn wait_moves(app: &mut App, ctx: &egui::Context) {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while app.mover.as_ref().unwrap().jobs > 0 {
            app.receive_moves(ctx);
            assert!(Instant::now() < deadline, "move queue stalled");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        app.refilter();
        assert!(app.error.is_none(), "{:?}", app.error);
    }
    #[test]
    fn crop_draft_cancel_save_failure_and_relocated_edit_keep_correct_identity() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("a.ARW");
        std::fs::write(&raw, b"original RAW").unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![raw.clone()]);
        app.enable_bridge_moves(&ctx, dir.path().join("journal"));
        app.photos[0].loaded = true;
        app.photos[0].full_size = Some(Vec2::new(6000., 4000.));
        app.begin_crop();
        app.crop_editor
            .as_mut()
            .unwrap()
            .select_ratio("1:1".into(), Some(1.));
        assert!(!raw.with_extension("xmp").exists());
        app.crop_editor = None; // Cancel discards the draft, preserving original bytes.
        assert!(!raw.with_extension("xmp").exists());
        app.begin_crop();
        app.crop_editor
            .as_mut()
            .unwrap()
            .select_ratio("16:9".into(), Some(16. / 9.));
        let expected = app.crop_editor.as_ref().unwrap().value();
        app.apply_crop();
        assert!(app.crop_editor.is_none());
        assert_eq!(xmp::read(&raw).unwrap().crop, Some(expected));
        let dest = dir.path().join("精选");
        std::fs::create_dir(&dest).unwrap();
        app.folder_editor.stage(&[0], &dest);
        app.submit_moves();
        wait_moves(&mut app, &ctx);
        let moved = dest.join("a.ARW");
        assert_eq!(app.paths[0], moved);
        app.begin_crop();
        assert_eq!(app.crop_editor.as_ref().unwrap().value(), expected);
        app.crop_editor
            .as_mut()
            .unwrap()
            .select_ratio("1:1".into(), Some(1.));
        app.apply_crop();
        let saved = xmp::read(&moved).unwrap().crop;
        assert_ne!(saved, Some(expected));
        assert!(!raw.with_extension("xmp").exists());
        assert_eq!(app.photos[0].values.crop, saved);
        std::fs::write(moved.with_extension("xmp"), "<broken>").unwrap();
        app.begin_crop();
        app.apply_crop();
        assert!(app.crop_editor.is_some());
        assert!(app.error.is_some());
        assert_eq!(app.photos[0].values.crop, saved);
        assert_eq!(
            std::fs::read_to_string(moved.with_extension("xmp")).unwrap(),
            "<broken>"
        );
    }
    #[test]
    fn bridge_continuous_batch_and_second_move_keep_photo_identity_and_new_xmp_path() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let paths: Vec<_> = (0..4).map(|i| source.join(format!("{i}.ARW"))).collect();
        for p in &paths {
            std::fs::write(p, "unchanged raw").unwrap();
        }
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, paths.clone());
        app.enable_bridge_moves(&ctx, dir.path().join("journal"));
        for p in &mut app.photos {
            p.loaded = true;
            p.exposure_base = Some(0);
        }
        app.rating(2);
        let dest = app.folder_editor.create_folder(dir.path(), "精选").unwrap();
        app.folder_editor.stage(&[0], &dest);
        app.submit_moves();
        assert_eq!(app.current, 1);
        app.folder_editor.repeat_move(1, &app.visible);
        app.submit_moves();
        assert_eq!(app.current, 2);
        app.go(0);
        app.rating(5); // queued photos cannot write at their old path.
        {
            let locations = app.pool.locations();
            let location = locations[0].read().unwrap();
            assert_eq!(xmp::read(&location).unwrap().rating, Some(2));
        }
        wait_moves(&mut app, &ctx);
        assert_eq!(app.paths[0], dest.join("0.ARW"));
        assert_eq!(app.paths[1], dest.join("1.ARW"));
        app.go(0);
        app.rating(4);
        assert_eq!(xmp::read(&app.paths[0]).unwrap().rating, Some(4));
        assert!(!paths[0].with_extension("xmp").exists());
        let batch = app.folder_editor.create_folder(dir.path(), "批量").unwrap();
        app.folder_editor.stage(&[2, 3], &batch);
        app.submit_moves();
        wait_moves(&mut app, &ctx);
        app.folder_editor.selected = Some(dest.clone());
        app.refilter();
        assert_eq!(app.visible, [0, 1]);
        app.folder_editor.selected = None;
        app.refilter();
        app.go(0);
        app.compare = Some(Compare {
            indices: [0, 3],
            views: [
                View {
                    zoom: 1.,
                    pan: Vec2::new(30., 40.),
                },
                View {
                    zoom: 1.,
                    pan: Vec2::new(10., 20.),
                },
            ],
            active: 0,
            linked: true,
        });
        let again = app
            .folder_editor
            .create_folder(dir.path(), "再移动")
            .unwrap();
        app.folder_editor.advance = false;
        app.folder_editor.stage(&[0], &again);
        app.submit_moves();
        wait_moves(&mut app, &ctx);
        assert_eq!(app.compare.as_ref().unwrap().indices, [0, 3]);
        assert_eq!(
            app.compare.as_ref().unwrap().views[0].pan,
            Vec2::new(30., 40.)
        );
        app.rating(5);
        assert_eq!(xmp::read(&app.paths[0]).unwrap().rating, Some(5));
        assert!(!dest.join("0.ARW").exists());
        for p in app.paths.iter() {
            assert_eq!(std::fs::read(p).unwrap(), b"unchanged raw");
        }
    }
    #[test]
    fn bridge_close_waits_for_pending_jobs_and_text_focus_keeps_m_for_typing() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("one.ARW");
        std::fs::write(&raw, "raw").unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![raw]);
        app.enable_bridge_moves(&ctx, dir.path().join("journal"));
        let target = app
            .folder_editor
            .create_folder(dir.path(), "target")
            .unwrap();
        let locations = app.pool.locations();
        let held = locations[0].read().unwrap(); // deterministic in-flight job.
        ctx.begin_pass(egui::RawInput::default());
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("folder-search")));
        app.folder_editor.last_target = Some(target.clone());
        ctx.input_mut(|i| {
            i.events.push(egui::Event::Key {
                key: Key::M,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
        });
        app.keyboard(&ctx);
        assert_eq!(app.mover.as_ref().unwrap().jobs, 0);
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("folder-search")));
        app.folder_editor.stage(&[0], &target);
        app.submit_moves();
        app.finish_close(&ctx, false);
        assert!(!app.allow_close);
        assert!(app.close_after_moves);
        drop(held);
        wait_moves(&mut app, &ctx);
        assert!(app.allow_close);
        let _ = ctx.end_pass();
    }
    #[test]
    fn folder_plan_filtering_and_grading_keep_original_photo_identity() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("上午");
        let b = dir.path().join("下午");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let paths = vec![a.join("a.ARW"), b.join("b.ARW")];
        for p in &paths {
            std::fs::write(p, "unchanged raw").unwrap();
        }
        let manifest = dir.path().join("session.txt");
        fastcull::handoff::Manifest::new("folder-test".into(), &paths)
            .unwrap()
            .write(&manifest)
            .unwrap();
        let session = Arc::new(Session::new(
            paths
                .iter()
                .map(|p| state::Item {
                    path: p.clone(),
                    updates: Updates {
                        rating: Some(4),
                        ..Default::default()
                    },
                })
                .collect(),
        ));
        session.attach_manifest(manifest.clone()).unwrap();
        for i in 0..2 {
            session.initialize(i).unwrap();
        }
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = App::new_session(&cc, paths.clone(), Some(session));
        assert!(app.filter_editor.open);
        assert!(!app.folder_editor.open);
        for p in &mut app.photos {
            p.loaded = true;
            p.exposure_base = Some(100000);
        }
        let dest = app.folder_editor.create_folder(dir.path(), "精选").unwrap();
        assert!(app.folder_editor.stage(&[1], &dest));
        app.folder_editor.selected = Some(dest.clone());
        app.filters.selected.insert(
            fastcull::filtering::Field::Rating,
            [Some("4".into())].into(),
        );
        app.refilter();
        assert_eq!(app.visible, [1]);
        assert_eq!(app.current, 1);
        app.rating(5);
        app.adjust_exposure(1);
        assert_eq!(xmp::read(&paths[0]).unwrap().rating, Some(4));
        assert_eq!(xmp::read(&paths[1]).unwrap().rating, Some(5));
        assert_eq!(xmp::read(&paths[1]).unwrap().exposure, Some(433333));
        assert!(!dest.join("b.ARW").exists());
        let rows = fastcull::handoff::Manifest::read(&manifest)
            .unwrap()
            .rows_since(0);
        assert_eq!(rows[0].0, paths[1]);
        assert_eq!(std::fs::read(&paths[1]).unwrap(), b"unchanged raw");
        app.folder_editor.selected = None;
        app.filters = Default::default();
        app.refilter();
        assert_eq!(app.visible, [0, 1]);
    }
    #[test]
    fn folder_search_and_multiple_selection_do_not_grade_or_close_photos() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("a.ARW"), PathBuf::from("b.ARW")]);
        ctx.begin_pass(key_input(&[Key::Q, Key::Num5]));
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("folder-search")));
        app.keyboard(&ctx);
        assert!(!app.close_dialog);
        assert_eq!(app.current, 0);
        let _ = ctx.end_pass();
        app.visible = vec![1];
        ctx.begin_pass(egui::RawInput::default());
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("folder-search")));
        ctx.input_mut(|i| {
            i.events.push(egui::Event::Key {
                key: Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            })
        });
        app.keyboard(&ctx);
        assert_eq!(
            app.folder_editor.photos.iter().copied().collect::<Vec<_>>(),
            [1]
        );
        let _ = ctx.end_pass();
    }
    #[test]
    fn minimum_window_with_filters_keeps_toolbar_bounded() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("photo.ARW")]);
        app.photos[0].loaded = true;
        app.photos[0].exposure_base = Some(100000);
        let mut height = 0.;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640., 480.))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::SidePanel::left("review-filters")
                .exact_width(242.)
                .show(ctx, |_| {});
            height = egui::TopBottomPanel::bottom("review-controls")
                .show(ctx, |ui| app.toolbar(ui))
                .response
                .rect
                .height();
        });
        assert!(
            height <= 112.,
            "Toolbar takes {height} px and crowds the image"
        );
    }
    #[test]
    fn filmstrip_wheel_respects_hover_drag_dialog_filter_and_comparison() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(
            &cc,
            (0..8)
                .map(|i| PathBuf::from(format!("film-{i}.ARW")))
                .collect(),
        );
        app.visible = vec![0, 2, 4, 6];
        let frame = |app: &mut App, pos: Pos2, extra: Vec<egui::Event>| {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pos));
            input.events.extend(extra);
            let _ = ctx.run(input, |ctx| {
                app.film_panel(ctx);
            });
        };
        let wheel = |delta| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0., delta),
            modifiers: egui::Modifiers::NONE,
        };
        let inside = Pos2::new(400., 560.);
        frame(&mut app, inside, vec![]);
        frame(&mut app, inside, vec![wheel(-1.)]);
        assert_eq!(app.current, 2);
        frame(&mut app, Pos2::new(400., 100.), vec![wheel(-1.)]);
        assert_eq!(app.current, 2);
        app.close_dialog = true;
        frame(&mut app, inside, vec![wheel(-1.)]);
        assert_eq!(app.current, 2);
        app.close_dialog = false;
        frame(
            &mut app,
            inside,
            vec![
                egui::Event::PointerButton {
                    pos: inside,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                wheel(-1.),
            ],
        );
        assert_eq!(app.current, 2);
        frame(
            &mut app,
            inside,
            vec![egui::Event::PointerButton {
                pos: inside,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        app.go(2);
        app.compare = Some(Compare {
            indices: [2, 4],
            views: [View::default(); 2],
            active: 0,
            linked: true,
        });
        frame(&mut app, inside, vec![wheel(-1.)]);
        assert_eq!(app.current, 6);
        assert_eq!(app.compare.as_ref().unwrap().indices, [6, 4]);
        frame(&mut app, inside, vec![wheel(-1.)]);
        assert_eq!(app.current, 6);
    }

    #[test]
    fn filmstrip_border_drag_resizes_and_keeps_current_photo() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("resize.ARW")]);
        let mut frame = |events| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                events,
                ..Default::default()
            };
            let mut rect = Rect::NOTHING;
            let _ = ctx.run(input, |ctx| {
                rect = app.film_panel(ctx);
            });
            rect
        };
        let original = frame(vec![]);
        let edge = Pos2::new(400., original.top());
        frame(vec![egui::Event::PointerMoved(edge)]);
        frame(vec![egui::Event::PointerButton {
            pos: edge,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        let moved = edge - Vec2::new(0., 80.);
        let resized = frame(vec![egui::Event::PointerMoved(moved)]);
        frame(vec![egui::Event::PointerButton {
            pos: moved,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(
            resized.height() > original.height() + 50.,
            "{original:?} -> {resized:?}"
        );
        assert_eq!(app.current, 0);
    }

    #[test]
    fn compare_single_click_zooms_and_drag_does_not_toggle() {
        for side in [0, 1] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = App::new(&cc, vec![PathBuf::from("a.ARW"), PathBuf::from("b.ARW")]);
            app.enter_compare();
            app.compare.as_mut().unwrap().linked = false;
            for i in 0..2 {
                app.photos[i].full_size = Some(Vec2::new(2400., 1600.));
                let t = ctx.load_texture(
                    format!("photo-{i}"),
                    egui::ColorImage::filled([120, 80], Color32::WHITE),
                    TextureOptions::LINEAR,
                );
                app.fit.insert(i, t, 0);
            }
            let mut time = 0.;
            let mut draw = |app: &mut App, events| {
                time += 0.1;
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| app.compare_canvas(ui, ctx));
                    },
                )
            };
            draw(&mut app, vec![]);
            draw(&mut app, vec![]);
            let pos = Pos2::new(200. + side as f32 * 600., 300.);
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            draw(
                &mut app,
                vec![egui::Event::PointerMoved(pos), button(pos, true)],
            );
            draw(&mut app, vec![button(pos, false)]);
            assert_eq!(app.compare.as_ref().unwrap().active, side);
            assert!(
                (app.compare.as_ref().unwrap().views[side].zoom - 1.).abs() < 0.00001,
                "one click must enter 100% in either compare pane"
            );
            assert_eq!(app.compare.as_ref().unwrap().views[1 - side].zoom, 0.);
            let anchor = app.compare.as_ref().unwrap().views[side].pan;
            assert_ne!(
                anchor,
                Vec2::ZERO,
                "zoom must center the clicked image point"
            );
            draw(&mut app, vec![button(pos, true)]);
            let moved = pos + Vec2::new(40., 20.);
            draw(&mut app, vec![egui::Event::PointerMoved(moved)]);
            draw(&mut app, vec![button(moved, false)]);
            assert!(
                (app.compare.as_ref().unwrap().views[side].zoom - 1.).abs() < 0.00001,
                "drag release must not toggle zoom"
            );
            draw(&mut app, vec![]);
            draw(&mut app, vec![]);
            draw(&mut app, vec![button(moved, true)]);
            draw(&mut app, vec![button(moved, false)]);
            assert_eq!(app.compare.as_ref().unwrap().views[side].zoom, 0.);
            assert!(
                app.fit_edge <= 1280,
                "compare previews must follow the pane size instead of the 2560px startup default"
            );
            for (label, expected) in [("100%", 1.), ("适配", 0.)] {
                let output = draw(&mut app, vec![]);
                let pos = output
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(t)
                            if t.galley.job.text == label && (t.pos.x >= 600.) == (side == 1) =>
                        {
                            Some(t.pos + Vec2::splat(3.))
                        }
                        _ => None,
                    })
                    .expect("each pane must render a zoom control");
                draw(
                    &mut app,
                    vec![egui::Event::PointerMoved(pos), button(pos, true)],
                );
                draw(&mut app, vec![button(pos, false)]);
                assert!(
                    (app.compare.as_ref().unwrap().views[side].zoom - expected).abs() < 0.00001,
                    "{label} button must work on side {side}"
                );
            }
        }
    }

    #[test]
    fn click_zoom_centers_clicked_image_point_and_modes_are_consistent() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")]);
        assert!(app.filter_editor.open);
        let image = Rect::from_min_size(Pos2::new(200., 50.), Vec2::new(600., 400.));
        app.zoom_at(image, Some(Pos2::new(650., 150.)));
        assert!(app.zoom);
        assert_eq!(app.pan, Vec2::new(-0.25, 0.25));
        app.next(1);
        assert_eq!(app.pan, Vec2::new(-0.25, 0.25));
        app.zoom_at(image, Some(image.center()));
        assert!(!app.zoom);
        app.zoom_at(image, Some(Pos2::ZERO));
        assert_eq!(app.pan, Vec2::ZERO); // Letterbox click has no image coordinate.
        for key in [Key::C, Key::G, Key::E] {
            ctx.begin_pass(key_input(&[key]));
            app.keyboard(&ctx);
            match key {
                Key::C => {
                    assert!(app.compare.is_some());
                    assert!(!app.grid);
                }
                Key::G => assert!(app.grid),
                Key::E => {
                    assert!(app.compare.is_none());
                    assert!(!app.grid);
                }
                _ => unreachable!(),
            }
            let _ = ctx.end_pass();
        }
    }

    #[test]
    fn zoom_navigation_never_flashes_fit_before_full_geometry_is_known() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")]);
        let preview = ctx.load_texture(
            "pending-preview",
            egui::ColorImage::filled([600, 400], Color32::WHITE),
            TextureOptions::LINEAR,
        );
        let id = preview.id();
        app.fit.insert(1, preview, 1);
        app.zoom = true;
        app.pan = Vec2::new(0.1, -0.1);
        app.next(1);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.central(ui, ctx));
            },
        );
        assert!(app.zoom);
        assert_eq!(app.pan, Vec2::new(0.1, -0.1));
        assert!(
            !output.shapes.iter().any(
                |shape| matches!(&shape.shape, egui::Shape::Mesh(mesh) if mesh.texture_id == id)
            ),
            "100% navigation must not briefly draw the next photo fitted to the window"
        );
    }

    #[test]
    fn fit_navigation_geometry_stays_fixed_across_thumb_fit_and_full() {
        for orientation in [1, 6] {
            for compare in [false, true] {
                for cropped in [false, true] {
                    for ppp in [1., 1.5, 2.] {
                        let ctx = egui::Context::default();
                        ctx.set_pixels_per_point(ppp);
                        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                        let mut app =
                            App::new(&cc, vec![PathBuf::from("a.ARW"), PathBuf::from("b.ARW")]);
                        app.next(1);
                        // Decoded previews carry source metadata before their first frame.
                        let mut info = fastcull::arw::ArwInfo::read(std::path::Path::new(
                            "tests/fixtures/preview-3x2.jpg",
                        ))
                        .unwrap();
                        info.image_size = Some((1200, 800));
                        app.photos[1].info = Some(info);
                        app.photos[1].values.orientation = Some(orientation);
                        if cropped {
                            app.photos[1].values.crop = Some(fastcull::crop::Crop {
                                enabled: true,
                                left: 200_000,
                                top: 100_000,
                                right: 800_000,
                                bottom: 900_000,
                                angle: 12_000_000,
                            });
                        }
                        if compare {
                            app.compare = Some(Compare {
                                indices: [0, 1],
                                active: 1,
                                linked: false,
                                views: [View {
                                    zoom: 0.,
                                    pan: Vec2::ZERO,
                                }; 2],
                            });
                        }
                        let mut vertices = Vec::new();
                        // Real Sony preview proportions, including resize rounding.
                        for (stage, size) in
                            [[200, 134], [808, 540], [808, 540], [1200, 800], [808, 540]]
                                .into_iter()
                                .enumerate()
                        {
                            let t = ctx.load_texture(
                                format!("stage-{stage}"),
                                egui::ColorImage::filled(size, Color32::WHITE),
                                TextureOptions::LINEAR,
                            );
                            let id = t.id();
                            match stage {
                                0 => app.thumbs.insert(1, t, 1),
                                1 => app.fit.insert(1, t, 1),
                                2 => {
                                    app.photos[1].full_size = Some(Vec2::new(1200., 800.));
                                    app.fit.insert(1, t, 1);
                                }
                                3 => app.full.insert(1, t, 1),
                                _ => {
                                    app.full.items.remove(&1);
                                    app.fit.insert(1, t, 1);
                                }
                            }
                            let input = egui::RawInput {
                                screen_rect: Some(Rect::from_min_size(
                                    Pos2::ZERO,
                                    Vec2::new(1600., 900.),
                                )),
                                ..Default::default()
                            };
                            // Apply pending DPI/layout changes before measuring.
                            let _ = ctx.run(input.clone(), |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| app.central(ui, ctx));
                            });
                            let output = ctx.run(input, |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| app.central(ui, ctx));
                            });
                            let mesh = output
                                .shapes
                                .iter()
                                .find_map(|shape| match &shape.shape {
                                    egui::Shape::Mesh(mesh) if mesh.texture_id == id => Some(mesh),
                                    _ => None,
                                })
                                .expect("current photo is rendered");
                            vertices.push(
                                mesh.vertices
                                    .iter()
                                    .map(|v| (v.pos, v.uv))
                                    .collect::<Vec<_>>(),
                            );
                        }
                        assert!(
                            vertices.iter().all(|v| *v == vertices[0]),
                            "main photo shifts: orientation={orientation} compare={compare} cropped={cropped} ppp={ppp}: {vertices:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn zoom_preview_to_full_keeps_crop_for_each_resolution_rotation_and_dpi() {
        for size in [[1200, 800], [1800, 1200]] {
            for orientation in [1, 6] {
                for ppp in [1., 1.5, 2.] {
                    for compare in [false, true] {
                        let ctx = egui::Context::default();
                        ctx.set_pixels_per_point(ppp);
                        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                        let mut app =
                            App::new(&cc, vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")]);
                        app.zoom = true;
                        app.next(1);
                        app.pan = Vec2::new(0.13, -0.09);
                        app.photos[1].full_size = Some(Vec2::new(size[0] as f32, size[1] as f32));
                        app.photos[1].values.orientation = Some(orientation);
                        if compare {
                            app.compare = Some(Compare {
                                indices: [0, 1],
                                active: 1,
                                linked: false,
                                views: [View {
                                    zoom: 1.,
                                    pan: app.pan,
                                }; 2],
                            });
                        }
                        let frame = |app: &mut App, id| {
                            // Apply the pending DPI/viewport change before measuring.
                            let _ = ctx.run(
                                egui::RawInput {
                                    screen_rect: Some(Rect::from_min_size(
                                        Pos2::ZERO,
                                        Vec2::new(800., 600.),
                                    )),
                                    ..Default::default()
                                },
                                |ctx| {
                                    egui::CentralPanel::default()
                                        .show(ctx, |ui| app.central(ui, ctx));
                                },
                            );
                            let output = ctx.run(
                                egui::RawInput {
                                    screen_rect: Some(Rect::from_min_size(
                                        Pos2::ZERO,
                                        Vec2::new(800., 600.),
                                    )),
                                    ..Default::default()
                                },
                                |ctx| {
                                    egui::CentralPanel::default()
                                        .show(ctx, |ui| app.central(ui, ctx));
                                },
                            );
                            output
                                .shapes
                                .iter()
                                .find_map(|shape| match &shape.shape {
                                    egui::Shape::Mesh(mesh) if mesh.texture_id == id => {
                                        Some(Rect::from_min_max(
                                            mesh.vertices[0].pos,
                                            mesh.vertices[2].pos,
                                        ))
                                    }
                                    _ => None,
                                })
                                .expect("current photo is rendered")
                        };
                        let thumb = ctx.load_texture(
                            "small",
                            egui::ColorImage::filled([120, 80], Color32::WHITE),
                            TextureOptions::LINEAR,
                        );
                        let id = thumb.id();
                        app.thumbs.insert(1, thumb, 1);
                        let initial = frame(&mut app, id);
                        let fit = ctx.load_texture(
                            "fit",
                            egui::ColorImage::filled([600, 400], Color32::WHITE),
                            TextureOptions::LINEAR,
                        );
                        let id = fit.id();
                        app.fit.insert(1, fit, 1);
                        let middle = frame(&mut app, id);
                        let full = ctx.load_texture(
                            "full",
                            egui::ColorImage::filled(size, Color32::WHITE),
                            TextureOptions::LINEAR,
                        );
                        let id = full.id();
                        app.full.insert(1, full, 1);
                        let final_rect = frame(&mut app, id);
                        assert_eq!(
                            initial, middle,
                            "thumbnail and fit must use full-image geometry"
                        );
                        assert_eq!(
                            middle, final_rect,
                            "full upload must only change sharpness: {size:?}, orientation={orientation}, ppp={ppp}, compare={compare}"
                        );
                        let expected = if orientation >= 5 {
                            Vec2::new(size[1] as f32, size[0] as f32)
                        } else {
                            Vec2::new(size[0] as f32, size[1] as f32)
                        } / ppp;
                        assert!((final_rect.size() - expected).length() < 0.01);
                    }
                }
            }
        }
    }
    fn key_input(keys: &[Key]) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        for &key in keys {
            input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        input
    }
    #[test]
    fn rapid_rating_and_navigation_keep_original_event_order() {
        for (keys, ratings) in [
            ([Key::Num5, Key::ArrowRight], [5, 2]),
            ([Key::ArrowRight, Key::Num5], [1, 5]),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let paths: Vec<_> = (0..2)
                .map(|i| dir.path().join(format!("{i}.ARW")))
                .collect();
            for (i, p) in paths.iter().enumerate() {
                xmp::write(
                    p,
                    &Updates {
                        rating: Some(i as u8 + 1),
                        ..Default::default()
                    },
                )
                .unwrap();
            }
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = App::new(&cc, paths.clone());
            // Drain the background sidecar reads before writing again: on a slow
            // CI machine the metadata worker can otherwise hold the same file
            // while this test replaces it, and Windows drops that write.
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            let mut drained = 0;
            while drained < paths.len() && Instant::now() < deadline {
                drained += app.pool.take_metadata().len();
                if drained < paths.len() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            for (i, p) in paths.iter().enumerate() {
                app.photos[i].loaded = true;
                app.photos[i].values = xmp::read(p).unwrap();
            }
            ctx.begin_pass(key_input(&keys));
            app.keyboard(&ctx);
            let _ = ctx.end_pass();
            for (p, rating) in paths.iter().zip(ratings) {
                assert_eq!(xmp::read(p).unwrap().rating, Some(rating), "{keys:?}");
            }
            assert_eq!(app.current, 1);
        }
    }
    #[test]
    fn open_menu_keeps_keys_for_the_menu_instead_of_grading_or_navigating() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("a.ARW"), PathBuf::from("b.ARW")]);
        ctx.begin_pass(key_input(&[Key::ArrowRight, Key::Q]));
        egui::Popup::open_id(&ctx, egui::Id::new("review-menu"));
        app.keyboard(&ctx);
        assert_eq!(app.current, 0);
        assert!(!app.close_dialog);
        assert!(ctx.input(|i| i.key_pressed(Key::ArrowRight)));
        let _ = ctx.end_pass();
    }
    #[test]
    fn filtering_comparison_grading_and_exposure_keep_correct_photo_and_session_fields() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..2)
            .map(|i| dir.path().join(format!("{i}.ARW")))
            .collect();
        let session = Arc::new(Session::new(
            paths
                .iter()
                .map(|p| state::Item {
                    path: p.clone(),
                    updates: Updates {
                        rating: Some(4),
                        ..Default::default()
                    },
                })
                .collect(),
        ));
        for (i, p) in paths.iter().enumerate() {
            session.initialize(i).unwrap();
            xmp::write(
                p,
                &Updates {
                    exposure: Some(100000),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let manifest = dir.path().join("session.txt");
        fastcull::handoff::Manifest::new("combined-test".into(), &paths)
            .unwrap()
            .write(&manifest)
            .unwrap();
        session.attach_manifest(manifest.clone()).unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = App::new_session(&cc, paths.clone(), Some(session));
        for (i, p) in paths.iter().enumerate() {
            app.photos[i].loaded = true;
            app.photos[i].values = xmp::read(p).unwrap();
            app.photos[i].exposure_base = Some(100000);
        }
        app.current = 1;
        app.compare = Some(Compare {
            indices: [0, 1],
            views: [View {
                zoom: 1.,
                pan: Vec2::new(0.1, 0.2),
            }; 2],
            active: 1,
            linked: false,
        });
        app.filters.selected.insert(
            fastcull::filtering::Field::Rating,
            [Some("4".into())].into(),
        );
        app.adjust_exposure(1);
        app.refilter();
        assert_eq!(
            app.compare.as_ref().unwrap().views[0].pan,
            Vec2::new(0.1, 0.2)
        );
        assert_eq!(xmp::read(&paths[0]).unwrap().exposure, Some(100000));
        assert_eq!(xmp::read(&paths[1]).unwrap().exposure, Some(433333));
        app.rating(5);
        app.refilter();
        assert_eq!(app.visible, [0]);
        assert!(app.compare.is_none());
        assert_eq!(app.current, 0);
        let rows = fastcull::handoff::Manifest::read(&manifest)
            .unwrap()
            .rows_since(0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, paths[1]);
        assert_eq!(rows[0].1.as_ref().unwrap().rating, Some(5));
        assert_eq!(rows[0].1.as_ref().unwrap().exposure, Some(433333));
        app.filters.selected.insert(
            fastcull::filtering::Field::Rating,
            [Some("3".into())].into(),
        );
        app.refilter();
        app.rating(1);
        app.adjust_exposure(3);
        assert_eq!(xmp::read(&paths[0]).unwrap().rating, Some(4));
        assert_eq!(xmp::read(&paths[0]).unwrap().exposure, Some(100000));
    }
    #[test]
    fn selecting_other_compared_photo_activates_it_without_replacing_either() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = App::new(&cc, vec![PathBuf::from("a.ARW"), PathBuf::from("b.ARW")]);
        app.current = 1;
        app.compare = Some(Compare {
            indices: [0, 1],
            views: [View::default(); 2],
            active: 1,
            linked: false,
        });
        app.go(0);
        assert_eq!(app.current, 0);
        assert_eq!(app.compare.as_ref().unwrap().active, 0);
        assert_eq!(app.compare.as_ref().unwrap().indices, [0, 1]);
    }
    #[test]
    fn relative_pan_stops_both_at_first_edge_and_preserves_offset() {
        let mut c = Compare {
            indices: [0, 1],
            active: 1,
            linked: true,
            views: [
                View {
                    zoom: 1.,
                    pan: Vec2::new(0.29, -0.1),
                },
                View {
                    zoom: 2.,
                    pan: Vec2::new(-0.1, 0.08),
                },
            ],
        };
        let offset = c.views[0].pan - c.views[1].pan;
        c.translate(
            1,
            Vec2::new(0.1, 0.03),
            [Vec2::splat(0.3), Vec2::splat(0.4)],
        );
        assert!((c.views[0].pan.x - 0.3).abs() < 0.00001);
        assert!((c.views[1].pan.x + 0.09).abs() < 0.00001);
        assert!((c.views[0].pan - c.views[1].pan - offset).length() < 0.00001);
        c.translate(
            0,
            Vec2::new(-0.02, 0.),
            [Vec2::splat(0.3), Vec2::splat(0.4)],
        );
        assert!((c.views[0].pan.x - 0.28).abs() < 0.00001);
        assert!((c.views[0].pan - c.views[1].pan - offset).length() < 0.00001);
    }
    #[test]
    fn linked_zoom_limits_keep_ratio_and_fit_keeps_subject_anchors() {
        let mut c = Compare {
            indices: [0, 1],
            active: 0,
            linked: true,
            views: [
                View {
                    zoom: 2.,
                    pan: Vec2::new(0.1, 0.2),
                },
                View {
                    zoom: 4.,
                    pan: Vec2::new(-0.05, 0.08),
                },
            ],
        };
        c.scale(0, 8., [0.2, 0.4]);
        assert_eq!(c.views[0].zoom, 4.);
        assert_eq!(c.views[1].zoom, 8.);
        let anchors = c.views.map(|v| v.pan);
        c.scale(0, 0., [0.2, 0.4]);
        assert_eq!(c.views.map(|v| v.zoom), [0., 0.]);
        c.scale(0, 1., [0.2, 0.4]);
        assert_eq!(c.views.map(|v| v.zoom), [1., 2.]);
        assert_eq!(c.views.map(|v| v.pan), anchors);
    }
    #[test]
    fn linked_navigation_does_not_reset_reference_alignment() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = App::new(
            &cc,
            (0..3).map(|i| PathBuf::from(format!("{i}.ARW"))).collect(),
        );
        app.current = 1;
        app.compare = Some(Compare {
            indices: [0, 1],
            active: 1,
            linked: true,
            views: [
                View {
                    zoom: 2.,
                    pan: Vec2::new(0.12, -0.08),
                },
                View {
                    zoom: 1.,
                    pan: Vec2::new(-0.06, 0.15),
                },
            ],
        });
        app.go(2);
        let c = app.compare.as_ref().unwrap();
        assert_eq!(c.indices, [0, 2]);
        assert_eq!(c.views[0].zoom, 2.);
        assert_eq!(c.views[0].pan, Vec2::new(0.12, -0.08));
        assert_eq!(c.views[1].pan, Vec2::new(-0.06, 0.15));
    }
    #[test]
    fn linked_zoom_preserves_separately_aligned_subjects_and_scale_ratio() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = App::new(&cc, vec![PathBuf::from("a.ARW"), PathBuf::from("b.ARW")]);
        app.compare = Some(Compare {
            indices: [0, 1],
            views: [
                View {
                    zoom: 0.5,
                    pan: Vec2::new(0.12, -0.08),
                },
                View {
                    zoom: 1.,
                    pan: Vec2::new(-0.06, 0.15),
                },
            ],
            active: 0,
            linked: true,
        });
        app.set_zoom(true);
        let c = app.compare.as_ref().unwrap();
        assert_eq!(c.views[0].zoom, 1.);
        assert_eq!(
            c.views[1].zoom, 2.,
            "linked zoom must apply a ratio, not copy absolute scale"
        );
        assert_eq!(c.views[0].pan, Vec2::new(0.12, -0.08));
        assert_eq!(c.views[1].pan, Vec2::new(-0.06, 0.15));
    }
    #[test]
    fn comparison_keyboard_pan_moves_active_view_and_respects_linking() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("a.ARW"), PathBuf::from("b.ARW")]);
        app.current = 1;
        app.zoom = true;
        app.compare = Some(Compare {
            indices: [0, 1],
            views: [
                View {
                    zoom: 1.,
                    pan: Vec2::new(0.12, -0.08),
                },
                View {
                    zoom: 1.,
                    pan: Vec2::new(-0.06, 0.15),
                },
            ],
            active: 1,
            linked: true,
        });
        app.full.insert(
            1,
            ctx.load_texture(
                "pan",
                egui::ColorImage::filled([100, 100], Color32::WHITE),
                TextureOptions::LINEAR,
            ),
            1,
        );
        let reference = app.full.items.get(&1).unwrap().handle.clone();
        app.full.insert(0, reference, 1);
        let offset =
            app.compare.as_ref().unwrap().views[0].pan - app.compare.as_ref().unwrap().views[1].pan;
        ctx.begin_pass(key_input(&[Key::ArrowUp]));
        app.keyboard(&ctx);
        let _ = ctx.end_pass();
        let c = app.compare.as_ref().unwrap();
        assert!(c.views[1].pan.y > 0.);
        assert!((c.views[0].pan - c.views[1].pan - offset).length() < 0.00001);
        let previous = c.views[0].pan;
        app.compare.as_mut().unwrap().linked = false;
        ctx.begin_pass(key_input(&[Key::ArrowDown]));
        app.keyboard(&ctx);
        let _ = ctx.end_pass();
        assert_eq!(app.compare.as_ref().unwrap().views[0].pan, previous);
        assert!(app.compare.as_ref().unwrap().views[1].pan.y < previous.y);
    }
    #[test]
    fn exposure_thirds_accumulate_to_a_stop_and_keep_other_develop_settings() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("exposure.ARW");
        std::fs::write(raw.with_extension("xmp"),r#"<rdf:Description xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Contrast2012="23"/>"#).unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = App::new(&cc, vec![raw.clone()]);
        app.photos[0].loaded = true;
        app.photos[0].exposure_base = Some(100000);
        app.photos[0].values.exposure = Some(100000);
        for target in [433333, 766667, 1100000] {
            app.adjust_exposure(1);
            assert_eq!(xmp::read(&raw).unwrap().exposure, Some(target));
        }
        app.adjust_exposure(-3);
        assert_eq!(xmp::read(&raw).unwrap().exposure, Some(100000));
        for _ in 0..8 {
            app.adjust_exposure(3);
        }
        assert_eq!(xmp::read(&raw).unwrap().exposure, Some(5_000_000));
        app.adjust_exposure(-3);
        assert_eq!(
            xmp::read(&raw).unwrap().exposure,
            Some(4_000_000),
            "{:?}",
            app.error
        );
        assert!(
            std::fs::read_to_string(raw.with_extension("xmp"))
                .unwrap()
                .contains("crs:Contrast2012=\"23\"")
        );
    }
    #[test]
    fn comparing_keeps_two_distinct_photos_and_keyboard_navigation_skips_reference() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(
            &cc,
            (0..4)
                .map(|i| PathBuf::from(format!("missing-{i}.ARW")))
                .collect(),
        );
        app.current = 2;
        app.zoom = true;
        app.compare = Some(Compare {
            indices: [1, 2],
            views: [
                View {
                    zoom: 1.,
                    pan: Vec2::new(0.1, 0.2),
                },
                View {
                    zoom: 2.,
                    pan: Vec2::ZERO,
                },
            ],
            active: 1,
            linked: false,
        });
        app.next(-1);
        assert_eq!(app.current, 0);
        assert_eq!(app.compare.as_ref().unwrap().indices, [1, 0]);
        assert_eq!(
            app.compare.as_ref().unwrap().views[0].pan,
            Vec2::new(0.1, 0.2)
        );
        app.activate_compare(0);
        app.set_zoom(false);
        assert_eq!(app.compare.as_ref().unwrap().views[1].zoom, 2.);
        app.compare.as_mut().unwrap().linked = true;
        app.set_zoom(true);
        assert_eq!(app.compare.as_ref().unwrap().views[1].zoom, 2.);
        // Editing a filter cannot turn numeric/star keys into grading actions.
        ctx.begin_pass(egui::RawInput::default());
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(("fastcull-filter-input", "query"))));
        ctx.input_mut(|i| {
            i.events.push(egui::Event::Key {
                key: Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
        });
        app.keyboard(&ctx);
        assert_eq!(app.current, 1);
        let _ = ctx.end_pass();
    }
    #[test]
    fn close_keyboard_opens_confirmation_without_writing_a_sync_request() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("missing.ARW")]);
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: Key::Q,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        ctx.begin_pass(input);
        app.keyboard(&ctx);
        assert!(app.close_dialog);
        assert!(!app.allow_close);
        let _ = ctx.end_pass();
    }
    #[test]
    fn comparison_cache_retains_both_large_images_when_neighbors_arrive() {
        let ctx = egui::Context::default();
        let mut cache = TextureCache::new(2 * 64 * 64 * 4, 4);
        cache.pinned = Some(1);
        for i in 0..8 {
            cache.insert_near(
                i,
                ctx.load_texture(
                    format!("compare{i}"),
                    egui::ColorImage::filled([64, 64], Color32::WHITE),
                    TextureOptions::LINEAR,
                ),
                0,
            );
        }
        assert_eq!(cache.items.len(), 2);
        assert!(cache.items.contains_key(&0));
        assert!(cache.items.contains_key(&1));
    }
    #[test]
    fn fit_canvas_does_not_request_full_synchronously_and_thumbnail_error_is_independent() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("missing.ARW")]);
        app.photos[0]
            .image_errors
            .insert(Level::Thumb, "bad thumbnail".into());
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.central(ui, ctx));
        });
        assert!(app.pool.requested(0, Level::Fit(app.fit_edge)));
        assert!(!app.pool.requested(0, Level::Full));
        app.zoom = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.central(ui, ctx));
        });
        assert!(app.pool.requested(0, Level::Full));
    }
    #[test]
    fn arrows_select_photos_even_at_one_hundred_percent() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("a.ARW"), PathBuf::from("b.ARW")]);
        app.zoom = true;
        app.pan = Vec2::new(0.1, 0.2);
        for (key, expected) in [(Key::ArrowRight, 1), (Key::ArrowLeft, 0)] {
            let mut input = egui::RawInput::default();
            input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            ctx.begin_pass(input);
            app.keyboard(&ctx);
            assert_eq!(
                app.current, expected,
                "100% arrow navigation changed pan instead of photo"
            );
            assert!(app.zoom);
            assert_eq!(app.pan, Vec2::new(0.1, 0.2));
            let _ = ctx.end_pass();
        }
    }
    #[test]
    fn zoom_navigation_keeps_relative_anchor_across_sizes_and_orientations() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")]);
        for (i, size) in [[600, 400], [800, 1200]].into_iter().enumerate() {
            let texture = ctx.load_texture(
                format!("anchor-{i}"),
                egui::ColorImage::filled(size, Color32::WHITE),
                TextureOptions::LINEAR,
            );
            app.full.insert(i, texture, 0);
        }
        app.photos[1].values.orientation = Some(6);
        app.zoom = true;
        app.canvas_size = Vec2::splat(100.);
        app.move_view(Vec2::new(30., -20.), 2.);
        let anchor = Vec2::new(0.1, -0.1);
        assert_eq!(app.pan, anchor);
        app.next(1);
        assert_eq!(app.pan, anchor);
        // Rotated second photo displays as 600 x 400 logical pixels at 200% DPI.
        app.move_view(Vec2::new(60., -40.), 2.);
        assert_eq!(app.pan, anchor * 2.);
        app.next(-1);
        assert_eq!(
            app.pan, anchor,
            "returning to a visited photo must restore its own anchor"
        );
        app.next(1);
        assert_eq!(app.pan, anchor * 2., "each photo remembers its last anchor");
        app.next(-1);
        app.compare = Some(Compare {
            indices: [0, 1],
            views: [View {
                zoom: 1.,
                pan: anchor,
            }; 2],
            active: 0,
            linked: true,
        });
        // Direct selection of the reference activates it without resetting either view.
        app.go(1);
        assert_eq!(
            app.compare.as_ref().unwrap().views.map(|v| v.pan),
            [anchor; 2]
        );
    }
    fn auto_focus_fixture(ctx: &egui::Context) -> App {
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, ["a.jpg", "b.jpg", "c.jpg"].map(PathBuf::from).into());
        app.settings.focus_on_unviewed = true;
        app.zoom = true;
        for i in 0..3 {
            app.photos[i].loaded = true;
            app.photos[i].info = Some(
                ArwInfo::read(std::path::Path::new("tests/fixtures/preview-3x2.jpg")).unwrap(),
            );
            app.photos[i].full_size = Some(Vec2::new(6000., 4000.));
            app.focus_overlay.insert_fixture(
                app.paths[i].clone(),
                fastcull::photo_focus::Info {
                    source: "fixture",
                    areas: vec![fastcull::photo_focus::Area {
                        center: Pos2::new(0.75, 0.25),
                        size: None,
                    }],
                },
            );
        }
        app
    }
    #[test]
    fn auto_focus_only_on_first_zoomed_visit_and_restores_manual_position() {
        let ctx = egui::Context::default();
        let mut app = auto_focus_fixture(&ctx);
        let viewport = Vec2::new(800., 600.);
        app.canvas_size = viewport;
        app.pan = Vec2::new(0.1, 0.1);
        app.next(1);
        assert_eq!(app.pending_focus, Some(1));
        app.apply_auto_focus(1, viewport, &ctx);
        assert_eq!(app.pan, Vec2::new(-0.25, 0.25));
        assert!(
            !app.focus_overlay.enabled,
            "automatic positioning must not force the overlay on"
        );
        app.move_view(Vec2::new(300., -200.), ctx.pixels_per_point());
        let manual = app.pan;
        app.next(-1);
        assert_eq!(app.pan, Vec2::new(0.1, 0.1));
        app.next(1);
        assert_eq!(app.pending_focus, None);
        app.apply_auto_focus(1, viewport, &ctx);
        assert_eq!(app.pan, manual);
        app.next(1);
        app.photos[2].values.orientation = Some(6);
        app.apply_auto_focus(2, viewport, &ctx);
        assert_eq!(app.pan, Vec2::new(-0.25, -0.25));
    }
    #[test]
    fn auto_focus_respects_disabled_fit_visited_and_grid_navigation() {
        let ctx = egui::Context::default();
        for case in 0..4 {
            let mut app = auto_focus_fixture(&ctx);
            app.pan = Vec2::new(0.1, 0.2);
            match case {
                0 => app.settings.focus_on_unviewed = false,
                1 => app.zoom = false,
                2 => {
                    app.viewed.insert(1);
                }
                _ => app.grid = true,
            }
            app.next(1);
            assert_eq!(app.pending_focus, None, "case {case}");
            app.apply_auto_focus(1, Vec2::new(800., 600.), &ctx);
            assert_eq!(
                app.pan,
                if case == 1 {
                    Vec2::ZERO
                } else {
                    Vec2::new(0.1, 0.2)
                }
            );
        }
    }
    #[test]
    fn auto_focus_waits_for_geometry_cancels_on_input_and_handles_missing_points() {
        let ctx = egui::Context::default();
        let mut app = auto_focus_fixture(&ctx);
        let viewport = Vec2::new(800., 600.);
        app.canvas_size = viewport;
        app.next(1);
        app.photos[1].full_size = None;
        app.apply_auto_focus(1, viewport, &ctx);
        assert_eq!(app.pending_focus, Some(1));
        app.move_view(Vec2::new(20., 0.), 1.);
        app.photos[1].full_size = Some(Vec2::new(6000., 4000.));
        app.apply_auto_focus(1, viewport, &ctx);
        assert_eq!(
            app.pan,
            Vec2::ZERO,
            "late data must not override a manual gesture"
        );
        app.next(1);
        app.next(-1);
        app.apply_auto_focus(2, viewport, &ctx);
        assert_eq!(
            app.pan,
            Vec2::ZERO,
            "a stale response must not move a different photo"
        );
        app.focus_overlay
            .insert_fixture(app.paths[2].clone(), Default::default());
        app.viewed.remove(&2);
        app.loupe_positions.remove(&2);
        app.next(1);
        app.pan = Vec2::new(0.1, -0.1);
        app.apply_auto_focus(2, viewport, &ctx);
        assert_eq!(app.pan, Vec2::new(0.1, -0.1));
        assert_eq!(app.pending_focus, None);
    }
    #[test]
    fn auto_focus_compare_centers_only_the_new_candidate_and_clamps_to_photo_edges() {
        let ctx = egui::Context::default();
        let mut app = auto_focus_fixture(&ctx);
        let reference = Vec2::new(0.1, 0.2);
        app.compare = Some(Compare {
            indices: [0, 1],
            views: [View {
                zoom: 1.,
                pan: reference,
            }; 2],
            active: 1,
            linked: true,
        });
        app.current = 1;
        app.go(2);
        app.photos[2].full_size = Some(Vec2::new(1000., 800.));
        app.apply_auto_focus(2, Vec2::new(800., 600.), &ctx);
        let c = app.compare.as_ref().unwrap();
        assert_eq!(c.views[0].pan, reference);
        assert_eq!(c.views[1].pan, Vec2::new(-0.1, 0.125));
        assert_eq!(c.views.map(|v| v.zoom), [1., 1.]);
    }
    #[test]
    fn rotation_returns_to_original() {
        for base in 1..=8 {
            let mut o = base;
            for _ in 0..4 {
                o = clockwise(o);
            }
            assert_eq!(base, o);
        }
    }
    #[test]
    fn toolbar_focus_does_not_steal_or_duplicate_navigation() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        ctx.begin_pass(input);
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("toolbar-button")));
        assert!(ctx.wants_keyboard_input());
        assert!(shortcut(&ctx, Key::Space));
        assert!(!shortcut(&ctx, Key::Space));
        assert!(!ctx.input(|i| i.key_pressed(Key::Space)));
        let _ = ctx.end_pass();
    }
    #[test]
    fn texture_budget_protects_current() {
        let ctx = egui::Context::default();
        let mut c = TextureCache::new(2 * 64 * 64 * 4, 4);
        for i in 0..8 {
            c.insert(
                i,
                ctx.load_texture(
                    format!("test{i}"),
                    egui::ColorImage::filled([64, 64], Color32::WHITE),
                    TextureOptions::LINEAR,
                ),
                0,
            );
        }
        assert!(c.items.contains_key(&0));
        assert!(c.items.len() <= 2);
    }
    #[test]
    fn far_prefetch_cannot_evict_the_next_photo_on_a_small_preview_budget() {
        let ctx = egui::Context::default();
        let mut c = TextureCache::new(2 * 64 * 64 * 4, 4);
        for i in 0..3 {
            c.insert_near(
                i,
                ctx.load_texture(
                    format!("preview{i}"),
                    egui::ColorImage::filled([64, 64], Color32::WHITE),
                    TextureOptions::LINEAR,
                ),
                0,
            );
        }
        assert!(c.items.contains_key(&0));
        assert!(c.items.contains_key(&1));
        assert!(!c.items.contains_key(&2));
    }
    #[test]
    fn raw_modal_gate_blocks_main_frame_navigation_and_grading_then_recovers() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")]);
        let mut frame = eframe::Frame::_new_kittest();
        app.raw_windows.set_busy_for_test(true);
        let input = |keys: Vec<Key>| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280., 850.))),
            events: keys
                .into_iter()
                .map(|key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                })
                .collect(),
            ..Default::default()
        };
        let _ = ctx.run(input(vec![Key::ArrowRight, Key::Num5]), |ctx| {
            eframe::App::update(&mut app, ctx, &mut frame)
        });
        assert_eq!(app.current, 0);
        assert_eq!(app.photos[0].values.rating, None);
        assert_eq!(app.photos[1].values.rating, None);
        app.raw_windows.set_busy_for_test(false);
        let _ = ctx.run(input(vec![Key::ArrowRight]), |ctx| {
            eframe::App::update(&mut app, ctx, &mut frame)
        });
        assert_eq!(app.current, 1);
    }
    #[test]
    fn raw_save_becomes_the_new_main_exposure_anchor_and_protects_crop_from_stale_lr_data() {
        let crop = fastcull::crop::Crop {
            enabled: true,
            left: 100000,
            right: 900000,
            top: 200000,
            bottom: 800000,
            angle: 0,
        };
        let mut photo = Photo {
            exposure_base: Some(0),
            exposure_delta: 1.,
            ..Default::default()
        };
        let values = Values {
            exposure: Some(350000),
            rating: Some(4),
            crop: Some(crop),
            orientation: Some(6),
            ..Default::default()
        };
        let updates = Updates {
            exposure: Some(350000),
            rating: Some(4),
            crop: Some(crop),
            orientation: Some(6),
            ..Default::default()
        };
        photo.apply_raw_saved(values, updates);
        photo.apply_develop_baseline(crate::exposure::Baseline {
            exposure: Some(0),
            crop: Some(Default::default()),
            orientation: Some(1),
            ..Default::default()
        });
        assert_eq!(photo.exposure_base, Some(350000));
        assert_eq!(photo.exposure_delta, 0.);
        assert_eq!(photo.values.crop, Some(crop));
        assert_eq!(photo.values.orientation, Some(6));
        assert_eq!(photo.values.rating, Some(4));
    }
}
