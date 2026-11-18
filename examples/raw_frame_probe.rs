//! Full-resolution production frame/timing dump for read-only regressions.
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::{io::Write, path::PathBuf, time::Instant};
    struct Logger;
    impl log::Log for Logger {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, r: &log::Record<'_>) {
            if r.target().starts_with("fastcull") {
                eprintln!("{}", r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: Logger = Logger;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(args.len() == 3, "RAW XMP-or-dash float32-output");
    #[cfg(any(windows, target_os = "macos"))]
    {
        let start = Instant::now();
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
            .context("native GPU")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        fastcull::raw_compute::install(device, queue);
        println!("device_ms={:.3}", start.elapsed().as_secs_f64() * 1000.);
    }
    let source = PathBuf::from(&args[0]);
    let xmp = if args[1] == "-" {
        String::new()
    } else {
        std::fs::read_to_string(&args[1])?
    };
    let mut result = fastcull::local_raw::render_scene(&fastcull::local_raw::SceneRequest {
        source,
        xmp,
        max_edge: 0,
    })?;
    println!(
        "TIMINGS {:?} size={:?} profile={}",
        result.timings, result.frame.size, result.frame.profile
    );
    result.frame.materialize_hue_sat(&|| false)?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(&args[2])?);
    out.write_all(bytemuck::cast_slice(&result.frame.pixels))?;
    out.flush()?;
    Ok(())
}
