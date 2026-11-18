//! GPU-resident camera RGB → chroma NR → geometry / ProPhoto → display.
//! A small histogram/status buffer is read back, never the full scene bitmap.
use super::*;
pub struct Scene {
    pub texture: wgpu::Texture,
    pub samples: Vec<[f32; 4]>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: [usize; 2],
}
pub struct SceneParameters {
    pub output: [usize; 2],
    pub uvs: [[f32; 2]; 4],
    pub matrix: [[f64; 3]; 3],
    pub correction: [f64; 3],
    pub limit: f32,
    pub direct: bool,
}
pub(super) struct ScenePipelines {
    nr_layout: wgpu::BindGroupLayout,
    x: wgpu::ComputePipeline,
    y: wgpu::ComputePipeline,
    geometry_layout: wgpu::BindGroupLayout,
    gather_layout: wgpu::BindGroupLayout,
    gather_params_layout: wgpu::BindGroupLayout,
    geometry: wgpu::ComputePipeline,
    gather: wgpu::ComputePipeline,
}
fn storage(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    )
}
fn uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    )
}
fn entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty,
        count: None,
    }
}
fn texture(binding: u32, write: bool) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        if write {
            wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba32Float,
                view_dimension: wgpu::TextureViewDimension::D2,
            }
        } else {
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            }
        },
    )
}
impl Context {
    fn create_scene_pipelines(&self) -> Result<ScenePipelines> {
        let d = &self.device;
        let nr_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resident NR"),
            entries: &[
                storage(0, true),
                storage(1, false),
                storage(2, false),
                uniform(3),
                texture(4, true),
            ],
        });
        let nr_shader = format!(
            "{}\n{}",
            include_str!("raw_compute.wgsl"),
            r#"
@group(0) @binding(4) var nr_scene:texture_storage_2d<rgba32float,write>;
@compute @workgroup_size(16,8)
fn store_y(@builtin(global_invocation_id) id:vec3<u32>) {
    if(id.x>=p.width || id.y>=p.output_height){return;}
    var rgb:vec3<f32>;
    if(p.strength==0.) {
        let i=3u*((id.y+p.top)*p.width+id.x);
        rgb=vec3<f32>(source[i],source[i+1u],source[i+2u]);
    } else { rgb=filtered(id.x,id.y); }
    textureStore(nr_scene,vec2<u32>(id.x,id.y+p.pad),vec4<f32>(rgb,1.));
}"#
        );
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resident NR"),
            source: wgpu::ShaderSource::Wgsl(nr_shader.into()),
        });
        let make = |shader: &wgpu::ShaderModule, layouts: &[&wgpu::BindGroupLayout], name| {
            let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(name),
                bind_group_layouts: layouts,
                push_constant_ranges: &[],
            });
            d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&layout),
                module: shader,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let x = make(&shader, &[&nr_layout], "filter_x");
        let y = make(&shader, &[&nr_layout], "store_y");
        let geometry_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resident geometry"),
            entries: &[
                texture(0, false),
                texture(1, true),
                uniform(2),
                storage(3, false),
                storage(4, false),
                storage(5, true),
            ],
        });
        let gather_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resident samples"),
            entries: &[texture(0, false)],
        });
        let gather_params_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resident sample parameters"),
            entries: &[uniform(2), storage(3, false)],
        });
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resident geometry"),
            source: wgpu::ShaderSource::Wgsl(include_str!("raw_scene.wgsl").into()),
        });
        let geometry = make(&shader, &[&geometry_layout], "geometry");
        let gather = make(&shader, &[&gather_params_layout, &gather_layout], "gather");
        Ok(ScenePipelines {
            nr_layout,
            x,
            y,
            geometry_layout,
            gather_layout,
            gather_params_layout,
            geometry,
            gather,
        })
    }
    pub fn scene(
        &self,
        rgb: &[[f32; 3]],
        width: usize,
        amount: u32,
        p: &SceneParameters,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Scene> {
        ensure!(
            width > 0 && rgb.len().is_multiple_of(width) && !rgb.is_empty() && amount <= 100,
            "invalid resident source"
        );
        ensure!(
            crate::raw_parallel::all(rgb, |v| v.iter().all(|v| v.is_finite())),
            "invalid resident pixels"
        );
        ensure!(
            p.matrix
                .iter()
                .flatten()
                .chain(p.correction.iter())
                .all(|v| v.is_finite() && (*v as f32).is_finite())
                && p.uvs.iter().flatten().all(|v| v.is_finite())
                && p.limit.is_finite()
                && p.limit > 0.,
            "invalid resident geometry"
        );
        ensure!(!cancelled(), "已取消 GPU 常驻处理");
        ensure!(
            !p.direct || p.output == [width, rgb.len() / width],
            "resident direct dimensions differ"
        );
        let _gate = self.gate.lock().unwrap();
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = (|| {
            let pipelines = self
                .scene
                .get_or_init(|| self.create_scene_pipelines().map_err(|e| format!("{e:#}")))
                .as_ref()
                .map_err(|e| anyhow::anyhow!(e.clone()))?;
            self.scene_inner(rgb, width, amount, p, cancelled, pipelines)
        })();
        let validation = pollster::block_on(self.device.pop_error_scope());
        let allocation = pollster::block_on(self.device.pop_error_scope());
        ensure!(
            validation.is_none() && allocation.is_none(),
            "resident scene: {validation:?} {allocation:?}"
        );
        result
    }
    fn scene_inner(
        &self,
        rgb: &[[f32; 3]],
        width: usize,
        amount: u32,
        p: &SceneParameters,
        cancelled: &(dyn Fn() -> bool + Sync),
        pipelines: &ScenePipelines,
    ) -> Result<Scene> {
        let height = rgb.len() / width;
        let d = &self.device;
        let q = &self.queue;
        let limits = d.limits();
        ensure!(
            [width, height, p.output[0], p.output[1]]
                .iter()
                .all(|v| *v > 0 && *v <= limits.max_texture_dimension_2d as usize),
            "resident texture exceeds GPU limits"
        );
        let create_texture = |label, w, h| {
            d.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w as u32,
                    height: h as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let camera = create_texture("resident NR camera", width, height);
        let camera_view = camera.create_view(&Default::default());
        let output = create_texture("resident ProPhoto", p.output[0], p.output[1]);
        let view = output.create_view(&Default::default());
        let allocate = |label, size, usage| {
            d.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let dummy = allocate("unused NR RGB", 4, wgpu::BufferUsages::STORAGE);
        let scale = width.max(height).div_ceil(1600).clamp(1, 4);
        let radius = if amount == 0 {
            0
        } else {
            (if amount > 50 { 3 } else { 2 }) * scale
        };
        let max_rows =
            (limits.max_storage_buffer_binding_size as usize).min(64 * 1024 * 1024) / (width * 12);
        ensure!(max_rows > 2 * radius, "resident stripe exceeds GPU limits");
        let rows = 2048.min(max_rows - 2 * radius);
        for first in (0..height).step_by(rows) {
            ensure!(!cancelled(), "已取消 GPU 常驻处理");
            let last = (first + rows).min(height);
            let start = first.saturating_sub(radius);
            let end = (last + radius).min(height);
            crate::raw_trace::add_h2d(((end - start) * width * 12) as u64);
            let source = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("resident camera upload"),
                contents: bytemuck::cast_slice(&rgb[start * width..end * width]),
                usage: wgpu::BufferUsages::STORAGE,
            });
            let horizontal = allocate(
                "resident chroma horizontal",
                ((end - start) * width * 8) as u64,
                wgpu::BufferUsages::STORAGE,
            );
            let params = [
                width as u32,
                (end - start) as u32,
                (last - first) as u32,
                (first - start) as u32,
                radius as u32,
                (scale as f32).to_bits(),
                ((amount as f32 / 25.).min(1.)).to_bits(),
                first as u32,
            ];
            let uniform = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("resident NR parameters"),
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bindings = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("resident NR stripe"),
                layout: &pipelines.nr_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: source.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: horizontal.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: dummy.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&camera_view),
                    },
                ],
            });
            let mut encoder = d.create_command_encoder(&Default::default());
            if amount > 0 {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_bind_group(0, &bindings, &[]);
                pass.set_pipeline(&pipelines.x);
                pass.dispatch_workgroups(
                    width.div_ceil(16) as u32,
                    (end - start).div_ceil(8) as u32,
                    1,
                );
            }
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_bind_group(0, &bindings, &[]);
                pass.set_pipeline(&pipelines.y);
                pass.dispatch_workgroups(
                    width.div_ceil(16) as u32,
                    (last - first).div_ceil(8) as u32,
                    1,
                );
            }
            // Bound in-flight staging/scratch allocations to one stripe.
            let submission = q.submit([encoder.finish()]);
            d.poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(10)),
            })?;
        }
        let n = p.output[0].min(128) * p.output[1].min(128);
        let sample_bytes = (n * 16) as u64;
        let samples = allocate(
            "resident histogram",
            sample_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let invalid = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("resident status"),
            contents: &[0u8; 4],
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let mut params = Vec::<u32>::from([
            width as u32,
            height as u32,
            p.output[0] as u32,
            p.output[1] as u32,
        ]);
        for v in [
            p.uvs[0][0],
            p.uvs[0][1],
            p.uvs[1][0],
            p.uvs[1][1],
            p.uvs[2][0],
            p.uvs[2][1],
            p.uvs[3][0],
            p.uvs[3][1],
        ] {
            params.push(v.to_bits());
        }
        for row in p.matrix {
            params.extend(row.map(|v| (v as f32).to_bits()));
            params.push(0);
        }
        params.extend(p.correction.map(|v| (v as f32).to_bits()));
        params.push(0);
        params.extend([
            p.limit.to_bits(),
            (u32::from(p.direct) as f32).to_bits(),
            0,
            0,
        ]);
        let uniform = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("resident scene parameters"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // Tiny, exact CPU divisions, shared by rows/columns. GPU rcp division
        // can shift a UV by a source-pixel ULP on large rotated photographs.
        let coordinates: Vec<f32> = (0..p.output[0])
            .map(|x| (x as f32 + 0.5) / p.output[0] as f32)
            .chain((0..p.output[1]).map(|y| (y as f32 + 0.5) / p.output[1] as f32))
            .collect();
        let coordinates = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("resident exact coordinates"),
            contents: bytemuck::cast_slice(&coordinates),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bindings = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("resident geometry"),
            layout: &pipelines.geometry_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&camera_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: samples.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: invalid.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: coordinates.as_entire_binding(),
                },
            ],
        });
        let gather = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("resident sample source"),
            layout: &pipelines.gather_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        let gather_params = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("resident sample parameters"),
            layout: &pipelines.gather_params_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: samples.as_entire_binding(),
                },
            ],
        });
        crate::raw_trace::add_d2h(sample_bytes + 4);
        let readback = allocate(
            "resident small readback",
            sample_bytes + 4,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let mut encoder = d.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipelines.geometry);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(
                p.output[0].div_ceil(16) as u32,
                p.output[1].div_ceil(8) as u32,
                1,
            );
        }
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipelines.gather);
            pass.set_bind_group(0, &gather_params, &[]);
            pass.set_bind_group(1, &gather, &[]);
            pass.dispatch_workgroups(
                p.output[0].min(128).div_ceil(16) as u32,
                p.output[1].min(128).div_ceil(8) as u32,
                1,
            );
        }
        encoder.copy_buffer_to_buffer(&samples, 0, &readback, 0, sample_bytes);
        encoder.copy_buffer_to_buffer(&invalid, 0, &readback, sample_bytes, 4);
        let submission = q.submit([encoder.finish()]);
        let bytes = map_read(d, &readback, submission)?;
        ensure!(
            u32::from_ne_bytes(bytes[sample_bytes as usize..].try_into()?) == 0,
            "显影结果超出浮点显示范围"
        );
        ensure!(!cancelled(), "已取消 GPU 常驻处理");
        Ok(Scene {
            texture: output,
            samples: bytemuck::cast_slice(&bytes[..sample_bytes as usize]).to_vec(),
            device: d.clone(),
            queue: q.clone(),
            size: p.output,
        })
    }
}
fn map_read(
    d: &wgpu::Device,
    buffer: &wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
) -> Result<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    d.poll(wgpu::PollType::Wait {
        submission_index: Some(submission),
        timeout: Some(Duration::from_secs(10)),
    })?;
    rx.recv_timeout(Duration::from_secs(1))??;
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    Ok(bytes)
}
impl Scene {
    /// Explicit CPU export/reference path; interactive display never calls it.
    pub fn read_pixels(&self) -> Result<Vec<[f32; 4]>> {
        let row = (self.size[0] * 16).div_ceil(256) * 256;
        let rows =
            ((self.device.limits().max_buffer_size as usize).min(64 * 1024 * 1024) / row).max(1);
        let mut result = Vec::with_capacity(self.size[0] * self.size[1]);
        for first in (0..self.size[1]).step_by(rows) {
            let height = rows.min(self.size[1] - first);
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("explicit scene export stripe"),
                size: (row * height) as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = self.device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: first as u32,
                        z: 0,
                    },
                    ..self.texture.as_image_copy()
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row as u32),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: self.size[0] as u32,
                    height: height as u32,
                    depth_or_array_layers: 1,
                },
            );
            let bytes = map_read(
                &self.device,
                &readback,
                self.queue.submit([encoder.finish()]),
            )?;
            for row in bytes.chunks_exact(row) {
                result.extend_from_slice(bytemuck::cast_slice(&row[..self.size[0] * 16]));
            }
        }
        Ok(result)
    }
}
pub fn try_scene(
    rgb: &[[f32; 3]],
    width: usize,
    amount: u32,
    p: &SceneParameters,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Scene>> {
    if rgb.len() < 4_000_000
        || std::env::var("FASTCULL_RAW_RESIDENT").is_ok_and(|v| v == "0")
        || std::env::var("FASTCULL_RAW_COMPUTE").is_ok_and(|v| v == "cpu")
    {
        return Ok(None);
    }
    let context = ACTIVE.lock().unwrap().clone();
    let Some(context) = context else {
        return Ok(None);
    };
    let started = std::time::Instant::now();
    match context.scene(rgb, width, amount, p, cancelled) {
        Ok(scene) => {
            log::info!(
                "RAW resident scene_ms={:.3} pixels={} full_readback_bytes=0",
                started.elapsed().as_secs_f64() * 1000.,
                p.output[0] * p.output[1]
            );
            Ok(Some(scene))
        }
        Err(error) => {
            ensure!(!cancelled(), "已取消 GPU 常驻处理");
            log::warn!("RAW resident fallback: {error:#}");
            Ok(None)
        }
    }
}
