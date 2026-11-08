//! DirectX GPU preview. Every draw owns its uniforms, including both compare panes.
use super::{LinearImage, Recipe, Settings, valid_curve, valid_lookup};
use eframe::{egui, egui_wgpu, wgpu};
use std::sync::{Arc, Mutex};
use wgpu::util::DeviceExt;

const SHADER: &str = include_str!("raw_preview.wgsl");

struct Pipelines {
    layout: wgpu::BindGroupLayout,
    scene: wgpu::RenderPipeline,
    jpeg: wgpu::RenderPipeline,
    srgb_target: bool,
}
impl Pipelines {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RAW preview layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                buffer_layout(
                    1,
                    wgpu::BufferBindingType::Storage { read_only: true },
                    wgpu::ShaderStages::FRAGMENT,
                ),
                buffer_layout(
                    2,
                    wgpu::BufferBindingType::Storage { read_only: true },
                    wgpu::ShaderStages::FRAGMENT,
                ),
                buffer_layout(
                    3,
                    wgpu::BufferBindingType::Uniform,
                    wgpu::ShaderStages::VERTEX_FRAGMENT,
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RAW preview pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RAW / JPEG preview WGSL"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = |entry| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        Self {
            scene: pipeline("fs_scene"),
            jpeg: pipeline("fs_jpeg"),
            layout,
            srgb_target: format.is_srgb(),
        }
    }
}
fn buffer_layout(
    binding: u32,
    ty: wgpu::BufferBindingType,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn buffer(device: &wgpu::Device, data: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("RAW preview data"),
        contents: data,
        usage,
    })
}
struct GpuImage {
    texture: wgpu::Texture,
    curve: wgpu::Buffer,
    look: wgpu::Buffer,
}
enum ImageState {
    Pending(Vec<[f32; 4]>),
    Ready(GpuImage),
    Failed,
}
pub struct LinearTexture {
    size: [usize; 2],
    recipe: Recipe,
    state: Mutex<ImageState>,
}
impl LinearTexture {
    pub fn size_vec2(&self) -> egui::Vec2 {
        egui::vec2(self.size[0] as f32, self.size[1] as f32)
    }
}

pub struct Preview {
    state: egui_wgpu::RenderState,
    pipelines: Arc<Pipelines>,
    identity_curve: wgpu::Buffer,
    identity_look: wgpu::Buffer,
    error: Mutex<Option<String>>,
}
impl Preview {
    pub fn new(state: &egui_wgpu::RenderState) -> Arc<Self> {
        let device = &state.device;
        Arc::new(Self {
            pipelines: Arc::new(Pipelines::new(device, state.target_format)),
            identity_curve: buffer(
                device,
                bytemuck::cast_slice(&[0f32, 1.]),
                wgpu::BufferUsages::STORAGE,
            ),
            identity_look: buffer(
                device,
                bytemuck::cast_slice(&[0f32, 1., 1., 0.]),
                wgpu::BufferUsages::STORAGE,
            ),
            state: state.clone(),
            error: Mutex::new(None),
        })
    }
    pub fn upload(self: &Arc<Self>, image: LinearImage) -> Arc<LinearTexture> {
        *self.error.lock().unwrap() = None;
        Arc::new(LinearTexture {
            size: image.size,
            recipe: image.recipe,
            state: Mutex::new(ImageState::Pending(image.pixels)),
        })
    }
    pub fn paint(
        self: &Arc<Self>,
        painter: &egui::Painter,
        canvas: egui::Rect,
        rect: egui::Rect,
        texture: egui::TextureId,
        uvs: [egui::Pos2; 4],
        settings: Settings,
    ) {
        // UI runs before texture updates. Retry a new egui texture next frame;
        // never lock the renderer from inside its own prepare/paint callbacks.
        let image = self
            .state
            .renderer
            .read()
            .texture(&texture)
            .and_then(|t| t.texture.clone());
        let Some(image) = image else {
            painter.ctx().request_repaint();
            return;
        };
        let gpu = GpuImage {
            texture: image,
            curve: self.identity_curve.clone(),
            look: self.identity_look.clone(),
        };
        let params = parameters(
            canvas,
            rect,
            uvs,
            settings,
            &Recipe::default(),
            false,
            self.pipelines.srgb_target,
        );
        let draw = Draw::new(&self.state.device, &self.pipelines, &gpu, params);
        painter.add(egui_wgpu::Callback::new_paint_callback(
            canvas,
            Callback {
                preview: self.clone(),
                frame: None,
                draw: Mutex::new(Some(draw)),
                params,
                scene: false,
            },
        ));
    }
    pub fn paint_linear(
        self: &Arc<Self>,
        painter: &egui::Painter,
        canvas: egui::Rect,
        rect: egui::Rect,
        texture: Arc<LinearTexture>,
        uvs: [egui::Pos2; 4],
        settings: Settings,
    ) {
        let params = parameters(
            canvas,
            rect,
            uvs,
            settings,
            &texture.recipe,
            true,
            self.pipelines.srgb_target,
        );
        painter.add(egui_wgpu::Callback::new_paint_callback(
            canvas,
            Callback {
                preview: self.clone(),
                frame: Some(texture),
                draw: Mutex::new(None),
                params,
                scene: true,
            },
        ));
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
    pub fn destroy(&self) {
        // wgpu resources are reference counted and retire with their frame/draw.
        // No GL context, global garbage queue, or device-wide wait is needed.
    }
}

fn validate_image(
    size: [usize; 2],
    pixels: &[[f32; 4]],
    recipe: &Recipe,
    limits: &wgpu::Limits,
) -> Result<(), String> {
    let [w, h] = size;
    if w == 0
        || h == 0
        || w > limits.max_texture_dimension_2d as usize
        || h > limits.max_texture_dimension_2d as usize
        || w.checked_mul(h) != Some(pixels.len())
    {
        return Err("浮点 RAW 预览尺寸无效或超过显卡纹理上限".into());
    }
    if pixels.iter().flatten().any(|v| !v.is_finite()) {
        return Err("浮点 RAW 预览含无效数值".into());
    }
    if !valid_curve(&recipe.curve) || !recipe.baseline_exposure.is_finite() {
        return Err("RAW 色调曲线或曝光基线无效".into());
    }
    if [recipe.look.as_ref(), recipe.extra_look.as_ref()]
        .into_iter()
        .flatten()
        .any(|v| {
            !valid_lookup(v)
                || v.encoding > 1
                || v.data.len() as u64 * 16 > limits.max_storage_buffer_binding_size as u64
        })
    {
        return Err("RAW 色彩查找表尺寸、数值或编码无效".into());
    }
    let cells = recipe.look.as_ref().map_or(1, |v| v.data.len())
        + recipe.extra_look.as_ref().map_or(1, |v| v.data.len());
    if cells as u64 * 16 > limits.max_storage_buffer_binding_size as u64 {
        return Err("RAW 查找表合计超过显卡上限".into());
    }
    Ok(())
}
fn upload_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [usize; 2],
    pixels: &[[f32; 4]],
    recipe: &Recipe,
) -> Result<GpuImage, String> {
    validate_image(size, pixels, recipe, &device.limits())?;
    let extent = wgpu::Extent3d {
        width: size[0] as u32,
        height: size[1] as u32,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("linear RAW float32"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(pixels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(extent.width * 16),
            rows_per_image: None,
        },
        extent,
    );
    let curve = buffer(
        device,
        bytemuck::cast_slice(&recipe.curve),
        wgpu::BufferUsages::STORAGE,
    );
    let lookup: Vec<[f32; 4]> = [recipe.look.as_ref(), recipe.extra_look.as_ref()]
        .into_iter()
        .flat_map(|m| {
            m.map(|m| {
                m.data
                    .iter()
                    .map(|v| [v[0], v[1], v[2], 0.])
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![[0., 1., 1., 0.]])
        })
        .collect();
    let look = buffer(
        device,
        bytemuck::cast_slice(&lookup),
        wgpu::BufferUsages::STORAGE,
    );
    Ok(GpuImage {
        texture,
        curve,
        look,
    })
}

type Parameters = [[f32; 4]; 9];
fn parameters(
    canvas: egui::Rect,
    rect: egui::Rect,
    uvs: [egui::Pos2; 4],
    settings: Settings,
    recipe: &Recipe,
    scene: bool,
    srgb_target: bool,
) -> Parameters {
    let dims = recipe
        .look
        .as_ref()
        .map(|m| {
            [
                m.dims[0] as f32,
                m.dims[1] as f32,
                m.dims[2] as f32,
                m.encoding as f32,
            ]
        })
        .unwrap_or([1., 1., 1., 0.]);
    let exposure = if scene {
        (settings.exposure + recipe.baseline_exposure).clamp(-20., 20.)
    } else {
        settings.exposure
    };
    [
        [
            2. * (rect.left() - canvas.left()) / canvas.width() - 1.,
            1. - 2. * (rect.top() - canvas.top()) / canvas.height(),
            2. * (rect.right() - canvas.left()) / canvas.width() - 1.,
            1. - 2. * (rect.bottom() - canvas.top()) / canvas.height(),
        ],
        [uvs[0].x, uvs[0].y, uvs[1].x, uvs[1].y],
        [uvs[2].x, uvs[2].y, uvs[3].x, uvs[3].y],
        [
            settings.basic[0],
            settings.basic[1],
            settings.basic[2],
            settings.basic[3],
        ],
        [settings.basic[4], settings.basic[5], settings.basic[6], 0.],
        [
            settings.wb[0],
            settings.wb[1],
            settings.wb[2],
            2f32.powf(exposure),
        ],
        dims,
        [
            recipe.curve.len() as f32,
            u32::from(srgb_target) as f32,
            recipe.look.as_ref().map_or(1, |m| m.data.len()) as f32,
            0.,
        ],
        recipe
            .extra_look
            .as_ref()
            .map(|m| {
                [
                    m.dims[0] as f32,
                    m.dims[1] as f32,
                    m.dims[2] as f32,
                    m.encoding as f32,
                ]
            })
            .unwrap_or([1., 1., 1., 0.]),
    ]
}
struct Draw {
    bindings: wgpu::BindGroup,
}
impl Draw {
    fn new(
        device: &wgpu::Device,
        pipelines: &Pipelines,
        image: &GpuImage,
        params: Parameters,
    ) -> Self {
        let uniform = buffer(
            device,
            bytemuck::cast_slice(&params),
            wgpu::BufferUsages::UNIFORM,
        );
        let view = image.texture.create_view(&Default::default());
        Self {
            bindings: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RAW preview draw"),
                layout: &pipelines.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: image.curve.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: image.look.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            }),
        }
    }
}
struct Callback {
    preview: Arc<Preview>,
    frame: Option<Arc<LinearTexture>>,
    draw: Mutex<Option<Draw>>,
    params: Parameters,
    scene: bool,
}
impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _: &egui_wgpu::ScreenDescriptor,
        _: &mut wgpu::CommandEncoder,
        _: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(frame) = &self.frame {
            let mut state = frame.state.lock().unwrap();
            if let ImageState::Pending(pixels) = &*state {
                *state = match upload_image(device, queue, frame.size, pixels, &frame.recipe) {
                    Ok(image) => ImageState::Ready(image),
                    Err(error) => {
                        log::error!("DirectX RAW upload: {error}");
                        *self.preview.error.lock().unwrap() = Some(error);
                        ImageState::Failed
                    }
                };
            }
            if let ImageState::Ready(image) = &*state {
                *self.draw.lock().unwrap() = Some(Draw::new(
                    device,
                    &self.preview.pipelines,
                    image,
                    self.params,
                ));
            }
        }
        Vec::new()
    }
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _: &egui_wgpu::CallbackResources,
    ) {
        if let Some(draw) = &*self.draw.lock().unwrap() {
            let pipelines = &self.preview.pipelines;
            pass.set_pipeline(if self.scene {
                &pipelines.scene
            } else {
                &pipelines.jpeg
            });
            pass.set_bind_group(0, &draw.bindings, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

#[cfg(test)]
#[path = "raw_preview_wgpu_tests.rs"]
mod tests;
