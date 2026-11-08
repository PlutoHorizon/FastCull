//! Render a RAW/XMP pair through production CPU display formulas for comparisons.
#[allow(dead_code, unused_imports)]
#[path = "../src/raw_preview.rs"]
mod raw_preview;
use anyhow::{Context, Result, ensure};
use fastcull::{dcp::Encoding, local_raw, xmp};
use std::{fs, path::PathBuf};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
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
    let frame = rendered.frame;
    let recipe = raw_preview::Recipe {
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
