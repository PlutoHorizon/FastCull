//! Read-only latency probe of the production scene/cache paths.
use anyhow::{Context as _, Result, ensure};
use fastcull::{local_raw, raw_float_cache, xmp};
use std::{path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|v| v == "--gpu") {
        args.remove(0);
        #[cfg(any(windows, target_os = "macos"))]
        {
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
            println!("GPU {:?}", adapter.get_info());
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
            fastcull::raw_compute::install(device, queue);
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        anyhow::bail!("native GPU probe requires Windows or macOS");
    }
    ensure!(
        args.len() >= 2,
        "--cache <fcr> [max-ms] | --scene <raw> <xmp> <edge> [require-reuse]"
    );
    if args[0] == "--cache" {
        let limit = args.get(2).map(|s| s.parse::<f64>()).transpose()?;
        for _ in 0..3 {
            let start = Instant::now();
            let frame = raw_float_cache::read(std::path::Path::new(&args[1]))?;
            let ms = start.elapsed().as_secs_f64() * 1000.;
            println!("cache_ms={ms:.3} pixels={}", frame.pixels.len());
            if let Some(limit) = limit {
                ensure!(ms < limit, "cache read {ms:.3} exceeds {limit} ms");
            }
        }
        return Ok(());
    }
    ensure!(args[0] == "--scene" && args.len() >= 4, "invalid arguments");
    let raw = PathBuf::from(&args[1]);
    let packet = std::fs::read_to_string(&args[2])?;
    let edge: u32 = args[3].parse()?;
    let profile = xmp::patch(
        &packet,
        &xmp::Updates {
            camera_profile: Some("Adobe Color".into()),
            ..Default::default()
        },
    )?;
    let wb = xmp::patch(
        &packet,
        &xmp::Updates {
            white_balance: Some(xmp::WhiteBalance::Custom {
                temperature: 6200,
                tint: 10,
            }),
            ..Default::default()
        },
    )?;
    for (label, xmp) in [
        ("first", &packet),
        ("repeat", &packet),
        ("profile", &profile),
        ("wb", &wb),
        ("wb-repeat", &wb),
    ] {
        let rendered = local_raw::render_scene(&local_raw::SceneRequest {
            source: raw.clone(),
            xmp: xmp.clone(),
            max_edge: edge,
        })?;
        println!("{label} {:?}", rendered.timings);
        if args.get(4).is_some() && ["repeat", "profile", "wb-repeat"].contains(&label) {
            ensure!(
                rendered.timings.process_ms == 0.,
                "{label} needlessly reprocessed sensor RGB: {:.3} ms",
                rendered.timings.process_ms
            );
        }
    }
    Ok(())
}
