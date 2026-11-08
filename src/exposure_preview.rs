//! GPU exposure preview on the existing embedded-JPEG texture.
#[cfg(not(any(windows, target_os = "macos")))]
#[path = "exposure_preview_gl.rs"]
mod backend;
#[cfg(not(any(windows, target_os = "macos")))]
pub use backend::Preview;

#[cfg(any(windows, target_os = "macos"))]
pub struct Preview(std::sync::Arc<crate::raw_preview::Preview>);
#[cfg(any(windows, target_os = "macos"))]
impl Preview {
    pub fn new(state: &eframe::egui_wgpu::RenderState) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(crate::raw_preview::Preview::new(state)))
    }
    pub fn paint(
        self: &std::sync::Arc<Self>,
        painter: &eframe::egui::Painter,
        canvas: eframe::egui::Rect,
        rect: eframe::egui::Rect,
        texture: eframe::egui::TextureId,
        uvs: [eframe::egui::Pos2; 4],
        delta: f32,
    ) {
        self.0.paint(
            painter,
            canvas,
            rect,
            texture,
            uvs,
            crate::raw_preview::Settings {
                exposure: delta,
                ..Default::default()
            },
        );
    }
    pub fn error(&self) -> Option<String> {
        self.0.error()
    }
    pub fn destroy(&self) {
        self.0.destroy();
    }
}
