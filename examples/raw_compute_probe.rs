// Native GPU acceptance runs on supported desktop backends only.
#[cfg(any(windows, target_os = "macos"))]
mod native {
    //! Native GPU correctness/performance acceptance; no photographs are modified.
    use anyhow::{Result, ensure};
    use fastcull::{raw_chroma, raw_compute};
    use std::time::Instant;
    fn fixture(w: usize, h: usize) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let x = i % w;
                let y = i / w;
                let lum = 0.02 + 0.8 * (x as f32 / w as f32) + 0.3 * (y as f32 / h as f32);
                let n = (((i.wrapping_mul(1664525).wrapping_add(1013904223)) >> 17) % 101) as f32
                    / 100.
                    - 0.5;
                if x < w / 2 {
                    [lum + n * 0.03, lum - n * 0.03, lum + n * 0.03]
                } else {
                    [lum * 1.8, lum * 0.7, lum * 0.3]
                }
            })
            .collect()
    }
    pub fn run() -> Result<()> {
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
            .expect("native adapter");
        println!("GPU {:?}", adapter.get_info());
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        raw_compute::install(device.clone(), queue.clone());
        let gpu = raw_compute::Context::new(device, queue);
        for (w, h, tile) in [
            (1, 1, 1),
            (23, 17, 3),
            (1601, 17, 5),
            (3201, 19, 5),
            (6401, 21, 5),
            (1, 6401, 3),
        ] {
            let input = fixture(w, h);
            for amount in [0, 1, 25, 75, 100] {
                let mut cpu = input.clone();
                raw_chroma::reduce(&mut cpu, w, amount, &|| false)?;
                let got = gpu.reduce_with_tile_rows(&input, w, amount, &|| false, tile)?;
                let mut max_error = 0_f32;
                for (a, b) in cpu.iter().zip(&got) {
                    for c in 0..3 {
                        let error = (a[c] - b[c]).abs();
                        max_error = max_error.max(error);
                        ensure!(
                            error <= 0.00002 + 0.00005 * a[c].abs(),
                            "GPU mismatch {w}x{h} amount={amount} {a:?} {b:?}"
                        );
                    }
                }
                println!("CHECK {w}x{h} amount={amount} max_error={max_error}");
            }
        }
        let w = 6192;
        let h = 4128;
        let input = fixture(w, h);
        let mut cpu = input.clone();
        let start = Instant::now();
        raw_chroma::reduce(&mut cpu, w, 25, &|| false)?;
        let cpu_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let got = gpu.reduce(&input, w, 25, &|| false)?;
        let gpu_ms = start.elapsed().as_secs_f64() * 1000.;
        let max_error = cpu
            .iter()
            .zip(&got)
            .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs()))
            .fold(0_f32, f32::max);
        ensure!(max_error < 0.0001, "large image error {max_error}");
        println!(
            "BENCH pixels={} cpu_ms={cpu_ms:.3} gpu_ms={gpu_ms:.3} max_error={max_error}",
            w * h
        );
        ensure!(
            gpu_ms < cpu_ms,
            "GPU path is slower including uploads/readback"
        );
        ensure!(
            gpu.reduce(&input, w, 25, &|| true).is_err(),
            "cancelled GPU work committed"
        );
        let small = fixture(1601, 17);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        ensure!(
            gpu.reduce_with_tile_rows(
                &small,
                1601,
                25,
                &|| calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 2,
                3
            )
            .is_err(),
            "partly cancelled stripes committed"
        );
        ensure!(
            gpu.reduce(&small, 0, 25, &|| false).is_err(),
            "invalid dimensions accepted"
        );
        ensure!(
            gpu.reduce(&small, 1601, 101, &|| false).is_err(),
            "invalid amount accepted"
        );
        let mut invalid = small.clone();
        invalid[0][0] = f32::NAN;
        ensure!(
            gpu.reduce(&invalid, 1601, 25, &|| false).is_err(),
            "nonfinite input accepted"
        );
        let wide = fixture(4_000_000, 1);
        ensure!(
            raw_compute::try_reduce(&wide, 4_000_000, 25, &|| false)?.is_none(),
            "unsupported GPU dimensions did not fall back"
        );
        println!("CANCEL initial/mid-stripe; invalid input and bounded-buffer fallback PASS");
        Ok(())
    }
}
#[cfg(any(windows, target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    native::run()
}
#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("Native GPU probe requires Windows or macOS");
}
