//! Read-only diagnosis using the production display formulas; no Adobe calls.
#[allow(dead_code, unused_imports)]
#[path = "../src/raw_preview.rs"]
mod raw_preview;
use anyhow::{Result, ensure};
use fastcull::{dcp::Encoding, local_raw, raw_color, xmp};
use raw_preview::{HsvLookup, Recipe, Settings, evaluate_scene};
use std::{fs, io::Write, path::PathBuf};

fn curve() -> Vec<f32> {
    let tone = raw_color::default_tone();
    (0..4096).map(|i| tone.eval(i as f32 / 4095.)).collect()
}
fn main() -> Result<()> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    // Diagnostic single-variable test of the public classic SDK ordering.
    // Restrict interpretation to >=0 total EV with all basic controls at zero.
    let prelook_clip = args.first().is_some_and(|v| v == "--prelook-clip");
    if prelook_clip {
        args.remove(0);
    }
    let pair = args.first().is_some_and(|v| v == "--pair");
    if pair {
        args.remove(0);
    }
    let sample_fit = args.first().is_some_and(|v| v == "--sample-fit");
    if sample_fit {
        args.remove(0);
    }
    if args.first().is_some_and(|v| v == "--ramp") {
        ensure!(
            args.len() == 2 || args.len() == 3,
            "--ramp CSV [--require-highlight-detail]"
        );
        let recipe = Recipe {
            curve: curve(),
            ..Default::default()
        };
        let mut out = fs::File::create(&args[1])?;
        writeln!(out, "input,ev,r,g,b")?;
        for ev in [-2., -1., 0., 0.5, 1., 2., 3.] {
            for i in 0..=2048 {
                let x = i as f32 / 1024.;
                let rgb = evaluate_scene(
                    [x; 3],
                    Settings {
                        exposure: ev,
                        ..Default::default()
                    },
                    &recipe,
                );
                writeln!(out, "{x},{ev},{},{},{}", rgb[0], rgb[1], rgb[2])?;
            }
        }
        let settings = Settings {
            exposure: 1.,
            ..Default::default()
        };
        let a = evaluate_scene([0.55; 3], settings, &recipe);
        let b = evaluate_scene([0.85; 3], settings, &recipe);
        println!("+1EV input0.55={a:?} input0.85={b:?}");
        if args
            .get(2)
            .is_some_and(|v| v == "--require-highlight-detail")
        {
            ensure!(
                a != b,
                "positive exposure collapsed distinct unclipped scene values to the same white output"
            );
        }
        return Ok(());
    }
    ensure!(
        args.len() == 3 || args.len() == 4,
        "RAW XMP OUT-DIR [max-edge]"
    );
    let source = PathBuf::from(&args[0]);
    let packet = fs::read_to_string(&args[1])?;
    let values = xmp::parse_values(&packet)?;
    let edge = args.get(3).map(|v| v.parse()).transpose()?.unwrap_or(1280);
    let directory = PathBuf::from(&args[2]);
    fs::create_dir_all(&directory)?;
    let mut rendered = local_raw::render_scene(&local_raw::SceneRequest {
        source,
        xmp: packet,
        max_edge: edge,
    })?;
    rendered.frame.materialize_hue_sat(&|| false)?;
    let frame = rendered.frame;
    let lookup = |m: fastcull::dcp::HsvMap| HsvLookup {
        dims: m.dims,
        data: m.data,
        encoding: u32::from(m.encoding == Encoding::Srgb),
    };
    let recipe = Recipe {
        hue_sat: frame.pending_hue_sat,
        look: frame.recipe.look.map(lookup),
        extra_look: frame.recipe.extra_look.map(lookup),
        curve: frame.recipe.curve,
        baseline_exposure: frame.recipe.baseline_exposure,
    };
    let basic = values.basic;
    let settings = Settings {
        basic: [
            basic.contrast,
            basic.highlights,
            basic.shadows,
            basic.whites,
            basic.blacks,
            basic.vibrance,
            basic.saturation,
        ]
        .map(|v| v.unwrap_or(0) as f32 / 100.),
        ..Default::default()
    };
    let mut raw = std::io::BufWriter::new(fs::File::create(directory.join("scene.f32"))?);
    raw.write_all(bytemuck::cast_slice(&frame.pixels))?;
    raw.flush()?;
    fs::write(
        directory.join("metadata.txt"),
        format!(
            "size={:?}\nprofile={}\nfingerprint={}\nbaseline_ev={}\ntimings={:?}\n",
            frame.size,
            frame.profile,
            frame.profile_fingerprint,
            recipe.baseline_exposure,
            rendered.timings
        ),
    )?;
    let display_size = if sample_fit {
        let scale = 1280. / frame.size[0].max(frame.size[1]) as f32;
        frame
            .size
            .map(|n| (n as f32 * scale.min(1.)).round().max(1.) as usize)
    } else {
        frame.size
    };
    let sampled = sample_fit.then(|| {
        (0..display_size[0] * display_size[1])
            .map(|i| {
                let x = i % display_size[0];
                let y = i / display_size[0];
                let sx = ((x as f32 + 0.5) / display_size[0] as f32 * frame.size[0] as f32 - 0.5)
                    .clamp(0., (frame.size[0] - 1) as f32);
                let sy = ((y as f32 + 0.5) / display_size[1] as f32 * frame.size[1] as f32 - 0.5)
                    .clamp(0., (frame.size[1] - 1) as f32);
                let (x0, y0) = (sx as usize, sy as usize);
                let (x1, y1) = (
                    (x0 + 1).min(frame.size[0] - 1),
                    (y0 + 1).min(frame.size[1] - 1),
                );
                let (dx, dy) = (sx - x0 as f32, sy - y0 as f32);
                std::array::from_fn::<_, 4, _>(|c| {
                    (frame.pixels[y0 * frame.size[0] + x0][c] * (1. - dx)
                        + frame.pixels[y0 * frame.size[0] + x1][c] * dx)
                        * (1. - dy)
                        + (frame.pixels[y1 * frame.size[0] + x0][c] * (1. - dx)
                            + frame.pixels[y1 * frame.size[0] + x1][c] * dx)
                            * dy
                })
            })
            .collect::<Vec<_>>()
    });
    let display_pixels = sampled.as_deref().unwrap_or(&frame.pixels);
    let exposures: &[f32] = if pair {
        &[0., 1.]
    } else {
        &[-2., -1., 0., 0.5, 1., 2., 3.]
    };
    for &ev in exposures {
        let gain = 2f32.powf((ev + recipe.baseline_exposure).clamp(-20., 20.));
        let rgba: Vec<u8> = display_pixels
            .iter()
            .flat_map(|p| {
                let rgb = [p[0], p[1], p[2]].map(|v| {
                    if prelook_clip && ev + recipe.baseline_exposure >= 0. {
                        (v * gain).clamp(0., 1.) / gain
                    } else {
                        v
                    }
                });
                evaluate_scene(
                    rgb,
                    Settings {
                        exposure: ev,
                        ..settings
                    },
                    &recipe,
                )
                .map(|v| (v * 255.).round() as u8)
            })
            .collect();
        let file = fs::File::create(directory.join(format!("ev-{ev:+.2}.png")))?;
        let mut png = png::Encoder::new(file, display_size[0] as u32, display_size[1] as u32);
        png.set_color(png::ColorType::Rgb);
        png.set_depth(png::BitDepth::Eight);
        png.set_source_srgb(png::SrgbRenderingIntent::RelativeColorimetric);
        png.write_header()?.write_image_data(&rgba)?;
    }
    println!(
        "{} {:?} baselineEV={}",
        frame.profile, frame.size, recipe.baseline_exposure
    );
    Ok(())
}
