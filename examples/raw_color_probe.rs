//! Render a RAW/XMP pair through production CPU display formulas for comparisons.
#[allow(dead_code, unused_imports)]
#[path = "../src/raw_preview.rs"]
mod raw_preview;
use anyhow::{Context, Result, ensure};
use fastcull::{dcp::Encoding, local_raw, xmp};
use std::{fs, path::PathBuf};

fn main() -> Result<()> {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
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
    if args.len() == 2 && args[0] == "--inventory" {
        let path = PathBuf::from(&args[1]);
        let started = std::time::Instant::now();
        let info = fastcull::arw::ArwInfo::read(&path)?;
        let metadata = started.elapsed();
        let choices =
            fastcull::raw_profiles::choices(&path, info.capture.camera.as_deref().unwrap_or(""));
        println!(
            "metadata_ms={:.2} inventory_ms={:.2} choices={}",
            metadata.as_secs_f64() * 1000.,
            (started.elapsed() - metadata).as_secs_f64() * 1000.,
            choices.len()
        );
        return Ok(());
    }
    if args.len() == 4 && args[0] == "--profile-xmp" {
        let packet = fs::read_to_string(&args[1])?;
        let name = args[2].to_str().context("profile must be UTF-8")?;
        let patched = xmp::patch(
            &packet,
            &xmp::Updates {
                camera_profile: Some(name.into()),
                ..Default::default()
            },
        )?;
        fs::write(&args[3], patched)?;
        return Ok(());
    }
    ensure!(
        args.len() == 3 || args.len() == 4,
        "raw_color_probe <RAW> <XMP> <PNG> [max-edge]"
    );
    let source = PathBuf::from(&args[0]);
    let packet = fs::read_to_string(&args[1])?;
    let values = xmp::parse_values(&packet)?;
    let edge = args
        .get(3)
        .map(|v| -> Result<u32> { Ok(v.to_str().context("edge must be UTF-8")?.parse()?) })
        .transpose()?
        .unwrap_or(1280);
    let rendered = local_raw::render_scene(&local_raw::SceneRequest {
        source,
        xmp: packet,
        max_edge: edge,
    })?;
    let mut frame = rendered.frame;
    frame.materialize_hue_sat(&|| false)?;
    println!("TIMINGS {:?}", rendered.timings);
    let recipe = raw_preview::Recipe {
        hue_sat: frame.pending_hue_sat,
        extra_look: frame
            .recipe
            .extra_look
            .as_ref()
            .map(|v| raw_preview::HsvLookup {
                dims: v.dims,
                data: v.data.clone(),
                encoding: u32::from(v.encoding == Encoding::Srgb),
            }),
        look: frame.recipe.look.as_ref().map(|v| raw_preview::HsvLookup {
            dims: v.dims,
            data: v.data.clone(),
            encoding: u32::from(v.encoding == Encoding::Srgb),
        }),
        curve: frame.recipe.curve,
        baseline_exposure: frame.recipe.baseline_exposure,
    };
    let mut settings = raw_preview::Settings {
        exposure: values.exposure.unwrap_or(0) as f32 / 1_000_000.,
        ..Default::default()
    };
    let b = values.basic;
    settings.basic = [
        b.contrast,
        b.highlights,
        b.shadows,
        b.whites,
        b.blacks,
        b.vibrance,
        b.saturation,
    ]
    .map(|v| v.unwrap_or(0) as f32 / 100.);
    let pixels: Vec<u8> = frame
        .pixels
        .iter()
        .flat_map(|p| {
            raw_preview::evaluate_scene([p[0], p[1], p[2]], settings, &recipe)
                .map(|v| (v * 255.).round() as u8)
        })
        .collect();
    let mut png = png::Encoder::new(
        fs::File::create(&args[2])?,
        frame.size[0] as u32,
        frame.size[1] as u32,
    );
    png.set_color(png::ColorType::Rgb);
    png.set_depth(png::BitDepth::Eight);
    png.set_source_srgb(png::SrgbRenderingIntent::RelativeColorimetric);
    png.write_header()?.write_image_data(&pixels)?;
    println!(
        "PROFILE\t{}\nWARNING\t{}\nBASELINE\t{}\nSIZE\t{}\t{}",
        frame.profile,
        frame.warning.as_deref().unwrap_or(""),
        recipe.baseline_exposure,
        frame.size[0],
        frame.size[1]
    );
    Ok(())
}
