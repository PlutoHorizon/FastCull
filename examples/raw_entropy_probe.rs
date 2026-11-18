//! Compare complete GPU lossless-JPEG tiles against a separate CPU prefix decoder.
//!
//! Diagnostics are part of the output on purpose: the encoded storage size of a
//! RAW differs from the active image size (Sony ARW stores 6656x4608 and shows
//! 6192x4128), and passing the active size used to be reported as "no supported
//! tiled RAW directory" without naming either number. `--auto` reads the
//! storage size from the file itself; the negative case keeps failing with an
//! explicit directory listing.
#[cfg(any(windows, target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use fastcull::raw_entropy::{self, Kind};
    use std::time::Instant;
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
    let mut auto = false;
    let mut mode = String::from("both");
    let mut positional: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--auto" | "--storage-size" => {
                auto = true;
                index += 1;
            }
            "--gpu-only" => {
                mode = "gpu".into();
                index += 1;
            }
            "--cpu-only" => {
                mode = "cpu".into();
                index += 1;
            }
            "--mode" => {
                mode = args
                    .get(index + 1)
                    .cloned()
                    .context("--mode needs a value")?;
                index += 2;
            }
            other => {
                positional.push(other.to_string());
                index += 1;
            }
        }
    }
    ensure!(
        matches!(mode.as_str(), "both" | "cpu" | "gpu"),
        "mode must be both|cpu|gpu"
    );
    ensure!(
        positional.len() == 2 || positional.len() == 4,
        "usage: raw_entropy_probe [--auto|--storage-size] [--mode both|cpu|gpu] \
         sony|dng <file> [raw-width raw-height]"
    );
    let kind = match positional[0].as_str() {
        "sony" => Kind::Sony,
        "dng" => Kind::LinearDng,
        _ => anyhow::bail!("kind must be sony|dng"),
    };
    let path = std::path::Path::new(&positional[1]);
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let found = raw_entropy::directories(&bytes, kind)?;
    for directory in &found {
        println!(
            "DIR {} supported_stream={} dimension_match={}",
            directory.summary(),
            directory.stream_supported,
            directory.dimension_match
        );
    }
    let requested = if auto || positional.len() == 2 {
        raw_entropy::storage_size(&bytes, kind)?
    } else {
        (positional[2].parse()?, positional[3].parse()?)
    };
    let storage = raw_entropy::storage_size(&bytes, kind).ok();
    println!(
        "DISPATCH kind={kind:?} requested={}x{} storage={} mode={mode} directories={}",
        requested.0,
        requested.1,
        storage
            .map(|(w, h)| format!("{w}x{h}"))
            .unwrap_or_else(|| "unknown".into()),
        found.len()
    );
    let start = Instant::now();
    let image = raw_entropy::read(path, kind, requested.0, requested.1)?;
    let parse_ms = start.elapsed().as_secs_f64() * 1000.;
    println!(
        "PARSE tiles={} bytes={} ms={parse_ms:.3} samples={}",
        image.tiles.len(),
        image.tiles.iter().map(|t| t.data.len()).sum::<usize>(),
        image.width * image.height * image.channels
    );
    if mode == "cpu" {
        let start = Instant::now();
        let cpu = raw_entropy::decode_cpu(&image, &|| false)?;
        let cpu_ms = start.elapsed().as_secs_f64() * 1000.;
        println!(
            "ENTROPY backend=parallel-cpu samples={} cpu_ms={cpu_ms:.3} equality_checked=false",
            cpu.len()
        );
        return Ok(());
    }
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
    let gpu = fastcull::raw_compute::Context::new(device, queue);
    let mut cpu_ms = None;
    let cpu = if mode == "gpu" {
        Vec::new()
    } else {
        let start = Instant::now();
        let cpu = raw_entropy::decode_cpu(&image, &|| false)?;
        cpu_ms = Some(start.elapsed().as_secs_f64() * 1000.);
        println!(
            "ENTROPY backend=parallel-cpu samples={} cpu_ms={:.3} equality_checked=pending",
            cpu.len(),
            cpu_ms.unwrap_or_default()
        );
        cpu
    };
    let start = Instant::now();
    let (out, transfer) = gpu.decode_entropy(&image, &|| false)?;
    let gpu_ms = start.elapsed().as_secs_f64() * 1000.;
    ensure!(
        mode == "gpu" || cpu.len() == out.len(),
        "sample count changed"
    );
    if let Some((i, (a, b))) = cpu.iter().zip(&out).enumerate().find(|(_, (a, b))| a != b) {
        anyhow::bail!("entropy mismatch at {i}: CPU {a} GPU {b}");
    }
    println!(
        "ENTROPY backend=gpu samples={} cpu_ms={} gpu_ms={gpu_ms:.3} h2d_bytes={} d2h_bytes={} \
         equality_checked={}",
        out.len(),
        cpu_ms.map_or("n/a".to_string(), |ms| format!("{ms:.3}")),
        transfer.0,
        transfer.1,
        mode != "gpu"
    );
    ensure!(
        gpu.decode_entropy(&image, &|| true).is_err(),
        "cancelled entropy committed"
    );
    Ok(())
}
#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("native GPU required");
}
