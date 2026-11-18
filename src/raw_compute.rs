//! GPU entropy, Bayer interpolation and camera-RGB processing.
//! Results commit only after every bounded stripe succeeds. CPU remains the
//! reference/fallback, including on cancellation, device loss or unsupported GPUs.
use anyhow::{Context as _, Result, ensure};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use wgpu::util::DeviceExt;

pub struct Context {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: OnceLock<Result<Pipelines, String>>,
    ahd: OnceLock<Result<ahd::Pipelines, String>>,
    entropy: OnceLock<Result<entropy::Pipeline, String>>,
    scene: OnceLock<Result<scene::ScenePipelines, String>>,
    gate: Mutex<()>,
}
struct Pipelines {
    layout: wgpu::BindGroupLayout,
    x: wgpu::ComputePipeline,
    y: wgpu::ComputePipeline,
}
static ACTIVE: Mutex<Option<Arc<Context>>> = Mutex::new(None);
/// GPU AHD pipeline warmup. `FASTCULL_RAW_PREWARM=0` used to be the only switch,
/// which coupled "no session prewarm" with "no AHD shader warmup" and made the
/// T06 comparison uninterpretable. `FASTCULL_RAW_AHD_WARMUP=1|0` now overrides
/// just the warmup side (default keeps the historical coupling).
pub fn demosaic_warmup_enabled() -> bool {
    let override_value = std::env::var("FASTCULL_RAW_AHD_WARMUP").unwrap_or_default();
    if override_value == "0" {
        return false;
    }
    let forced = override_value == "1";
    ACTIVE.lock().unwrap().is_some()
        && !std::env::var("FASTCULL_RAW_COMPUTE").is_ok_and(|v| v == "cpu")
        && !std::env::var("FASTCULL_RAW_DEMOSAIC").is_ok_and(|v| v == "cpu")
        && (forced || !std::env::var("FASTCULL_RAW_PREWARM").is_ok_and(|v| v == "0"))
}
pub fn prewarm_demosaic() {
    let context = ACTIVE.lock().unwrap().clone();
    if let Some(context) = context
        && let Err(error) = context.prewarm_demosaic()
    {
        log::debug!("RAW AHD prewarm unavailable: {error:#}");
    }
}
pub fn display_hue_sat_enabled() -> bool {
    ACTIVE.lock().unwrap().is_some()
        && !std::env::var("FASTCULL_RAW_COMPUTE").is_ok_and(|v| v == "cpu")
        && !std::env::var("FASTCULL_RAW_HSM").is_ok_and(|v| v == "cpu")
}
pub fn install(device: wgpu::Device, queue: wgpu::Queue) {
    *ACTIVE.lock().unwrap() = Some(Arc::new(Context::new(device, queue)));
}
impl Context {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            device,
            queue,
            pipelines: OnceLock::new(),
            ahd: OnceLock::new(),
            entropy: OnceLock::new(),
            scene: OnceLock::new(),
            gate: Mutex::new(()),
        }
    }
    fn pipelines(&self) -> Result<&Pipelines> {
        self.pipelines
            .get_or_init(|| self.create_pipelines().map_err(|e| format!("{e:#}")))
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))
    }
    fn create_pipelines(&self) -> Result<Pipelines> {
        let d = &self.device;
        d.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RAW chroma compute"),
            source: wgpu::ShaderSource::Wgsl(include_str!("raw_compute.wgsl").into()),
        });
        let entries: Vec<_> = (0..4)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 3 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: binding == 0,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RAW compute layout"),
            entries: &entries,
        });
        let pipeline_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RAW compute pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let make = |entry| {
            d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let x = make("filter_x");
        let y = make("filter_y");
        if let Some(error) = pollster::block_on(d.pop_error_scope()) {
            anyhow::bail!("RAW compute pipeline: {error}");
        }
        Ok(Pipelines { layout, x, y })
    }
    pub fn reduce(
        &self,
        rgb: &[[f32; 3]],
        width: usize,
        amount: u32,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<[f32; 3]>> {
        self.reduce_with_tile_rows(rgb, width, amount, cancelled, 2048)
    }
    /// Public for native GPU acceptance of halo/stripe boundaries.
    pub fn reduce_with_tile_rows(
        &self,
        rgb: &[[f32; 3]],
        width: usize,
        amount: u32,
        cancelled: &(dyn Fn() -> bool + Sync),
        rows: usize,
    ) -> Result<Vec<[f32; 3]>> {
        ensure!(
            width > 0 && !rgb.is_empty() && rgb.len().is_multiple_of(width) && amount <= 100,
            "invalid RAW compute dimensions/amount"
        );
        ensure!(!cancelled(), "已取消 GPU RAW 处理");
        ensure!(
            crate::raw_parallel::all(rgb, |p| p.iter().all(|v| v.is_finite())),
            "nonfinite RAW compute input"
        );
        if amount == 0 {
            return Ok(rgb.to_vec());
        }
        let _gate = self.gate.lock().unwrap();
        let pipelines = self.pipelines()?;
        // Runtime validation and allocation errors must use the same CPU fallback
        // as shader/device errors, rather than the default uncaptured-error panic.
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.reduce_stripes(rgb, width, amount, cancelled, rows, pipelines);
        let validation = pollster::block_on(self.device.pop_error_scope());
        let allocation = pollster::block_on(self.device.pop_error_scope());
        ensure!(
            validation.is_none(),
            "RAW compute validation: {validation:?}"
        );
        ensure!(
            allocation.is_none(),
            "RAW compute allocation: {allocation:?}"
        );
        result
    }
    fn reduce_stripes(
        &self,
        rgb: &[[f32; 3]],
        width: usize,
        amount: u32,
        cancelled: &(dyn Fn() -> bool + Sync),
        rows: usize,
        pipelines: &Pipelines,
    ) -> Result<Vec<[f32; 3]>> {
        let height = rgb.len() / width;
        let scale = width.max(height).div_ceil(1600).clamp(1, 4);
        let radius = (if amount > 50 { 3 } else { 2 }) * scale;
        let limits = self.device.limits();
        let max_bytes = (limits.max_storage_buffer_binding_size as usize)
            .min(limits.max_buffer_size as usize)
            .min(64 * 1024 * 1024);
        let max_rows = max_bytes
            .checked_div(width.checked_mul(12).context("RAW compute row overflow")?)
            .context("invalid RAW row")?;
        ensure!(
            max_rows > 2 * radius,
            "RAW compute row exceeds device limit"
        );
        let stripe = rows.max(1).min(max_rows - 2 * radius);
        ensure!(
            width.div_ceil(16) <= limits.max_compute_workgroups_per_dimension as usize
                && (stripe + 2 * radius).div_ceil(8)
                    <= limits.max_compute_workgroups_per_dimension as usize,
            "RAW compute dispatch exceeds device limit"
        );
        let mut result: Vec<[f32; 3]> = vec![[0.; 3]; rgb.len()];
        for first in (0..height).step_by(stripe) {
            ensure!(!cancelled(), "已取消 GPU RAW 处理");
            let last = (first + stripe).min(height);
            let start = first.saturating_sub(radius);
            let end = (last + radius).min(height);
            let input = &rgb[start * width..end * width];
            let count = (last - first) * width;
            crate::raw_trace::add_h2d((input.len() * 12) as u64);
            crate::raw_trace::add_d2h((count * 12) as u64);
            let d = &self.device;
            let source = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RAW camera stripe"),
                contents: bytemuck::cast_slice(input),
                usage: wgpu::BufferUsages::STORAGE,
            });
            let allocate = |label, size, usage| {
                d.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage,
                    mapped_at_creation: false,
                })
            };
            let horizontal = allocate(
                "RAW chroma horizontal",
                (input.len() * 8) as u64,
                wgpu::BufferUsages::STORAGE,
            );
            let output = allocate(
                "RAW chroma output",
                (count * 12) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            let readback = allocate(
                "RAW chroma readback",
                (count * 12) as u64,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            );
            let params = [
                width as u32,
                (end - start) as u32,
                (last - first) as u32,
                (first - start) as u32,
                radius as u32,
                (scale as f32).to_bits(),
                ((amount as f32 / 25.).min(1.)).to_bits(),
                0,
            ];
            let uniform = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RAW compute parameters"),
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bindings = [&source, &horizontal, &output, &uniform];
            let entries: Vec<_> = bindings
                .iter()
                .enumerate()
                .map(|(binding, b)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: b.as_entire_binding(),
                })
                .collect();
            let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RAW compute stripe"),
                layout: &pipelines.layout,
                entries: &entries,
            });
            let mut encoder = d.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_bind_group(0, &group, &[]);
                pass.set_pipeline(&pipelines.x);
                pass.dispatch_workgroups(
                    (width as u32).div_ceil(16),
                    ((end - start) as u32).div_ceil(8),
                    1,
                );
            }
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_bind_group(0, &group, &[]);
                pass.set_pipeline(&pipelines.y);
                pass.dispatch_workgroups(
                    (width as u32).div_ceil(16),
                    ((last - first) as u32).div_ceil(8),
                    1,
                );
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, (count * 12) as u64);
            let submission = self.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            d.poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(10)),
            })
            .context("wait for RAW compute")?;
            rx.recv_timeout(Duration::from_secs(1))
                .context("RAW compute map callback")??;
            ensure!(!cancelled(), "已取消 GPU RAW 处理");
            {
                let mapped = readback.slice(..).get_mapped_range();
                result[first * width..last * width].copy_from_slice(bytemuck::cast_slice(&mapped));
            }
            readback.unmap();
        }
        ensure!(
            crate::raw_parallel::all(&result, |p| p.iter().all(|v| v.is_finite())),
            "invalid GPU RAW result"
        );
        Ok(result)
    }
}
#[path = "raw_ahd.rs"]
mod ahd;
pub use ahd::{Bayer, try_demosaic};
#[path = "raw_entropy_gpu.rs"]
mod entropy;
#[path = "raw_scene.rs"]
mod scene;
pub use entropy::{Adoption, Outcome, entropy_enabled, try_entropy};
pub use scene::{Scene, SceneParameters, try_scene};
pub fn try_reduce(
    rgb: &[[f32; 3]],
    width: usize,
    amount: u32,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<[f32; 3]>>> {
    if amount == 0
        // At fit-preview sizes the first pipeline/upload/readback cost can
        // exceed the CPU filter. Reserve compute for full-detail workloads.
        || rgb.len() < 4_000_000
        || std::env::var("FASTCULL_RAW_COMPUTE").is_ok_and(|v| v == "cpu")
    {
        return Ok(None);
    }
    let context = ACTIVE.lock().unwrap().clone();
    let Some(context) = context else {
        return Ok(None);
    };
    let started = std::time::Instant::now();
    match context.reduce(rgb, width, amount, cancelled) {
        Ok(result) => {
            log::info!(
                "RAW compute GPU chroma pixels={} width={width} ms={:.3}",
                rgb.len(),
                started.elapsed().as_secs_f64() * 1000.
            );
            Ok(Some(result))
        }
        Err(error) => {
            ensure!(!cancelled(), "已取消 GPU RAW 处理");
            log::warn!("GPU RAW compute unavailable; using CPU: {error:#}");
            Ok(None)
        }
    }
}
