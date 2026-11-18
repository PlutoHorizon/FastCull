//! Real-device resident NR / geometry regression, including stripe boundaries.
#[cfg(any(windows, target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    use anyhow::ensure;
    use fastcull::{
        raw_compute::{Context, SceneParameters},
        white_balance,
    };
    let backends = if cfg!(windows) {
        wgpu::Backends::DX12
    } else {
        wgpu::Backends::METAL
    };
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });
    let adapter = instance
        .enumerate_adapters(backends)
        .into_iter()
        .min_by_key(|a| u8::from(a.get_info().vendor != 0x10de))
        .unwrap();
    println!("GPU {:?}", adapter.get_info());
    let (d, q) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    let context = Context::new(d, q);
    let matrix = [
        [1.21, -0.13, -0.08],
        [-0.03, 1.07, -0.04],
        [0.03, -0.17, 1.14],
    ];
    let correction = [0.79, 1.0, 1.27];
    let mut cases = 0;
    for (w, h) in [(23, 17), (1601, 39), (6401, 21), (1, 6401), (128, 2049)] {
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = ((i * 17 + i / w * 29) % 577) as f32 / 256. - 0.03;
                [v, v * 0.71, v * 1.32]
            })
            .collect();
        for noise in [0, 25, 75, 100] {
            let reference = context.reduce(&rgb, w, noise, &|| false)?;
            for (output, uvs, direct) in [
                ([w, h], [[0., 0.], [1., 0.], [1., 1.], [0., 1.]], true),
                ([h, w], [[0., 1.], [0., 0.], [1., 0.], [1., 1.]], false),
                (
                    [w, h],
                    [[0.97, 0.07], [0.03, 0.09], [0.03, 0.93], [0.97, 0.91]],
                    false,
                ),
            ] {
                let p = SceneParameters {
                    output,
                    uvs,
                    matrix,
                    correction,
                    limit: 65504.,
                    direct,
                };
                let scene = context.scene(&rgb, w, noise, &p, &|| false)?;
                let got = scene.read_pixels()?;
                let mut worst = 0_f32;
                for y in 0..output[1] {
                    for x in 0..output[0] {
                        let camera = if direct {
                            reference[y * w + x]
                        } else {
                            let fy = (y as f32 + 0.5) / output[1] as f32;
                            let fx = (x as f32 + 0.5) / output[0] as f32;
                            let uv = uvs.map(|p| eframe::egui::pos2(p[0], p[1]));
                            let point = uv[0].lerp(uv[3], fy).lerp(uv[1].lerp(uv[2], fy), fx);
                            let sx = (point.x * w as f32 - 0.5).clamp(0., (w - 1) as f32);
                            let sy = (point.y * h as f32 - 0.5).clamp(0., (h - 1) as f32);
                            let (x0, y0) = (sx as usize, sy as usize);
                            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
                            let (dx, dy) = (sx - x0 as f32, sy - y0 as f32);
                            std::array::from_fn(|c| {
                                (reference[y0 * w + x0][c] * (1. - dx)
                                    + reference[y0 * w + x1][c] * dx)
                                    * (1. - dy)
                                    + (reference[y1 * w + x0][c] * (1. - dx)
                                        + reference[y1 * w + x1][c] * dx)
                                        * dy
                            })
                        };
                        let expected = white_balance::apply(
                            matrix,
                            std::array::from_fn(|c| f64::from(camera[c]) * correction[c]),
                        )
                        .map(|v| v as f32);
                        let actual = got[y * output[0] + x];
                        for c in 0..3 {
                            let error = (actual[c] - expected[c]).abs();
                            worst = worst.max(error);
                            ensure!(
                                error < 0.00003,
                                "resident {w}x{h} noise={noise} direct={direct} at{x},{y} channel={c} error={error}"
                            );
                        }
                        ensure!(actual[3] == 1., "alpha changed");
                    }
                }
                let n = [output[0].min(128), output[1].min(128)];
                for y in 0..n[1] {
                    for x in 0..n[0] {
                        let sx = (2 * x + 1) * output[0] / (2 * n[0]);
                        let sy = (2 * y + 1) * output[1] / (2 * n[1]);
                        ensure!(
                            scene.samples[y * n[0] + x] == got[sy * output[0] + sx],
                            "histogram sample mismatch"
                        );
                    }
                }
                cases += 1;
                println!("PASS {w}x{h} noise={noise} direct={direct} max_error={worst}");
            }
        }
        let p = SceneParameters {
            output: [w, h],
            uvs: [[0., 0.], [1., 0.], [1., 1.], [0., 1.]],
            matrix,
            correction,
            limit: 65504.,
            direct: true,
        };
        ensure!(
            context.scene(&rgb, w, 25, &p, &|| true).is_err(),
            "initial cancellation committed"
        );
        let calls = std::sync::atomic::AtomicUsize::new(0);
        ensure!(
            context
                .scene(&rgb, w, 25, &p, &|| calls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    > 0)
                .is_err(),
            "late cancellation committed"
        );
        ensure!(
            context.scene(&rgb, w, 101, &p, &|| false).is_err(),
            "invalid noise accepted"
        );
        let low = SceneParameters { limit: 0.001, ..p };
        ensure!(
            context.scene(&rgb, w, 0, &low, &|| false).is_err(),
            "overflow accepted"
        );
    }
    println!("PASS {cases} resident cases; cancellation, range checks, histogram, CPU export");
    Ok(())
}
#[cfg(not(any(windows, target_os = "macos")))]
fn main() {}
