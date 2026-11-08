//! Decode all culler levels and save a bounded visual reference.
use anyhow::{Result, ensure};
use fastcull::{
    arw::ArwInfo,
    imaging::{self, Level},
};
use std::{fs, path::PathBuf, time::Instant};
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().expect("DNG path"));
    let output = PathBuf::from(args.next().expect("output directory"));
    fs::create_dir_all(&output)?;
    let info = ArwInfo::read(&path)?;
    println!("image_size={:?}", info.image_size);
    let mut failures = 0;
    for (name, level) in [
        ("thumb", Level::Thumb),
        ("fit", Level::Fit(2560)),
        ("full", Level::Full),
    ] {
        let started = Instant::now();
        match imaging::decode_level(&path, &info, level) {
            Ok(p) => {
                println!(
                    "{name}: {}x{} {:.3}s",
                    p.width,
                    p.height,
                    started.elapsed().as_secs_f64()
                );
                if level == Level::Full {
                    ensure!(
                        Some((p.width as u32, p.height as u32)) == info.image_size,
                        "Full must contain original pixels"
                    );
                }
                let scale = (p.width as f64 / 1600.).max(1.);
                let (w, h) = (
                    (p.width as f64 / scale) as usize,
                    (p.height as f64 / scale) as usize,
                );
                let mut rgba = Vec::with_capacity(w * h * 4);
                for y in 0..h {
                    for x in 0..w {
                        let offset = ((y as f64 * scale) as usize * p.width
                            + (x as f64 * scale) as usize)
                            * 4;
                        rgba.extend_from_slice(&p.rgba[offset..offset + 4]);
                    }
                }
                let p = imaging::Pixels {
                    width: w,
                    height: h,
                    rgba,
                };
                let file = fs::File::create(output.join(format!("{name}.png")))?;
                let mut encoder = png::Encoder::new(file, p.width as u32, p.height as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.write_header()?.write_image_data(&p.rgba)?;
            }
            Err(e) => {
                eprintln!("{name}: {e:#}");
                failures += 1;
            }
        }
    }
    fastcull::dng::clear_decode_cache();
    ensure!(failures == 0, "{failures} decode levels failed");
    Ok(())
}
