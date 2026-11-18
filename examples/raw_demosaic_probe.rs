//! Native sensor-RGB comparison before profiles/NR, using production AHD hooks.
#[cfg(any(windows, target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    struct Logger;
    impl log::Log for Logger {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, r: &log::Record<'_>) {
            if r.target().starts_with("fastcull") {
                eprintln!("{} {}", r.level(), r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: Logger = Logger;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let mut gpu_context = None;
    if args.first().is_some_and(|v| v == "--gpu") {
        args.remove(0);
        let backend = if cfg!(windows) {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::METAL
        };
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: backend,
            ..Default::default()
        });
        let adapter = instance
            .enumerate_adapters(backend)
            .into_iter()
            .min_by_key(|a| u8::from(a.get_info().vendor != 0x10de))
            .context("native adapter")?;
        println!("GPU {:?}", adapter.get_info());
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        gpu_context = Some(fastcull::raw_compute::Context::new(
            device.clone(),
            queue.clone(),
        ));
        fastcull::raw_compute::install(device, queue);
    }
    if args.as_slice() == ["--self-test"] {
        let gpu = gpu_context.context("--gpu required")?;
        let table: Vec<_> = (0..65536)
            .map(|i| {
                let r = i as f32 / 65535.;
                if r > 0.008856 {
                    r.powf(1. / 3.)
                } else {
                    7.787 * r + 16. / 116.
                }
            })
            .collect();
        for (w, h) in [(32, 32), (301, 145)] {
            for byte in [0x94u32, 0x16, 0x61, 0x49] {
                let filters = byte * 0x01010101;
                let expected = vec![[12000, 16000, 9000, 0]; w * h];
                let image: Vec<_> = (0..w * h)
                    .map(|i| {
                        let (x, y) = (i % w, i / w);
                        if x < 5 || y < 5 || x >= w - 5 || y >= h - 5 {
                            return expected[i];
                        }
                        let shift = ((((y as u32) << 1) & 14) | ((x as u32) & 1)) << 1;
                        let c = ((filters >> shift) & 3) as usize;
                        let mut pixel = [0; 4];
                        pixel[c] = expected[i][c];
                        pixel
                    })
                    .collect();
                let b = fastcull::raw_compute::Bayer {
                    image: &image,
                    width: w,
                    height: h,
                    filters,
                    xyz: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
                    cube_root: &table,
                };
                let out = gpu.demosaic(&b, &|| false)?;
                ensure!(expected == out, "flat-field AHD/stripe mismatch {byte:x}");
                let calls = std::sync::atomic::AtomicUsize::new(0);
                ensure!(
                    gpu.demosaic(&b, &|| calls
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        > 1)
                        .is_err(),
                    "AHD cancellation committed"
                );
                println!("AHD flat-field/stripe/cancel {w}x{h} CFA={byte:x} PASS");
            }
        }
        return Ok(());
    }
    ensure!(
        args.len() == 2,
        "[--gpu] RAW output-float32 | --gpu --self-test"
    );
    let start = std::time::Instant::now();
    let image = fastcull::native_raw::develop_linear(
        std::path::Path::new(&args[0]),
        &fastcull::native_raw::DevelopSettings {
            max_edge: 0,
            temperature: Some(5500.),
            tint: 0.,
            exposure: 0.,
        },
        &|| false,
    )?;
    println!(
        "size={}x{} unpack_ms={:.3} process_ms={:.3} wall_ms={:.3}",
        image.width,
        image.height,
        image.unpack_ms,
        image.process_ms,
        start.elapsed().as_secs_f64() * 1000.
    );
    std::fs::write(&args[1], bytemuck::cast_slice(&image.rgb))?;
    Ok(())
}
#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("native GPU probe requires Windows or macOS");
}
