//! Native graphics policy: DirectX 12 on Windows, Metal on macOS; no GL fallback.
use eframe::egui;

fn app_icon() -> std::sync::Arc<egui::IconData> {
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        std::sync::Arc::new(
            eframe::icon_data::from_png_bytes(include_bytes!("../assets/fastcull.png"))
                .expect("embedded FastCull icon must be a valid PNG"),
        )
    })
    .clone()
}

pub fn raw_preview(
    cc: &eframe::CreationContext<'_>,
) -> Option<std::sync::Arc<crate::raw_preview::Preview>> {
    #[cfg(target_os = "macos")]
    crate::macos_color::configure(cc);
    #[cfg(any(windows, target_os = "macos"))]
    {
        cc.wgpu_render_state
            .as_ref()
            .map(crate::raw_preview::Preview::new)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        cc.gl
            .as_ref()
            .map(|gl| crate::raw_preview::Preview::new(gl.clone()))
    }
}

pub fn exposure_preview(
    cc: &eframe::CreationContext<'_>,
) -> Option<std::sync::Arc<crate::exposure_preview::Preview>> {
    #[cfg(target_os = "macos")]
    crate::macos_color::configure(cc);
    #[cfg(any(windows, target_os = "macos"))]
    {
        cc.wgpu_render_state
            .as_ref()
            .map(crate::exposure_preview::Preview::new)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        cc.gl
            .as_ref()
            .map(|gl| crate::exposure_preview::Preview::new(gl.clone()))
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub fn native_options(viewport: egui::ViewportBuilder) -> eframe::NativeOptions {
    use eframe::{egui_wgpu, wgpu};
    use std::sync::Arc;

    let mut setup = egui_wgpu::WgpuSetupCreateNew::default();
    setup.instance_descriptor.backends = native_backends();
    setup.native_adapter_selector = Some(Arc::new(|adapters, surface| {
        let selected = adapters
            .iter()
            .filter(|adapter| surface.is_none_or(|s| adapter.is_surface_supported(s)))
            .min_by_key(|adapter| {
                let info = adapter.get_info();
                match (info.vendor, info.device_type) {
                    (0x10de, wgpu::DeviceType::DiscreteGpu) => 0,
                    (_, wgpu::DeviceType::DiscreteGpu) => 1,
                    (_, wgpu::DeviceType::IntegratedGpu) => 2,
                    (_, wgpu::DeviceType::VirtualGpu) => 3,
                    (_, wgpu::DeviceType::Other) => 4,
                    (_, wgpu::DeviceType::Cpu) => 5,
                }
            })
            .cloned()
            .ok_or_else(|| "没有支持当前窗口的原生图形设备".to_owned())?;
        log::info!(
            "graphics backend={:?} adapter={:?}",
            selected.get_info().backend,
            selected.get_info()
        );
        Ok(selected)
    }));
    // Preserve native detail for cameras wider than egui's default 8192 pixels.
    let device_descriptor = setup.device_descriptor.clone();
    setup.device_descriptor = Arc::new(move |adapter| {
        let mut descriptor = device_descriptor(adapter);
        descriptor.required_limits.max_texture_dimension_2d =
            adapter.limits().max_texture_dimension_2d;
        descriptor
    });

    eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
            ..Default::default()
        },
        viewport: viewport.with_icon(app_icon()),
        ..Default::default()
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn native_options(viewport: egui::ViewportBuilder) -> eframe::NativeOptions {
    eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: viewport.with_icon(app_icon()),
        ..Default::default()
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub fn native_backends() -> eframe::wgpu::Backends {
    #[cfg(windows)]
    {
        eframe::wgpu::Backends::DX12
    }
    #[cfg(target_os = "macos")]
    {
        eframe::wgpu::Backends::METAL
    }
}
