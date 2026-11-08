use super::*;
use crate::raw_preview::{HsvLookup, evaluate_scene};

#[test]
fn invalid_float_frames_are_rejected_before_gpu_upload() {
    let limits = wgpu::Limits::default();
    let pixels = [[0.18, 0.18, 0.18, 1.]];
    let mut recipe = Recipe::default();
    assert!(validate_image([1, 1], &pixels, &recipe, &limits).is_ok());
    assert!(validate_image([2, 1], &pixels, &recipe, &limits).is_err());
    assert!(validate_image([1, 1], &[[f32::NAN; 4]], &recipe, &limits).is_err());
    recipe.curve = vec![0.];
    assert!(validate_image([1, 1], &pixels, &recipe, &limits).is_err());
    recipe = Recipe::default();
    recipe.look = Some(HsvLookup {
        dims: [2, 2, 2],
        data: vec![[0., 1., 1.]],
        encoding: 0,
    });
    assert!(validate_image([1, 1], &pixels, &recipe, &limits).is_err());
    recipe.look = None;
    recipe.baseline_exposure = f32::INFINITY;
    assert!(validate_image([1, 1], &pixels, &recipe, &limits).is_err());
}

fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
    image: &GpuImage,
    params: Parameters,
    scene: bool,
) -> Vec<u8> {
    let extent = wgpu::Extent3d {
        width: 64,
        height: 1,
        depth_or_array_layers: 1,
    };
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("DirectX regression output"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if pipelines.srgb_target {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        },
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let draw = Draw::new(device, pipelines, image, params);
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let view = output.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(if scene {
            &pipelines.scene
        } else {
            &pipelines.jpeg
        });
        pass.set_bind_group(0, &draw.bindings, &[]);
        pass.draw(0..6, 0..1);
    }
    encoder.copy_texture_to_buffer(
        output.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    let (send, recv) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| send.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    recv.recv().unwrap().unwrap();
    let result = readback.slice(..).get_mapped_range().to_vec();
    readback.unmap();
    result
}

#[test]
#[ignore = "requires a real native GPU adapter; run explicitly for graphics acceptance"]
fn native_gpu_pixels_match_cpu_scene_and_jpeg_reference() {
    let backends = if cfg!(target_os = "macos") {
        wgpu::Backends::METAL
    } else {
        wgpu::Backends::DX12
    };
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });
    let adapter = instance
        .enumerate_adapters(backends)
        .into_iter()
        .min_by_key(|a| u8::from(a.get_info().vendor != 0x10de))
        .expect("native GPU adapter");
    println!("GPU acceptance: {:?}", adapter.get_info());
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let pipelines = Pipelines::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let error = pollster::block_on(device.pop_error_scope());
    assert!(error.is_none(), "shader/pipeline validation: {error:?}");
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64., 1.));
    let uvs = [
        egui::pos2(0., 0.),
        egui::pos2(1., 0.),
        egui::pos2(1., 1.),
        egui::pos2(0., 1.),
    ];
    let samples: Vec<[f32; 4]> = (0..64)
        .map(|i| {
            if i == 0 {
                [0., 0., 0., 1.]
            } else if i == 1 {
                [0.18, 0.18, 0.18, 1.]
            } else if i == 2 {
                [4., 2., 1.1, 1.]
            } else {
                [
                    (i % 13) as f32 / 12.,
                    (i % 17) as f32 / 16.,
                    (i % 23) as f32 / 22.,
                    1.,
                ]
            }
        })
        .collect();
    let mut cases = 0;
    for encoding in [0, 1, 2, 3] {
        let mut recipe = Recipe::default();
        if encoding < 2 {
            recipe.baseline_exposure = 0.3;
            recipe.curve = vec![0., 0.14, 0.45, 0.8, 1.];
            recipe.look = Some(HsvLookup {
                dims: [3, 2, 2],
                encoding,
                data: (0..12)
                    .map(|i| {
                        [
                            (i % 3) as f32 * 2. - 2.,
                            0.85 + i as f32 * 0.01,
                            0.95 + i as f32 * 0.01,
                        ]
                    })
                    .collect(),
            });
        }
        if encoding == 3 {
            recipe.look = Some(HsvLookup {
                dims: [1, 2, 1],
                data: vec![[0., 1., 1.], [8., 0.8, 1.1]],
                encoding: 0,
            });
            recipe.extra_look = Some(HsvLookup {
                dims: [1, 2, 2],
                data: vec![
                    [0., 1., 1.],
                    [-15., 1.2, 0.9],
                    [0., 1., 1.],
                    [20., 0.85, 1.15],
                ],
                encoding: 1,
            });
        }
        let image = upload_image(&device, &queue, [64, 1], &samples, &recipe).unwrap();
        let mut settings = vec![
            Settings::default(),
            Settings {
                exposure: -2.,
                ..Default::default()
            },
            Settings {
                exposure: 1.,
                wb: [1.1, 0.95, 0.8],
                ..Default::default()
            },
        ];
        for control in 0..7 {
            for value in [-1., 0.8] {
                let mut s = Settings::default();
                s.basic[control] = value;
                settings.push(s);
            }
        }
        settings.push(Settings {
            basic: [0.2, -0.35, 0.25, 0.15, -0.1, 1., -1.],
            ..Default::default()
        });
        for s in settings {
            let params = parameters(rect, rect, uvs, s, &recipe, true, false);
            let result = render(&device, &queue, &pipelines, &image, params, true);
            for (i, pixel) in result.chunks_exact(4).enumerate() {
                let expected =
                    evaluate_scene([samples[i][0], samples[i][1], samples[i][2]], s, &recipe);
                for c in 0..3 {
                    assert!(
                        (pixel[c] as f32 - expected[c] * 255.).abs() <= 1.1,
                        "scene case={cases} sample={i} channel={c} actual={} expected={} settings={s:?}",
                        pixel[c],
                        expected[c] * 255.
                    );
                }
            }
            cases += 1;
        }
    }
    let jpeg: Vec<[f32; 4]> = (0..64)
        .map(|i| [i as f32 / 63., (63 - i) as f32 / 63., 0.18, 1.])
        .collect();
    let recipe = Recipe::default();
    let image = upload_image(&device, &queue, [64, 1], &jpeg, &recipe).unwrap();
    for exposure in [-2., -1. / 3., 0., 1. / 3., 1., 2.] {
        let s = Settings {
            exposure,
            ..Default::default()
        };
        let result = render(
            &device,
            &queue,
            &pipelines,
            &image,
            parameters(rect, rect, uvs, s, &recipe, false, false),
            false,
        );
        for (i, pixel) in result.chunks_exact(4).enumerate() {
            for (c, value) in pixel.iter().take(3).enumerate() {
                let expected = crate::raw_preview::encode(
                    (crate::raw_preview::decode(jpeg[i][c]) * 2f32.powf(exposure)).clamp(0., 1.),
                );
                assert!(
                    (*value as f32 - expected * 255.).abs() <= 1.1,
                    "JPEG exposure={exposure} sample={i}"
                );
            }
        }
        cases += 1;
    }
    // Reversed UVs use the same binding/uniform path as rotated/cropped canvases.
    let reverse = [uvs[1], uvs[0], uvs[3], uvs[2]];
    let result = render(
        &device,
        &queue,
        &pipelines,
        &image,
        parameters(
            rect,
            rect,
            reverse,
            Settings::default(),
            &recipe,
            false,
            false,
        ),
        false,
    );
    for (i, pixel) in result.chunks_exact(4).enumerate() {
        for (c, value) in pixel.iter().take(3).enumerate() {
            assert!(
                (*value as f32 - jpeg[63 - i][c] * 255.).abs() <= 1.1,
                "mirrored UVs sample={i}"
            );
        }
    }
    let error = pollster::block_on(device.pop_error_scope());
    assert!(error.is_none(), "{error:?}");
    // Metal can expose an sRGB render target. The shader must compensate for
    // hardware encoding so the result stays identical to an Unorm drawable.
    let srgb = Pipelines::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
    let result = render(
        &device,
        &queue,
        &srgb,
        &image,
        parameters(rect, rect, uvs, Settings::default(), &recipe, false, true),
        false,
    );
    for (i, pixel) in result.chunks_exact(4).enumerate() {
        for (c, value) in pixel.iter().take(3).enumerate() {
            assert!(
                (*value as f32 - jpeg[i][c] * 255.).abs() <= 1.1,
                "sRGB target double-encoding sample={i}"
            );
        }
    }
    println!(
        "Native GPU acceptance passed: {cases} color cases * 64 pixels plus mirrored UVs; <=1.1 code-value error"
    );
}
