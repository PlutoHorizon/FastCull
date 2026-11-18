//! Compare LibRaw and custom entropy at the same native unpack boundary.
#[cfg(any(windows, target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use fastcull::raw_sensor::{self, Backend};
    use std::{path::Path, time::Instant};
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        !args.is_empty() && args.len() <= 2,
        "raw_sensor_probe <private RAW> [--gpu]"
    );
    let path = Path::new(&args[0]);
    let started = Instant::now();
    let reference = raw_sensor::capture(path, Backend::LibRaw, None, &|| false)?;
    let metadata = &reference.metadata;
    let mut hash = 0xcbf29ce484222325_u64;
    for v in &reference.samples {
        for b in v.to_le_bytes() {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
        }
    }
    println!(
        "{{\"stage\":\"{}\",\"width\":{},\"height\":{},\"active\":{:?},\"channels\":{},\"storage_channels\":{},\"row_stride\":{},\"bits\":{},\"black\":{},\"white\":{},\"decoder\":{},\"curve_decoder_managed\":{},\"samples\":{},\"sample_fnv64\":\"{hash:016x}\",\"reference_ms\":{:.3}}}",
        raw_sensor::STAGE,
        metadata.size[0],
        metadata.size[1],
        metadata.active,
        metadata.channels,
        metadata.storage_channels,
        metadata.row_stride,
        metadata.bits,
        metadata.black,
        metadata.white,
        fastcull::raw_trace::string(&metadata.decoder),
        metadata.flags & 1 != 0,
        reference.samples.len(),
        started.elapsed().as_secs_f64() * 1000.
    );
    let cpu = raw_sensor::capture(path, Backend::Parallel, None, &|| false)?;
    reference.compare(&cpu)?;
    let supported = matches!(
        metadata.decoder.as_str(),
        "sony_ljpeg_load_raw()" | "lossless_dng_load_raw()"
    );
    ensure!(
        !supported || cpu.adopted,
        "parallel path silently fell back"
    );
    println!(
        "{{\"backend\":\"parallel\",\"adopted\":{},\"all_metadata_equal\":true,\"all_samples_equal\":true}}",
        cpu.adopted
    );
    if args.get(1).is_some_and(|s| s == "--gpu") {
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
        let gpu = fastcull::raw_compute::Context::new(device, queue);
        let output = raw_sensor::capture(path, Backend::Gpu, Some(&gpu), &|| false)?;
        reference.compare(&output)?;
        ensure!(!supported || output.adopted, "GPU path silently fell back");
        println!(
            "{{\"backend\":\"gpu\",\"adopted\":{},\"all_metadata_equal\":true,\"all_samples_equal\":true}}",
            output.adopted
        );
    }
    ensure!(
        raw_sensor::capture(path, Backend::LibRaw, None, &|| true).is_err(),
        "cancelled sensor snapshot published"
    );
    println!("{{\"cancellation_rejected\":true,\"native_pointers_retained\":false}}");
    Ok(())
}
#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("requires native LibRaw");
}
