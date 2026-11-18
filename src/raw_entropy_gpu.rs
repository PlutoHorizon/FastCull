//! Bounded GPU tile batches. No input/output commits until all tiles succeed.
use super::*;
use crate::raw_entropy::{self, Image};
pub(super) struct Pipeline {
    layout: wgpu::BindGroupLayout,
    compute: wgpu::ComputePipeline,
}
impl Context {
    fn entropy_pipeline(&self) -> Result<&Pipeline> {
        self.entropy
            .get_or_init(|| self.create_entropy().map_err(|e| format!("{e:#}")))
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))
    }
    fn create_entropy(&self) -> Result<Pipeline> {
        let d = &self.device;
        d.push_error_scope(wgpu::ErrorFilter::Validation);
        d.push_error_scope(wgpu::ErrorFilter::Internal);
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tiled RAW entropy"),
            source: wgpu::ShaderSource::Wgsl(include_str!("raw_entropy.wgsl").into()),
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
                            read_only: binding != 3,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RAW entropy layout"),
            entries: &entries,
        });
        let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RAW entropy pipeline"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let compute = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("RAW Huffman + prediction"),
            layout: Some(&pl),
            module: &shader,
            entry_point: Some("entropy"),
            compilation_options: Default::default(),
            cache: None,
        });
        let internal = pollster::block_on(d.pop_error_scope());
        let validation = pollster::block_on(d.pop_error_scope());
        ensure!(
            internal.is_none() && validation.is_none(),
            "entropy shader: {internal:?} {validation:?}"
        );
        Ok(Pipeline { layout, compute })
    }
    pub fn decode_entropy(
        &self,
        image: &Image,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(Vec<u16>, (u64, u64))> {
        raw_entropy::validate(image)?;
        ensure!(!cancelled(), "已取消 GPU 熵解码");
        let _gate = self.gate.lock().unwrap();
        let compile_started = std::time::Instant::now();
        let p = self.entropy_pipeline()?;
        let compile_ms = compile_started.elapsed().as_secs_f64() * 1000.;
        log::info!("RAW entropy pipeline_ms={compile_ms:.3}");
        let d = &self.device;
        d.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        d.push_error_scope(wgpu::ErrorFilter::Validation);
        d.push_error_scope(wgpu::ErrorFilter::Internal);
        let result = self.entropy_batches(image, cancelled, p);
        let internal = pollster::block_on(d.pop_error_scope());
        let validation = pollster::block_on(d.pop_error_scope());
        let allocation = pollster::block_on(d.pop_error_scope());
        ensure!(
            internal.is_none() && validation.is_none() && allocation.is_none(),
            "entropy device: {internal:?} {validation:?} {allocation:?}"
        );
        result
    }
    fn entropy_batches(
        &self,
        image: &Image,
        cancelled: &(dyn Fn() -> bool + Sync),
        p: &Pipeline,
    ) -> Result<(Vec<u16>, (u64, u64))> {
        let max = (self.device.limits().max_storage_buffer_binding_size as usize)
            .min(self.device.limits().max_buffer_size as usize)
            .min(64 * 1024 * 1024);
        ensure!(
            !image.tiles.is_empty()
                && image
                    .width
                    .checked_mul(image.height)
                    .and_then(|n| n.checked_mul(image.channels))
                    .is_some_and(|n| n <= 160_000_000),
            "entropy output exceeds limit"
        );
        let mut result = vec![0u16; image.width * image.height * image.channels];
        let mut first = 0;
        let (mut setup_ms, mut wait_ms, mut assemble_ms) = (0., 0., 0.);
        let (mut h2d_bytes, mut d2h_bytes) = (0u64, 0u64);
        while first < image.tiles.len() {
            let setup_started = std::time::Instant::now();
            ensure!(!cancelled(), "已取消 GPU 熵解码");
            let mut last = first;
            let mut samples = 0;
            let mut input_bytes = 0;
            let mut table_bytes = 0;
            while let Some(t) = image.tiles.get(last) {
                let add = t.width * t.height * t.channels;
                // Conservative table accounting; deduplication reduces actual upload.
                if last > first
                    && ((samples + add + last - first + 1) * 4 > max
                        || input_bytes + t.data.len() + 12 > max
                        || table_bytes + t.channels * 4368 * 4 > max)
                {
                    break;
                }
                samples += add;
                input_bytes += t.data.len() + 12;
                table_bytes += t.channels * 4368 * 4;
                last += 1;
            }
            ensure!(
                last > first
                    && (samples + last - first) * 4 <= max
                    && input_bytes <= max
                    && table_bytes <= max,
                "entropy tile exceeds device buffer limit"
            );
            let mut encoded = Vec::<u8>::new();
            let mut tables = Vec::<u32>::new();
            let mut desc = Vec::<u32>::new();
            let mut seen = std::collections::HashMap::new();
            let mut offset = 0;
            for tile in &image.tiles[first..last] {
                let base = encoded.len() / 4;
                encoded.extend_from_slice(&tile.data);
                while !encoded.len().is_multiple_of(4) {
                    encoded.push(0);
                }
                encoded.extend_from_slice(&[0; 8]);
                let mut h = [0u32; 4];
                for (c, table) in tile.tables.iter().enumerate() {
                    let key = Arc::as_ptr(table) as *const u32 as usize;
                    h[c] = *seen.entry(key).or_insert_with(|| {
                        let at = tables.len() as u32;
                        tables.extend_from_slice(table);
                        at
                    });
                }
                desc.extend([
                    base as u32,
                    tile.data.len() as u32,
                    offset as u32,
                    tile.width as u32,
                    tile.height as u32,
                    tile.channels as u32,
                    tile.bits,
                    0,
                    h[0],
                    h[1],
                    h[2],
                    h[3],
                ]);
                offset += tile.width * tile.height * tile.channels;
            }
            let d = &self.device;
            let upload = |label, bytes: &[u8], usage| {
                d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytes,
                    usage,
                })
            };
            let input = upload("RAW entropy bytes", &encoded, wgpu::BufferUsages::STORAGE);
            let huff = upload(
                "RAW DC prefix tables",
                bytemuck::cast_slice(&tables),
                wgpu::BufferUsages::STORAGE,
            );
            let descriptors = upload(
                "RAW tile descriptors",
                bytemuck::cast_slice(&desc),
                wgpu::BufferUsages::STORAGE,
            );
            let count = last - first;
            let bytes = ((samples + count) * 4) as u64;
            let batch_h2d = encoded.len() as u64
                + (tables.len() * 4) as u64
                + (desc.len() * 4) as u64
                + 16
                + (count * 4) as u64;
            crate::raw_trace::add_h2d(batch_h2d);
            crate::raw_trace::add_d2h(bytes);
            h2d_bytes += batch_h2d;
            d2h_bytes += bytes;
            let output = d.create_buffer(&wgpu::BufferDescriptor {
                label: Some("RAW entropy result"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(
                &output,
                (samples * 4) as u64,
                bytemuck::cast_slice(&vec![u32::MAX; count]),
            );
            let readback = d.create_buffer(&wgpu::BufferDescriptor {
                label: Some("RAW entropy readback"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let uniform = upload(
                "RAW entropy counts",
                bytemuck::cast_slice(&[count as u32, samples as u32, 0, 0]),
                wgpu::BufferUsages::UNIFORM,
            );
            let entries: Vec<_> = [&input, &huff, &descriptors, &output, &uniform]
                .iter()
                .enumerate()
                .map(|(binding, v)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: v.as_entire_binding(),
                })
                .collect();
            let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RAW entropy batch"),
                layout: &p.layout,
                entries: &entries,
            });
            let mut encoder = d.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_pipeline(&p.compute);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(count as u32, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
            setup_ms += setup_started.elapsed().as_secs_f64() * 1000.;
            let wait_started = std::time::Instant::now();
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
            ensure!(!cancelled(), "已取消 GPU 熵解码");
            wait_ms += wait_started.elapsed().as_secs_f64() * 1000.;
            let assemble_started = std::time::Instant::now();
            {
                let mapped = readback.slice(..).get_mapped_range();
                let values: &[u32] = bytemuck::cast_slice(&mapped);
                ensure!(
                    values[samples..].iter().all(|v| *v == 0),
                    "GPU entropy rejected tile: {:?}",
                    &values[samples..]
                );
                let mut at = 0;
                for tile in &image.tiles[first..last] {
                    let n = tile.width * tile.height * tile.channels;
                    raw_entropy::assemble(image, tile, &values[at..at + n], &mut result)?;
                    at += n;
                }
            }
            readback.unmap();
            assemble_ms += assemble_started.elapsed().as_secs_f64() * 1000.;
            first = last;
        }
        log::info!(
            "RAW entropy setup_ms={setup_ms:.3} wait_ms={wait_ms:.3} assemble_ms={assemble_ms:.3} \
             h2d_bytes={h2d_bytes} d2h_bytes={d2h_bytes}"
        );
        Ok((result, (h2d_bytes, d2h_bytes)))
    }
}
pub fn entropy_enabled() -> bool {
    ACTIVE.lock().unwrap().is_some()
        && !std::env::var("FASTCULL_RAW_COMPUTE").is_ok_and(|v| v == "cpu")
        && !std::env::var("FASTCULL_RAW_ENTROPY").is_ok_and(|v| v == "cpu")
}
/// Which backend actually produced the samples, and what it cost.
pub struct Adoption {
    pub backend: &'static str,
    pub reason: String,
    pub tiles: usize,
    pub samples: usize,
    pub h2d_bytes: u64,
    pub d2h_bytes: u64,
    pub ms: f64,
}
pub enum Outcome {
    Adopted(Vec<u16>, Adoption),
    Unavailable(String),
}
pub fn try_entropy(image: &Image, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<Outcome> {
    if !entropy_enabled() {
        return Ok(Outcome::Unavailable(
            "entropy disabled (FASTCULL_RAW_ENTROPY=cpu or no GPU context)".into(),
        ));
    }
    let Some(context) = ACTIVE.lock().unwrap().clone() else {
        return Ok(Outcome::Unavailable("no GPU context installed".into()));
    };
    let start = std::time::Instant::now();
    // On an 8-worker host, independent prefix-table streams measured faster
    // than GPU dispatch + transfer, even on the tested discrete adapter.
    // Keep an explicit GPU override for device comparisons and lower-core CPUs.
    let mode = std::env::var("FASTCULL_RAW_ENTROPY").unwrap_or_default();
    if mode == "parallel"
        || (mode != "gpu" && std::thread::available_parallelism().map_or(1, usize::from) >= 8)
    {
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        match raw_entropy::decode_cpu(image, cancelled) {
            Ok(out) => {
                let ms = start.elapsed().as_secs_f64() * 1000.;
                let adoption = Adoption {
                    backend: "parallel-cpu",
                    reason: if mode == "parallel" {
                        "FASTCULL_RAW_ENTROPY=parallel".into()
                    } else {
                        format!("{cores} logical processors (>=8) prefer parallel CPU")
                    },
                    tiles: image.tiles.len(),
                    samples: out.len(),
                    h2d_bytes: 0,
                    d2h_bytes: 0,
                    ms,
                };
                log::info!(
                    "RAW parallel tile entropy {:?} {}x{} tiles={} ms={:.3}",
                    image.kind,
                    image.width,
                    image.height,
                    image.tiles.len(),
                    ms
                );
                return Ok(Outcome::Adopted(out, adoption));
            }
            Err(error) => {
                ensure!(!cancelled(), "已取消并行熵解码");
                log::warn!("Parallel tile entropy unavailable; using LibRaw: {error:#}");
                return Ok(Outcome::Unavailable(format!(
                    "parallel CPU failed: {error:#}"
                )));
            }
        }
    }
    match context.decode_entropy(image, cancelled) {
        Ok((out, bytes)) => {
            let ms = start.elapsed().as_secs_f64() * 1000.;
            log::info!(
                "RAW GPU entropy {:?} {}x{} tiles={} ms={:.3}",
                image.kind,
                image.width,
                image.height,
                image.tiles.len(),
                ms
            );
            Ok(Outcome::Adopted(
                out,
                Adoption {
                    backend: "gpu",
                    reason: "FASTCULL_RAW_ENTROPY=gpu or fewer than 8 processors".into(),
                    tiles: image.tiles.len(),
                    samples: image.width * image.height * image.channels,
                    h2d_bytes: bytes.0,
                    d2h_bytes: bytes.1,
                    ms,
                },
            ))
        }
        Err(error) => {
            ensure!(!cancelled(), "已取消 GPU 熵解码");
            log::warn!("GPU entropy unavailable; using LibRaw: {error:#}");
            Ok(Outcome::Unavailable(format!(
                "GPU decode failed: {error:#}"
            )))
        }
    }
}
