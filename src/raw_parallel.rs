//! Bounded row workers for CPU RAW stages; rows never share mutable pixels.
use anyhow::{Result, anyhow, ensure};

/// Read-only validation of large frames without another serial memory pass.
pub fn all<T: Sync>(pixels: &[T], valid: impl Fn(&T) -> bool + Sync) -> bool {
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8);
    if workers <= 1 || pixels.len() < 262_144 {
        return pixels.iter().all(valid);
    }
    std::thread::scope(|scope| {
        let valid = &valid;
        let jobs: Vec<_> = pixels
            .chunks(pixels.len().div_ceil(workers))
            .map(|part| scope.spawn(move || part.iter().all(valid)))
            .collect();
        jobs.into_iter().all(|job| job.join().unwrap_or(false))
    })
}

pub fn rows<T: Send>(
    pixels: &mut [T],
    width: usize,
    work: impl Fn(usize, &mut [T]) -> Result<()> + Sync,
) -> Result<()> {
    ensure!(
        width > 0 && pixels.len().is_multiple_of(width),
        "RAW 行尺寸无效"
    );
    let height = pixels.len() / width;
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8)
        .min(height.div_ceil(32))
        .max(1);
    if workers == 1 || pixels.len() < 262_144 {
        for (y, row) in pixels.chunks_mut(width).enumerate() {
            work(y, row)?;
        }
        return Ok(());
    }
    let stripe = height.div_ceil(workers);
    std::thread::scope(|scope| {
        let work = &work;
        let jobs: Vec<_> = pixels
            .chunks_mut(stripe * width)
            .enumerate()
            .map(|(i, chunk)| {
                scope.spawn(move || {
                    for (y, row) in chunk.chunks_mut(width).enumerate() {
                        work(i * stripe + y, row)?;
                    }
                    Ok::<(), anyhow::Error>(())
                })
            })
            .collect();
        for job in jobs {
            job.join().map_err(|_| anyhow!("RAW 行处理线程失败"))??;
        }
        Ok(())
    })
}

/// Each worker owns complete row bands, including a partial final band.
pub fn bands<T: Send>(
    pixels: &mut [T],
    width: usize,
    band_height: usize,
    work: impl Fn(usize, &mut [T]) -> Result<()> + Sync,
) -> Result<()> {
    ensure!(
        width > 0 && band_height > 0 && pixels.len().is_multiple_of(width),
        "RAW 分块尺寸无效"
    );
    let band_len = width
        .checked_mul(band_height)
        .ok_or_else(|| anyhow!("RAW 分块尺寸溢出"))?;
    let count = pixels.len().div_ceil(band_len);
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8)
        .min(count)
        .max(1);
    let stripe = count.div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let work = &work;
        let jobs: Vec<_> = pixels
            .chunks_mut(stripe * band_len)
            .enumerate()
            .map(|(i, chunk)| {
                scope.spawn(move || {
                    for (by, band) in chunk.chunks_mut(band_len).enumerate() {
                        work((i * stripe + by) * band_height, band)?;
                    }
                    Ok::<(), anyhow::Error>(())
                })
            })
            .collect();
        for job in jobs {
            job.join().map_err(|_| anyhow!("RAW 分块线程失败"))??;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_validation_detects_invalid_values_in_every_stripe() {
        let mut pixels = vec![1_f32; 600_007];
        assert!(all(&pixels, |v| v.is_finite()));
        for i in [
            0, 75_000, 150_000, 225_000, 300_000, 375_000, 450_000, 525_000, 600_006,
        ] {
            pixels[i] = f32::NAN;
            assert!(!all(&pixels, |v| v.is_finite()));
            pixels[i] = 1.;
        }
    }
    #[test]
    fn row_stripes_keep_order_and_propagate_failures() {
        let mut pixels = vec![0_u32; 512 * 1024];
        rows(&mut pixels, 512, |y, row| {
            for (x, p) in row.iter_mut().enumerate() {
                *p = (y * 512 + x) as u32;
            }
            Ok(())
        })
        .unwrap();
        assert!(pixels.iter().enumerate().all(|(i, p)| i as u32 == *p));
        assert!(
            rows(&mut pixels, 512, |y, _| {
                ensure!(y != 777, "cancelled");
                Ok(())
            })
            .is_err()
        );
    }
    #[test]
    fn partial_bands_preserve_row_coordinates_and_errors() {
        let mut pixels = vec![0usize; 257 * 135];
        bands(&mut pixels, 257, 32, |y, band| {
            for (i, p) in band.iter_mut().enumerate() {
                *p = y * 257 + i;
            }
            Ok(())
        })
        .unwrap();
        assert!(pixels.iter().enumerate().all(|(i, p)| i == *p));
        assert!(
            bands(&mut pixels, 257, 32, |y, _| {
                ensure!(y != 128, "cancelled");
                Ok(())
            })
            .is_err()
        );
    }
}
