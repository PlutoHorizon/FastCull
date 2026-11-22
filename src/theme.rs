//! FastCull design tokens.
//!
//! Every colour, type size, spacing step and corner radius used by the
//! interface is read from here, so the look can be retuned in one place.
//! The values follow docs/UI-REDESIGN-SPEC.md, section "一、设计令牌".
//!
//! Only appearance lives in this module: it never touches shortcuts, state
//! transitions or anything that is written back to a photo.

// The palette is defined as a whole; a few tokens are only wired up in later
// redesign stages.
#![allow(dead_code)]

use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, Stroke, TextStyle, Vec2};

// -- Colours -------------------------------------------------------------------------
//
// The window is built from four materials, and the whole look depends on them
// staying far enough apart to be told apart at a glance:
//
//   current().base      the window behind everything
//   current().canvas    the dark table the photo stands on
//   current().band      a recessed panel: the rail and the sidebar
//   current().raised    a panel that catches the light: the title bar and the toolbar
//
// A control sits one step above a band, so the depth of the interface is
// carried by value, not by shadows on every rectangle.
//
// The values follow the appearance the user picked: the upstream appearance
// module owns the light/dark choice, and every material below is derived
// from it, so switching the appearance keeps every surface consistent.

/// Every colour the interface paints with.
#[derive(Clone, Copy)]
pub struct Tokens {
    /// The window base. The gap between the panels shows it.
    pub base: Color32,
    /// The area the photo stands on.
    pub canvas: Color32,
    /// The top of the photo canvas gradient: a touch of light behind the photo.
    pub canvas_top: Color32,
    /// The bottom of the photo canvas gradient.
    pub canvas_bottom: Color32,
    /// A recessed panel: the activity rail and the filter sidebar.
    pub band: Color32,
    /// A raised panel: the title bar, the toolbar and the filmstrip.
    pub raised: Color32,
    /// A floating surface: the rail, the sidebar, the top bar, the toolbar.
    pub surface: Color32,
    /// The lit upper edge of a panel.
    pub surface_hi: Color32,
    /// A control resting on a panel.
    pub surface_2: Color32,
    /// A hovered control.
    pub surface_3: Color32,
    /// A hairline.
    pub line: Color32,
    /// A stronger hairline, used for selection frames.
    pub line_2: Color32,
    /// The shadow a floating card casts on the base.
    pub card_shadow: Color32,
    /// The rim of a photo on the canvas: a hairline of light, not a dark outline.
    pub photo_rim: Color32,
    /// The pool of light the photo stands in.
    pub photo_glow: Color32,
    /// Body copy and headings.
    pub text: Color32,
    /// Secondary notes and EXIF summaries.
    pub text_secondary: Color32,
    /// Placeholders, counters and disabled text.
    pub text_muted: Color32,
    /// Selection, primary action and focus.
    pub accent: Color32,
    /// Selected state background.
    pub accent_soft: Color32,
    /// Pick.
    pub success: Color32,
    /// Warnings and exposure readouts.
    pub warning: Color32,
    /// A rating. Gold reads as a rating the way blue never does.
    pub star: Color32,
    /// Reject and errors.
    pub danger: Color32,
    /// Soft shadow under a photo on the canvas.
    pub shadow: Color32,
    /// The wide, soft shadow that lifts a photo off the canvas.
    pub shadow_ambient: Color32,
    /// The tight shadow that anchors a photo to the canvas.
    pub shadow_contact: Color32,
}

impl Tokens {
    /// The materials for one appearance.
    ///
    /// The dark set is the redesign's own. The light set keeps the same
    /// relationships: a recessed band under a raised panel, a canvas the
    /// photo stands out of, and text that reads on both.
    pub fn for_theme(theme: egui::Theme) -> Tokens {
        let p = crate::appearance::Palette::for_theme(theme);
        match theme {
            egui::Theme::Light => Tokens {
                base: Color32::from_rgb(219, 222, 227),
                canvas: Color32::from_rgb(236, 238, 242),
                canvas_top: Color32::from_rgb(243, 245, 248),
                canvas_bottom: Color32::from_rgb(228, 231, 236),
                band: p.rail,
                raised: p.panel,
                surface: p.panel,
                surface_hi: Color32::from_rgb(252, 252, 253),
                surface_2: Color32::from_rgb(255, 255, 255),
                surface_3: p.hover,
                line: p.border,
                line_2: Color32::from_rgb(203, 209, 217),
                card_shadow: Color32::from_black_alpha(46),
                photo_rim: Color32::from_rgba_unmultiplied_const(0, 0, 0, 22),
                photo_glow: Color32::from_rgba_unmultiplied_const(0, 0, 0, 10),
                text: p.text,
                text_secondary: Color32::from_rgb(92, 102, 116),
                text_muted: Color32::from_rgb(132, 140, 152),
                accent: p.accent,
                accent_soft: p.selected,
                success: Color32::from_rgb(45, 133, 80),
                warning: Color32::from_rgb(170, 118, 24),
                star: Color32::from_rgb(197, 143, 24),
                danger: p.danger,
                shadow: Color32::from_black_alpha(38),
                shadow_ambient: Color32::from_black_alpha(44),
                shadow_contact: Color32::from_black_alpha(62),
            },
            egui::Theme::Dark => Tokens {
                base: Color32::from_rgb(7, 8, 10),
                canvas: Color32::from_rgb(7, 8, 10),
                canvas_top: Color32::from_rgb(16, 18, 23),
                canvas_bottom: Color32::from_rgb(6, 7, 9),
                band: Color32::from_rgb(20, 23, 29),
                raised: Color32::from_rgb(26, 30, 37),
                surface: Color32::from_rgb(26, 30, 37),
                surface_hi: Color32::from_rgb(56, 64, 78),
                surface_2: Color32::from_rgb(33, 38, 46),
                surface_3: Color32::from_rgb(43, 49, 60),
                line: Color32::from_rgb(40, 46, 56),
                line_2: Color32::from_rgb(58, 66, 78),
                card_shadow: Color32::from_black_alpha(150),
                photo_rim: Color32::from_rgba_unmultiplied_const(255, 255, 255, 34),
                photo_glow: Color32::from_rgba_unmultiplied_const(255, 255, 255, 9),
                text: Color32::from_rgb(231, 235, 241),
                text_secondary: Color32::from_rgb(166, 174, 186),
                text_muted: Color32::from_rgb(138, 147, 160),
                accent: Color32::from_rgb(90, 169, 255),
                accent_soft: Color32::from_rgb(27, 58, 99),
                success: Color32::from_rgb(95, 203, 138),
                warning: Color32::from_rgb(232, 190, 120),
                star: Color32::from_rgb(242, 193, 78),
                danger: Color32::from_rgb(238, 108, 104),
                shadow: Color32::from_black_alpha(120),
                shadow_ambient: Color32::from_black_alpha(140),
                shadow_contact: Color32::from_black_alpha(190),
            },
        }
    }
}

/// The tokens the interface paints with right now.
static TOKENS: std::sync::OnceLock<std::sync::RwLock<Tokens>> = std::sync::OnceLock::new();

fn store() -> &'static std::sync::RwLock<Tokens> {
    TOKENS.get_or_init(|| {
        std::sync::RwLock::new(Tokens::for_theme(egui::Theme::Dark))
    })
}

/// The tokens of the current appearance.
fn current() -> Tokens {
    *store().read().unwrap_or_else(|e| e.into_inner())
}

/// Follow the appearance the user picked.
///
/// The appearance can change while the window is open, so the tokens are
/// refreshed whenever the theme behind the context differs from the one the
/// stored tokens were built for.
pub fn refresh(ctx: &egui::Context) {
    let theme = ctx.theme();
    let mut guard = store().write().unwrap_or_else(|e| e.into_inner());
    let wanted = Tokens::for_theme(theme);
    if guard.base != wanted.base || guard.canvas != wanted.canvas {
        *guard = wanted;
    }
}

/// The window base. The gap between the panels shows it.
pub fn base() -> Color32 {
    current().base
}
/// The area the photo stands on.
pub fn canvas() -> Color32 {
    current().canvas
}
/// The top of the photo canvas gradient.
pub fn canvas_top() -> Color32 {
    current().canvas_top
}
/// The bottom of the photo canvas gradient.
pub fn canvas_bottom() -> Color32 {
    current().canvas_bottom
}
/// A recessed panel: the activity rail and the filter sidebar.
pub fn band() -> Color32 {
    current().band
}
/// A raised panel: the title bar, the toolbar and the filmstrip.
pub fn raised() -> Color32 {
    current().raised
}
/// A floating surface: the rail, the sidebar, the top bar, the toolbar.
pub fn surface() -> Color32 {
    current().surface
}
/// The lit upper edge of a panel.
pub fn surface_hi() -> Color32 {
    current().surface_hi
}
/// A control resting on a panel.
pub fn surface_2() -> Color32 {
    current().surface_2
}
/// A hovered control.
pub fn surface_3() -> Color32 {
    current().surface_3
}
/// A hairline.
pub fn line() -> Color32 {
    current().line
}
/// A stronger hairline, used for selection frames.
pub fn line_2() -> Color32 {
    current().line_2
}
/// The shadow a floating card casts on the base.
pub fn card_shadow() -> Color32 {
    current().card_shadow
}
/// The rim of a photo on the canvas.
pub fn photo_rim() -> Color32 {
    current().photo_rim
}
/// The pool of light the photo stands in.
pub fn photo_glow() -> Color32 {
    current().photo_glow
}
/// Body copy and headings.
pub fn text() -> Color32 {
    current().text
}
/// Secondary notes and EXIF summaries.
pub fn text_secondary() -> Color32 {
    current().text_secondary
}
/// Placeholders, counters and disabled text.
pub fn text_muted() -> Color32 {
    current().text_muted
}
/// Selection, primary action and focus.
pub fn accent() -> Color32 {
    current().accent
}
/// Selected state background.
pub fn accent_soft() -> Color32 {
    current().accent_soft
}
/// Pick.
pub fn success() -> Color32 {
    current().success
}
/// Warnings and exposure readouts.
pub fn warning() -> Color32 {
    current().warning
}
/// A rating.
pub fn star() -> Color32 {
    current().star
}
/// Reject and errors.
pub fn danger() -> Color32 {
    current().danger
}
/// Soft shadow under a photo on the canvas.
pub fn shadow() -> Color32 {
    current().shadow
}
/// The wide, soft shadow that lifts a photo off the canvas.
pub fn shadow_ambient() -> Color32 {
    current().shadow_ambient
}
/// The tight shadow that anchors a photo to the canvas.
pub fn shadow_contact() -> Color32 {
    current().shadow_contact
}

// -- Type scale ----------------------------------------------------------------------

/// Flags and stars drawn on top of filmstrip thumbnails.
pub const FS_CAPTION: f32 = 11.;
/// Counters, hints and auxiliary text.
pub const FS_SMALL: f32 = 12.;
/// The label of a section, set apart by its spacing rather than its weight.
pub const FS_SECTION: f32 = 12.;
/// Control text (theme default for Button and Body).
pub const FS_BODY: f32 = 13.5;
/// Body copy and list rows.
pub const FS_LABEL: f32 = 14.;
/// File names and panel titles.
pub const FS_TITLE: f32 = 17.;
/// The file name in the top bar: the one piece of type that names the photo.
pub const FS_HERO: f32 = 21.;

/// Dialog titles.
pub const FS_DISPLAY: f32 = 24.;

// -- Spacing and radii ---------------------------------------------------------------

pub const SP_XS: f32 = 4.;
pub const SP_S: f32 = 8.;
pub const SP_M: f32 = 12.;
pub const SP_L: f32 = 16.;

/// Input boxes and colour chips.
pub const RADIUS_SM: f32 = 5.;
/// Buttons and cards.
pub const RADIUS: f32 = 7.;

/// Corner radius of the photo on the canvas.
///
/// The token table lists `RADIUS_LG = 12` for photos, but section 3.5 of the
/// spec spells the photo radius out as 6px, so the photo uses its own step.
pub const RADIUS_PHOTO: f32 = 6.;

/// Proportional font at one of the type-scale steps.
pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Corner radius built from one of the radius tokens.
pub fn radius(value: f32) -> CornerRadius {
    CornerRadius::same(value as u8)
}

/// Placeholder text for an input box.
///
/// The global `override_text_color` also colours hint text, so a plain
/// `hint_text("…")` would draw the placeholder in body text colour. Give the
/// hint an explicit muted colour instead: a placeholder must never read like
/// real content.
pub fn placeholder(text: impl Into<String>) -> egui::WidgetText {
    egui::RichText::new(text).color(current().text_muted).into()
}

/// A section label: small, wide-tracked and quiet.
///
/// The spacing is what separates it from the values under it. A heavier weight
/// at this size only made the panel look like a form.
pub fn section(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text)
        .size(FS_SECTION)
        .color(current().text_secondary)
        .extra_letter_spacing(1.6)
}

/// A heading: larger, and with the same wide tracking as a section label.
pub fn heading(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text)
        .size(FS_TITLE)
        .strong()
        .color(current().text)
        .extra_letter_spacing(0.4)
}

// -- Input fields --------------------------------------------------------------------

/// A framed single-line input.
///
/// egui paints its own text edit with `extreme_bg_color`, which is the colour
/// of the window behind the panels: a hole in the middle of a control. The
/// field is drawn here instead, so it belongs to the same family as every
/// other control.
pub fn input(ui: &mut egui::Ui, text: &mut String, hint: &str, id: egui::Id) -> egui::Response {
    let height = ui.spacing().interact_size.y.max(28.);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, radius(RADIUS_SM), current().surface_2);
    painter.rect_stroke(
        rect,
        radius(RADIUS_SM),
        Stroke::new(1., current().line),
        egui::StrokeKind::Inside,
    );
    let inner = Rect::from_min_max(
        rect.left_top() + Vec2::new(8., 2.),
        rect.right_bottom() - Vec2::new(8., 2.),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.add(
            egui::TextEdit::singleline(text)
                .id(id)
                .frame(false)
                .desired_width(f32::INFINITY)
                .hint_text(placeholder(hint)),
        )
    })
    .inner
}

// -- Button roles -------------------------------------------------------------------

/// The main action of a group: a solid accent button with bold white text.
///
/// The fill can be swapped for a semantic colour (pick = `current().success`, reject =
/// `current().danger`) by the caller; the label and the borderless shape stay the same.
pub fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).strong())
        .fill(current().accent)
        .stroke(Stroke::NONE)
        .corner_radius(radius(RADIUS))
}

/// A secondary action: a raised neutral surface with a hairline border.
///
/// The surface and border colours come from the theme's widget visuals
/// (`current().surface_2` with a `current().line_2` hairline at rest) instead of being pinned
/// here, so the button still reacts to hover and press.
pub fn secondary(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).color(current().text))
        .fill(current().surface_2)
        .stroke(Stroke::new(1., current().line_2))
        .corner_radius(radius(RADIUS))
}

/// A secondary button in its selected state.
pub fn secondary_selected(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).color(current().accent).strong())
        .fill(current().accent_soft)
        .stroke(Stroke::new(1., current().accent.gamma_multiply(0.55)))
        .corner_radius(radius(RADIUS))
}

/// A flag action: pick or reject.
///
/// The two are mutually exclusive, so filling both of them all the time made
/// the toolbar look like two flags were set at once. At rest the button keeps
/// a dark tint of its own colour and coloured text; the solid fill is
/// reserved for the flag that is actually set.
pub fn flag_action(text: &str, color: Color32, active: bool) -> egui::Button<'_> {
    if active {
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).strong())
            .fill(color)
            .stroke(Stroke::NONE)
            .corner_radius(radius(RADIUS))
    } else {
        egui::Button::new(egui::RichText::new(text).color(color).strong())
            .fill(color.gamma_multiply(0.16))
            .stroke(Stroke::new(1., color.gamma_multiply(0.42)))
            .corner_radius(radius(RADIUS))
    }
}

/// A secondary button whose label is built from several runs of text, so a
/// count can be shown in a quieter colour than the label itself.
pub fn secondary_rich(text: egui::WidgetText) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(current().surface_2)
        .stroke(Stroke::new(1., current().line_2))
        .corner_radius(radius(RADIUS))
}

/// The selected variant of `secondary_rich`.
pub fn secondary_selected_rich(text: egui::WidgetText) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(current().accent_soft)
        .stroke(Stroke::new(1., current().accent.gamma_multiply(0.55)))
        .corner_radius(radius(RADIUS))
}

/// A toggle: a quiet control that lights up when it is on.
pub fn toggle(text: &str, active: bool) -> egui::Button<'_> {
    if active {
        egui::Button::new(egui::RichText::new(text).color(current().accent).strong())
            .fill(current().accent_soft)
            .stroke(Stroke::new(1., current().accent.gamma_multiply(0.55)))
            .corner_radius(radius(RADIUS))
    } else {
        egui::Button::new(egui::RichText::new(text).color(current().text_secondary))
            .fill(current().surface_2)
            .stroke(Stroke::new(1., current().line_2))
            .corner_radius(radius(RADIUS))
    }
}

/// A light action or a navigation step: no surface until it is hovered.
///
/// `frame_when_inactive(false)` keeps the frame hidden while the button is
/// idle, so the hovered surface only shows up under the pointer.
pub fn ghost(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).color(current().text_secondary))
        .frame_when_inactive(false)
        .stroke(Stroke::NONE)
        .corner_radius(radius(RADIUS))
}

/// A ghost button whose label marks the active state.
pub fn ghost_active(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).color(current().accent).strong())
        .frame_when_inactive(false)
        .stroke(Stroke::NONE)
        .corner_radius(radius(RADIUS))
}

/// A small ghost button, used for inline actions such as "重置".
pub fn ghost_small(text: &str) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(text)
            .size(FS_SMALL)
            .color(current().text_secondary),
    )
    .small()
    .frame_when_inactive(false)
    .stroke(Stroke::NONE)
    .corner_radius(radius(RADIUS))
}

/// Drop the native slider chrome so the caller can paint its own rail.
pub fn bare_slider(ui: &mut egui::Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    for widget in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
    ] {
        widget.bg_fill = Color32::TRANSPARENT;
        widget.weak_bg_fill = Color32::TRANSPARENT;
        widget.fg_stroke = Stroke::NONE;
        widget.bg_stroke = Stroke::NONE;
    }
}

/// Apply the FastCull theme to an egui context.
pub fn apply(ctx: &egui::Context) {
    // The appearance module has already chosen the theme; the materials here
    // follow it instead of forcing one look on the user.
    refresh(ctx);
    let t = current();
    let mut visuals = if ctx.theme() == egui::Theme::Light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.panel_fill = t.band;
    visuals.window_fill = t.raised;
    visuals.extreme_bg_color = t.base;
    visuals.override_text_color = Some(t.text);
    visuals.weak_text_color = Some(t.text_muted);
    visuals.selection.bg_fill = t.accent_soft;
    visuals.selection.stroke = Stroke::new(1., t.accent);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1., t.line);
    visuals.widgets.inactive.bg_fill = t.surface_2;
    visuals.widgets.inactive.weak_bg_fill = t.surface_2;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1., t.line_2);
    visuals.widgets.hovered.bg_fill = t.surface_3;
    visuals.widgets.hovered.weak_bg_fill = t.surface_3;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1., t.line_2);
    visuals.widgets.active.bg_fill = t.accent_soft;
    visuals.widgets.active.weak_bg_fill = t.accent_soft;
    visuals.widgets.active.bg_stroke = Stroke::new(1., t.accent);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.noninteractive,
    ] {
        // One radius for every control: a mixed set of 4px and 8px corners is
        // what makes an interface look assembled rather than designed.
        widget.corner_radius = radius(RADIUS);
    }
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.text_styles.insert(TextStyle::Body, font(FS_BODY));
        style.text_styles.insert(TextStyle::Button, font(FS_BODY));
        style.text_styles.insert(TextStyle::Small, font(FS_SMALL));
        style.text_styles.insert(TextStyle::Heading, font(FS_TITLE));
        // Monospace keeps the egui default.
        style.spacing.item_spacing = Vec2::splat(SP_S);
        style.spacing.button_padding = Vec2::new(SP_S, SP_XS);
        style.spacing.interact_size.y = 28.;
        // Indent is not part of the spacing scale; keep the previous value so
        // the sidebar trees stay where they were.
        style.spacing.indent = 14.;
    });
}
