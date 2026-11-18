//! Local chroma-only filtering of linear, white-balanced camera RGB.
//! Keep each pixel's luminance and use both luminance and colour edges as guides.
use anyhow::{Result, ensure};

pub fn reduce(
    rgb: &mut [[f32; 3]],
    width: usize,
    amount: u32,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    ensure!(
        width > 0 && rgb.len().is_multiple_of(width),
        "彩噪处理尺寸无效"
    );
    let height = rgb.len() / width;
    let guide: Vec<[f32; 3]> = rgb
        .iter()
        .map(|p| {
            let y = (p[0] + 2. * p[1] + p[2]) * 0.25;
            let scale = y.max(1e-6);
            [y, (p[0] - y) / scale, (p[2] - y) / scale]
        })
        .collect();
    let mut horizontal = vec![[0.; 2]; rgb.len()];
    // Chroma speckles become multi-pixel blotches at 100%. Keep the spatial
    // support comparable to the fit preview instead of using its tiny kernel
    // unchanged on a 24–60 MP image. Luminance is still never averaged.
    let detail_scale = width.max(height).div_ceil(1600).clamp(1, 4);
    let radius = (if amount > 50 { 3 } else { 2 }) * detail_scale;
    let spatial: Vec<f32> = (0..=radius)
        .map(|d| {
            let distance = d as f32 / detail_scale as f32;
            let knots = [6., 4., 1., 0.25];
            let lo = (distance as usize).min(2);
            knots[lo] + (knots[lo + 1] - knots[lo]) * (distance - lo as f32)
        })
        .collect();
    let strength = (amount as f32 / 25.).min(1.);
    let weight = |a: [f32; 3], b: [f32; 3], distance: usize| {
        let dy = (a[0] - b[0]) / (0.002 + 0.15 * a[0].max(b[0]));
        let dc = ((a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)) / 0.72;
        spatial[distance] / (1. + dy.powi(2) * dy.powi(2) + dc.powi(4))
    };
    crate::raw_parallel::rows(&mut horizontal, width, |y, row| {
        if y % 16 == 0 {
            ensure!(!cancelled(), "已取消彩噪处理");
        }
        for (x, pixel) in row.iter_mut().enumerate() {
            let i = y * width + x;
            let mut sum = [0.; 2];
            let mut total = 0.;
            for sx in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
                let p = guide[y * width + sx];
                let w = weight(guide[i], p, x.abs_diff(sx));
                for c in 0..2 {
                    sum[c] += p[c + 1] * w;
                }
                total += w;
            }
            *pixel = sum.map(|v| v / total);
        }
        Ok(())
    })?;
    crate::raw_parallel::rows(rgb, width, |y, row| {
        if y % 16 == 0 {
            ensure!(!cancelled(), "已取消彩噪处理");
        }
        for (x, pixel) in row.iter_mut().enumerate() {
            let i = y * width + x;
            let mut sum = [0.; 2];
            let mut total = 0.;
            for sy in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
                let j = sy * width + x;
                let w = weight(guide[i], guide[j], y.abs_diff(sy));
                for (c, value) in sum.iter_mut().enumerate() {
                    *value += horizontal[j][c] * w;
                }
                total += w;
            }
            let lum = guide[i][0];
            let chroma: [f32; 2] = std::array::from_fn(|c| {
                guide[i][c + 1] + (sum[c] / total - guide[i][c + 1]) * strength
            });
            let r = lum + chroma[0] * lum.max(1e-6);
            let b = lum + chroma[1] * lum.max(1e-6);
            *pixel = [r, (4. * lum - r - b) * 0.5, b];
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_resolution_chroma_blocks_are_suppressed() {
        let width = 6400;
        let mut rgb: Vec<_> = (0..width * 32)
            .map(|i| {
                let n = if (i % width / 4 + i / width / 4) % 2 == 0 {
                    0.02
                } else {
                    -0.02
                };
                [0.1 + n, 0.1 - n, 0.1 + n]
            })
            .collect();
        reduce(&mut rgb, width, 25, &|| false).unwrap();
        let residual = (8..24)
            .flat_map(|y| (16..width - 16).map(move |x| y * width + x))
            .map(|i| (rgb[i][0] - rgb[i][1]).abs())
            .sum::<f32>()
            / (16 * (width - 32)) as f32;
        assert!(
            residual < 0.01,
            "full-size colour blocks remain: {residual}"
        );
    }
    #[test]
    fn respects_disabled_filter_hdr_edges_and_cancellation() {
        let source: Vec<_> = (0..32 * 8)
            .map(|i| {
                if i % 32 < 16 {
                    [2., 0.5, 0.5]
                } else {
                    [0.5, 0.5, 2.]
                }
            })
            .collect();
        let mut rgb = source.clone();
        reduce(&mut rgb, 32, 0, &|| true).unwrap();
        assert_eq!(rgb, source);
        reduce(&mut rgb, 32, 25, &|| false).unwrap();
        for (i, (a, b)) in rgb.iter().zip(source).enumerate() {
            assert!((a[0] + 2. * a[1] + a[2] - b[0] - 2. * b[1] - b[2]).abs() < 1e-5);
            assert!((a[0] - b[0]).abs() < 0.01, "colour boundary changed at {i}");
        }
        assert!(reduce(&mut rgb, 32, 25, &|| true).is_err());
    }
}
