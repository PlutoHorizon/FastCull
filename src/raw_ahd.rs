//! Bounded GPU AHD execution; LibRaw supplies black/WB-corrected Bayer pixels.
use super::*;
pub struct Bayer<'a> {
    pub image: &'a [[u16; 4]],
    pub width: usize,
    pub height: usize,
    pub filters: u32,
    pub xyz: [[f32; 4]; 3],
    pub cube_root: &'a [f32],
}
pub(super) struct Pipelines {
    layout: wgpu::BindGroupLayout,
    stages: [wgpu::ComputePipeline; 4],
}
impl Context {
    pub(super) fn prewarm_demosaic(&self) -> Result<()> {
        let _gate = self.gate.lock().unwrap();
        let started = std::time::Instant::now();
        let _ = self.ahd_pipelines()?;
        log::info!(
            "RAW AHD prewarm_ms={:.3}",
            started.elapsed().as_secs_f64() * 1000.
        );
        Ok(())
    }
    fn ahd_pipelines(&self) -> Result<&Pipelines> {
        self.ahd
            .get_or_init(|| self.create_ahd().map_err(|e| format!("{e:#}")))
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))
    }
    fn create_ahd(&self) -> Result<Pipelines> {
        let d = &self.device;
        d.push_error_scope(wgpu::ErrorFilter::Validation);
        d.push_error_scope(wgpu::ErrorFilter::Internal);
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RAW GPU AHD"),
            source: wgpu::ShaderSource::Wgsl(include_str!("raw_ahd.wgsl").into()),
        });
        let entries: Vec<_> = (0..5)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 4 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: binding == 0 || binding == 3,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RAW AHD layout"),
            entries: &entries,
        });
        let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RAW AHD pipelines"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let stages = ["ahd_green", "ahd_rgb_lab", "ahd_homogeneity", "ahd_combine"].map(|entry| {
            d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let internal = pollster::block_on(d.pop_error_scope());
        let validation = pollster::block_on(d.pop_error_scope());
        if let Some(e) = internal.or(validation) {
            anyhow::bail!("GPU AHD shader: {e}");
        }
        Ok(Pipelines { layout, stages })
    }
    pub fn demosaic(
        &self,
        b: &Bayer<'_>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<[u16; 4]>> {
        ensure!(
            b.width >= 16
                && b.height >= 16
                && b.width.checked_mul(b.height) == Some(b.image.len())
                && b.image.len() <= 160_000_000,
            "invalid Bayer dimensions"
        );
        ensure!(
            b.cube_root.len() == 65536
                && b.xyz
                    .iter()
                    .flatten()
                    .chain(b.cube_root)
                    .all(|v| v.is_finite()),
            "invalid AHD colour data"
        );
        ensure!(!cancelled(), "已取消 GPU 去马赛克");
        let _gate = self.gate.lock().unwrap();
        let p = self.ahd_pipelines()?;
        let d = &self.device;
        d.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        d.push_error_scope(wgpu::ErrorFilter::Validation);
        d.push_error_scope(wgpu::ErrorFilter::Internal);
        let result = self.ahd_stripes(b, cancelled, p);
        let internal = pollster::block_on(d.pop_error_scope());
        let validation = pollster::block_on(d.pop_error_scope());
        let allocation = pollster::block_on(d.pop_error_scope());
        ensure!(
            internal.is_none() && validation.is_none() && allocation.is_none(),
            "GPU AHD device: {internal:?} {validation:?} {allocation:?}"
        );
        result
    }
    fn ahd_stripes(
        &self,
        b: &Bayer<'_>,
        cancelled: &(dyn Fn() -> bool + Sync),
        p: &Pipelines,
    ) -> Result<Vec<[u16; 4]>> {
        let d = &self.device;
        let limits = d.limits();
        let max = (limits.max_storage_buffer_binding_size as usize)
            .min(limits.max_buffer_size as usize)
            .min(128 * 1024 * 1024);
        let rows = max / (b.width * 64);
        ensure!(rows > 10, "Bayer row exceeds GPU buffer limit");
        let stripe = (rows - 10).min(384);
        ensure!(
            b.width.div_ceil(16) <= limits.max_compute_workgroups_per_dimension as usize,
            "Bayer dispatch exceeds GPU limits"
        );
        let table = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("AHD cube root table"),
            contents: bytemuck::cast_slice(b.cube_root),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut result = vec![[0u16; 4]; b.image.len()];
        for first in (0..b.height).step_by(stripe) {
            ensure!(!cancelled(), "已取消 GPU 去马赛克");
            let last = (first + stripe).min(b.height);
            let start = first.saturating_sub(5);
            let end = (last + 5).min(b.height);
            let input = &b.image[start * b.width..end * b.width];
            let count = (last - first) * b.width;
            crate::raw_trace::add_h2d((input.len() * 2) as u64);
            crate::raw_trace::add_d2h((count * 8) as u64);
            let source = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("AHD Bayer stripe"),
                contents: bytemuck::cast_slice(input),
                usage: wgpu::BufferUsages::STORAGE,
            });
            let alloc = |label, size, usage| {
                d.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage,
                    mapped_at_creation: false,
                })
            };
            let scratch = alloc(
                "AHD scratch",
                (input.len() * 64) as u64,
                wgpu::BufferUsages::STORAGE,
            );
            let output = alloc(
                "AHD output",
                (count * 8) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            let readback = alloc(
                "AHD readback",
                (count * 8) as u64,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            );
            let mut params = vec![
                b.width as u32,
                b.height as u32,
                (end - start) as u32,
                start as u32,
                (first - start) as u32,
                (last - first) as u32,
                b.filters,
                0,
            ];
            params.extend(b.xyz.iter().flatten().map(|v| v.to_bits()));
            let uniform = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("AHD parameters"),
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let entries: Vec<_> = [&source, &scratch, &output, &table, &uniform]
                .iter()
                .enumerate()
                .map(|(binding, v)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: v.as_entire_binding(),
                })
                .collect();
            let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("AHD stripe"),
                layout: &p.layout,
                entries: &entries,
            });
            let mut encoder = d.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for (i, pipeline) in p.stages.iter().enumerate() {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &group, &[]);
                let h = if i == 3 { last - first } else { end - start };
                pass.dispatch_workgroups((b.width as u32).div_ceil(16), (h as u32).div_ceil(8), 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, (count * 8) as u64);
            let submission = self.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            d.poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(10)),
            })?;
            rx.recv_timeout(Duration::from_secs(1))??;
            ensure!(!cancelled(), "已取消 GPU 去马赛克");
            {
                let mapped = readback.slice(..).get_mapped_range();
                result[first * b.width..last * b.width]
                    .copy_from_slice(bytemuck::cast_slice(&mapped));
            }
            readback.unmap();
        }
        Ok(result)
    }
}
pub fn try_demosaic(
    b: Bayer<'_>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<[u16; 4]>>> {
    if std::env::var("FASTCULL_RAW_COMPUTE").is_ok_and(|v| v == "cpu")
        || std::env::var("FASTCULL_RAW_DEMOSAIC").is_ok_and(|v| v == "cpu")
    {
        return Ok(None);
    }
    let Some(context) = ACTIVE.lock().unwrap().clone() else {
        return Ok(None);
    };
    let start = std::time::Instant::now();
    match context.demosaic(&b, cancelled) {
        Ok(result) => {
            log::info!(
                "RAW GPU AHD {}x{} ms={:.3}",
                b.width,
                b.height,
                start.elapsed().as_secs_f64() * 1000.
            );
            Ok(Some(result))
        }
        Err(error) => {
            ensure!(!cancelled(), "已取消 GPU 去马赛克");
            log::warn!("GPU AHD unavailable; using CPU: {error:#}");
            Ok(None)
        }
    }
}
