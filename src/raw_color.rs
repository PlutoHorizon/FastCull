//! Scene-referred local RAW development. Camera/profile processing stays in
//! floating point; exposure, Look and tone are deferred to the display recipe.
use crate::{
    dcp::{self, HsvMap, Profile, ToneCurve},
    native_raw::LinearDeveloped,
    white_balance::{self as wb, Matrix},
    xmp::{Values, WhiteBalance},
};
use anyhow::{Context, Result, ensure};
use eframe::egui::Vec2;

pub const PIPELINE_VERSION: &str = "scene-prophoto-v7-display-hsm";
pub const PROPHOTO_TO_XYZ: Matrix = [
    [0.7976749, 0.1351917, 0.0313534],
    [0.2880402, 0.7118741, 0.0000857],
    [0., 0., 0.82521],
];
const SRGB_TO_XYZ: Matrix = [
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.072175],
    [0.0193339, 0.119192, 0.9503041],
];

#[derive(Clone)]
pub struct Recipe {
    pub look: Option<HsvMap>,
    pub extra_look: Option<HsvMap>,
    pub curve: Vec<f32>,
    pub baseline_exposure: f32,
}
pub struct Frame {
    #[cfg(any(windows, target_os = "macos"))]
    pub resident: Option<crate::raw_compute::Scene>,
    pub size: [usize; 2],
    pub pixels: Vec<[f32; 4]>,
    pub recipe: Recipe,
    /// Apply per source texel before display interpolation, exposure and Look.
    /// Deferred full frames must be materialized before exporting a CPU bitmap.
    pub pending_hue_sat: Option<HsvMap>,
    pub grey: GreySampler,
    pub temperature: Option<f64>,
    pub tint: Option<f64>,
    pub profile: String,
    pub warning: Option<String>,
    pub profile_fingerprint: u64,
    pub make: String,
    pub model: String,
    pub requested_profile: String,
}
impl Frame {
    pub fn materialize_hue_sat(&mut self, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<()> {
        #[cfg(any(windows, target_os = "macos"))]
        if let Some(scene) = &self.resident {
            ensure!(!cancelled(), "已取消 GPU 场景导出");
            let pixels = scene.read_pixels()?;
            ensure!(!cancelled(), "已取消 GPU 场景导出");
            self.pixels = pixels;
            self.resident = None;
        }
        let Some(map) = &self.pending_hue_sat else {
            return Ok(());
        };
        let mut pixels = self.pixels.clone();
        crate::raw_parallel::rows(&mut pixels, self.size[0], |_, row| {
            ensure!(!cancelled(), "已取消颜色校正");
            for p in row {
                let rgb = map.apply([p[0], p[1], p[2]]);
                ensure!(
                    rgb.iter().all(|v| v.is_finite() && v.abs() < 65504.),
                    "颜色校正超出浮点范围"
                );
                *p = [rgb[0], rgb[1], rgb[2], p[3]];
            }
            Ok(())
        })?;
        ensure!(!cancelled(), "已取消颜色校正");
        self.pixels = pixels;
        self.pending_hue_sat = None;
        Ok(())
    }
}
/// Validate a display frame before handing it to the graphics API.
pub fn finite_pixels(pixels: &[[f32; 4]]) -> bool {
    crate::raw_parallel::all(pixels, |p| p.iter().all(|v| v.is_finite()))
}
/// Source-camera samples, before HSM, Look, exposure, tone or display encoding.
#[derive(Clone)]
pub struct GreySampler {
    pub size: [usize; 2],
    pub samples: Vec<[f32; 3]>,
    pub uvs: [[f32; 2]; 4],
    pub applied_mul: [f64; 3],
    pub native_matrix: Matrix,
    pub profile_color: Option<[Matrix; 2]>,
    pub profile_illuminants: [f64; 2],
    pub profile_dual: bool,
    pub calibration: [Matrix; 2],
}
impl GreySampler {
    pub fn estimate(&self, uv: [f32; 2]) -> Result<WhiteBalance> {
        ensure!(
            uv.iter().all(|x| x.is_finite() && (0. ..=1.).contains(x)),
            "取样点须位于照片内"
        );
        let left = std::array::from_fn::<_, 2, _>(|c| {
            self.uvs[0][c] + (self.uvs[3][c] - self.uvs[0][c]) * uv[1]
        });
        let right = std::array::from_fn::<_, 2, _>(|c| {
            self.uvs[1][c] + (self.uvs[2][c] - self.uvs[1][c]) * uv[1]
        });
        let p = std::array::from_fn::<_, 2, _>(|c| left[c] + (right[c] - left[c]) * uv[0]);
        let [w, h] = self.size;
        ensure!(
            w > 0 && h > 0 && self.samples.len() == w * h,
            "灰点采样格无效"
        );
        let x = ((p[0] * w as f32) as usize).min(w - 1);
        let y = ((p[1] * h as f32) as usize).min(h - 1);
        let valid = |rgb: [f32; 3]| {
            (0..3).all(|c| {
                let sensor = f64::from(rgb[c]) / self.applied_mul[c];
                sensor.is_finite() && sensor > 0.0001 && sensor < 0.9999
            })
        };
        ensure!(
            valid(self.samples[y * w + x]),
            "灰点过暗或已剪切，请重新取样"
        );
        let mut sum = [0.; 3];
        let mut count = 0;
        let mut total = 0;
        for sy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
            for sx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                let rgb = self.samples[sy * w + sx];
                total += 1;
                if valid(rgb) {
                    for c in 0..3 {
                        sum[c] += f64::from(rgb[c]) / self.applied_mul[c];
                    }
                    count += 1;
                }
            }
        }
        ensure!(
            count >= 3 && count * 2 > total,
            "灰点区域过暗或无效，请选择较均匀的中性区域"
        );
        let neutral = sum.map(|x| x / count as f64);
        let mut temperature = 6500.;
        let mut tint = 0.;
        for _ in 0..20 {
            let matrix = if let Some(color) = self.profile_color {
                let w = weight(temperature, self.profile_illuminants, self.profile_dual);
                wb::mul(
                    wb::interpolate(self.calibration[0], self.calibration[1], w),
                    wb::interpolate(color[0], color[1], w),
                )
            } else {
                self.native_matrix
            };
            let xyz = wb::apply(wb::inverse(matrix).context("白平衡矩阵不可逆")?, neutral);
            let (next, t) =
                wb::temperature_tint(xyz).context("该取样点超出白平衡范围，请改选中性灰区域")?;
            let converged = (next - temperature).abs() < 0.05;
            temperature = next;
            tint = t;
            if converged {
                break;
            }
        }
        let result = WhiteBalance::Custom {
            temperature: temperature.round() as u32,
            tint: tint.round() as i32,
        };
        ensure!(result.valid(), "灰点白平衡超出可编辑范围");
        Ok(result)
    }
}
pub fn weight(t: f64, illuminants: [f64; 2], dual: bool) -> f64 {
    let [a, b] = illuminants;
    if !dual || (1. / a - 1. / b).abs() < 1e-12 {
        return 1.;
    }
    ((1. / t - 1. / b) / (1. / a - 1. / b)).clamp(0., 1.)
}
fn camera_calibration(profile: &Profile, w: f64) -> Matrix {
    let matches = profile.camera_calibration_signature == profile.profile_calibration_signature;
    let cc = if matches {
        wb::interpolate(
            profile.calibration[0].unwrap_or(wb::IDENTITY),
            profile.calibration[1].unwrap_or(wb::IDENTITY),
            w,
        )
    } else {
        wb::IDENTITY
    };
    wb::mul(wb::diagonal(profile.analog_balance), cc)
}
pub fn find_profile(make: &str, model: &str, name: &str) -> Result<Option<Profile>> {
    dcp::find(&format!("{make} {model}"), name)
}
/// An explicit XMP profile wins; otherwise prefer the primary profile in a DNG.
/// Other RAW formats usually contain camera matrices, not a complete Adobe DCP.
pub fn find_profile_for_source(
    path: &std::path::Path,
    make: &str,
    model: &str,
    values: &Values,
) -> Result<Option<Profile>> {
    let name = requested_profile(values);
    if std::env::var_os("FASTCULL_DCP_PROFILE").is_some() {
        return find_profile(make, model, name);
    }
    let embedded = dcp::read_embedded(path);
    if let Ok(profiles) = &embedded {
        let selected = if let Some(requested) = values
            .look_name
            .as_deref()
            .or(values.camera_profile.as_deref())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            profiles.iter().find(|p| {
                p.name.eq_ignore_ascii_case(requested)
                    || requested.eq_ignore_ascii_case("Embedded")
                    || requested.eq_ignore_ascii_case("Embedded Camera Profile")
            })
        } else {
            profiles.first()
        };
        if let Some(profile) = selected {
            let mut profile = profile.clone();
            profile.requested_name = values.camera_profile.clone();
            return Ok(Some(profile));
        }
    }
    let mut profile = find_profile(make, model, name)?;
    if let (Some(profile), Err(error)) = (&mut profile, embedded) {
        profile.warning = Some(format!(
            "读取 DNG 内嵌配置失败：{error}；当前使用 {}",
            profile.name
        ));
        profile.fallback = true;
    }
    Ok(profile)
}
pub fn prepare_from_path(
    path: &std::path::Path,
    source: std::sync::Arc<LinearDeveloped>,
    values: &Values,
    max_edge: u32,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Frame> {
    let profile = find_profile_for_source(path, &source.make, &source.model, values)?;
    prepare_shared(source, values, max_edge, profile, cancelled)
}
pub fn requested_profile(values: &Values) -> &str {
    values
        .look_name
        .as_deref()
        .or(values.camera_profile.as_deref())
        .unwrap_or("Adobe Standard")
}
/// Public SDK classic SDR reference with its full 1025 samples. Modern ACR
/// process-version adjustments remain a separate, proprietary rendering stage.
pub fn default_tone() -> ToneCurve {
    ToneCurve::from_samples(&crate::adobe_tone_curve::CLASSIC_SDR)
        .expect("compiled public SDR reference")
}
fn normalized_forward(m: Matrix) -> Result<Matrix> {
    let white = wb::apply(m, [1.; 3]);
    ensure!(
        white.iter().all(|x| x.is_finite() && *x > 1e-7),
        "DCP 前向矩阵白点无效"
    );
    Ok(wb::mul(
        wb::diagonal(std::array::from_fn(|i| wb::D50[i] / white[i])),
        m,
    ))
}
fn normalized_color(m: Matrix) -> Result<Matrix> {
    let white = wb::apply(m, wb::D50);
    let maximum = white.into_iter().fold(f64::NEG_INFINITY, f64::max);
    ensure!(
        maximum.is_finite() && maximum > 1e-9,
        "DCP 色彩矩阵白点无效"
    );
    let scale = if (0.99..=1.01).contains(&maximum) {
        1.
    } else {
        1. / maximum
    };
    Ok(m.map(|row| row.map(|v| (v * scale * 10000.).round() / 10000.)))
}
pub fn prepare(
    source: LinearDeveloped,
    values: &Values,
    max_edge: u32,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Frame> {
    let name = requested_profile(values).to_string();
    let profile = find_profile(&source.make, &source.model, &name)?;
    prepare_using_profile(source, values, max_edge, profile, cancelled)
}
pub fn prepare_using_profile(
    source: LinearDeveloped,
    values: &Values,
    max_edge: u32,
    profile: Option<Profile>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Frame> {
    prepare_shared(
        std::sync::Arc::new(source),
        values,
        max_edge,
        profile,
        cancelled,
    )
}
fn prepare_shared(
    source: std::sync::Arc<LinearDeveloped>,
    values: &Values,
    max_edge: u32,
    mut profile: Option<Profile>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Frame> {
    if let Some(profile) = &mut profile {
        // Normalize the two calibrations before interpolation; a relative
        // matrix scale must not change the interpolated white point.
        profile.color = [
            normalized_color(profile.color[0])?,
            normalized_color(profile.color[1])?,
        ];
    }
    ensure!(
        source.width > 0
            && source.height > 0
            && source.width.checked_mul(source.height) == Some(source.rgb.len()),
        "线性相机图像尺寸不符"
    );
    let name = requested_profile(values).to_string();
    let native: Matrix = std::array::from_fn(|i| source.camera_matrix[i].map(f64::from));
    let applied: [f64; 3] = std::array::from_fn(|i| f64::from(source.applied_mul[i]));
    ensure!(
        applied.iter().all(|x| x.is_finite() && *x > 0.),
        "RAW 白平衡增益无效"
    );
    let explicit = values
        .white_balance
        .as_ref()
        .filter(|w| w.mode.as_deref() != Some("As Shot"))
        .and_then(|w| {
            w.temperature
                .map(|t| (f64::from(t), f64::from(w.tint.unwrap_or(0))))
        });
    let mut temperature = explicit
        .map(|v| v.0)
        .or(source.temperature)
        .unwrap_or(6500.);
    let mut tint = explicit.map(|v| v.1).or(source.tint).unwrap_or(0.);
    let mut white_known =
        explicit.is_some() || (source.temperature.is_some() && source.tint.is_some());
    // Resolve As Shot against the selected profile, retaining its camera gains.
    if explicit.is_none()
        && let Some(profile) = &profile
    {
        let neutral = applied.map(|x| 1. / x);
        for _ in 0..20 {
            let w = profile.weight(temperature);
            let cm = wb::mul(
                camera_calibration(profile, w),
                wb::interpolate(profile.color[0], profile.color[1], w),
            );
            let xyz = wb::apply(wb::inverse(cm).context("DCP 色彩矩阵不可逆")?, neutral);
            let Some((t, new_tint)) = wb::temperature_tint(xyz) else {
                break;
            };
            let done = (t - temperature).abs() < 0.05;
            temperature = t;
            tint = new_tint;
            white_known = true;
            if done {
                break;
            }
        }
    }
    let mut correction = [1.; 3];
    let mut hsm = None;
    let xyz_to_prophoto = wb::inverse(PROPHOTO_TO_XYZ).unwrap();
    let (
        matrix,
        recipe,
        profile_name,
        warning,
        fingerprint,
        profile_color,
        illuminants,
        dual,
        calibrations,
    ) = if let Some(profile) = &profile {
        let w = profile.weight(temperature);
        let cc = camera_calibration(profile, w);
        let cm = wb::mul(cc, wb::interpolate(profile.color[0], profile.color[1], w));
        let inv_cc = wb::inverse(cc).context("DCP 校准矩阵不可逆")?;
        let desired = if let Some((t, tint)) = explicit {
            let response = wb::apply(cm, wb::xyz(t, tint).context("白平衡白点无效")?);
            ensure!(
                response.iter().all(|x| x.is_finite() && *x > 1e-7),
                "DCP 白点超出相机响应范围"
            );
            let gains = response.map(|x| 1. / x);
            let gains = gains.map(|x| x / gains[1]);
            correction = std::array::from_fn(|i| gains[i] / applied[i]);
            gains
        } else {
            applied
        };
        let neutral = desired.map(|x| 1. / x);
        let to_xyz = match (profile.forward[0], profile.forward[1]) {
            (Some(a), Some(b)) => {
                let fm = wb::interpolate(normalized_forward(a)?, normalized_forward(b)?, w);
                let ref_neutral = wb::apply(inv_cc, neutral);
                ensure!(
                    ref_neutral.iter().all(|x| *x > 1e-7),
                    "DCP 参考相机白点无效"
                );
                wb::mul(
                    wb::mul(
                        wb::mul(fm, wb::diagonal(ref_neutral.map(|x| 1. / x))),
                        inv_cc,
                    ),
                    wb::diagonal(neutral),
                )
            }
            (Some(fm), None) | (None, Some(fm)) => {
                let ref_neutral = wb::apply(inv_cc, neutral);
                ensure!(
                    ref_neutral.iter().all(|x| *x > 1e-7),
                    "DCP 参考相机白点无效"
                );
                wb::mul(
                    wb::mul(
                        wb::mul(
                            normalized_forward(fm)?,
                            wb::diagonal(ref_neutral.map(|x| 1. / x)),
                        ),
                        inv_cc,
                    ),
                    wb::diagonal(neutral),
                )
            }
            _ => {
                let inverse = wb::inverse(cm).context("DCP 色彩矩阵不可逆")?;
                let white = wb::apply(inverse, neutral);
                normalized_forward(wb::mul(
                    wb::mul(
                        wb::adapt(white, wb::D50).context("DCP 白点适应失败")?,
                        inverse,
                    ),
                    wb::diagonal(neutral),
                ))?
            }
        };
        hsm = profile.hue_sat_at(temperature)?;
        let curve = profile.tone.clone().unwrap_or_else(default_tone);
        let curve_values = (0..4096)
            .map(|i| {
                let v = curve.eval(i as f32 / 4095.);
                profile
                    .enhanced_tone
                    .as_ref()
                    .map_or(v, |t| t.eval(v.powf(1. / 2.2)).powf(2.2))
            })
            .collect();
        (
            wb::mul(xyz_to_prophoto, to_xyz),
            Recipe {
                look: profile.look.clone(),
                extra_look: profile.extra_look.clone(),
                curve: curve_values,
                baseline_exposure: source.baseline_exposure as f32 + profile.baseline_exposure,
            },
            if profile.embedded {
                format!("{} · 内嵌 · {}", profile.name, source.model)
            } else {
                format!("{} · {}", profile.name, source.model)
            },
            profile.warning.clone(),
            profile.fingerprint,
            Some(profile.color),
            profile.illuminants,
            profile.dual_illuminant,
            [
                camera_calibration(profile, 1.),
                camera_calibration(profile, 0.),
            ],
        )
    } else {
        let matrix: Matrix = std::array::from_fn(|i| {
            std::array::from_fn(|j| f64::from(source.camera_to_srgb[i][j]))
        });
        let curve = default_tone();
        (
            wb::mul(
                wb::mul(
                    wb::mul(xyz_to_prophoto, wb::adapt(wb::D65, wb::D50).unwrap()),
                    SRGB_TO_XYZ,
                ),
                matrix,
            ),
            Recipe {
                look: None,
                extra_look: None,
                curve: (0..4096).map(|i| curve.eval(i as f32 / 4095.)).collect(),
                baseline_exposure: source.baseline_exposure as f32,
            },
            "相机矩阵 · 本地 SDR".to_string(),
            Some(format!(
                "未找到 {} 的 {name} 配置文件，使用相机矩阵",
                source.model
            )),
            0,
            None,
            [2850., 6500.],
            false,
            [wb::IDENTITY; 2],
        )
    };
    ensure!(
        matrix.iter().flatten().all(|x| x.is_finite()),
        "显影色彩矩阵无效"
    );
    let size = Vec2::new(source.width as f32, source.height as f32);
    let crop = values.crop.unwrap_or_default();
    let orientation = values.orientation.unwrap_or(source.orientation);
    let geometry = crop.geometry(size, orientation);
    let uvs = geometry.uvs(size, orientation);
    let sample_size = [source.width.min(128), source.height.min(128)];
    let mut samples = Vec::with_capacity(sample_size[0] * sample_size[1]);
    for y in 0..sample_size[1] {
        for x in 0..sample_size[0] {
            let sx = (2 * x + 1) * source.width / (2 * sample_size[0]);
            let sy = (2 * y + 1) * source.height / (2 * sample_size[1]);
            samples.push(source.rgb[sy * source.width + sx]);
        }
    }
    let grey = GreySampler {
        size: sample_size,
        samples,
        uvs: uvs.map(|p| [p.x, p.y]),
        applied_mul: applied,
        native_matrix: native,
        profile_color,
        profile_illuminants: illuminants,
        profile_dual: dual,
        calibration: calibrations,
    };
    let dimensions = geometry.rect.size();
    let scale = if max_edge == 0 {
        1.
    } else {
        (max_edge as f32 / dimensions.max_elem()).min(1.)
    };
    let output = [
        (dimensions.x * scale).round().max(1.) as usize,
        (dimensions.y * scale).round().max(1.) as usize,
    ];
    let count = output[0]
        .checked_mul(output[1])
        .context("线性预览尺寸溢出")?;
    ensure!(count <= 160_000_000, "线性预览超出像素限制");
    // Integrate the sensor footprint before geometric sampling. Four-point
    // point sampling aliases fine colour/noise when a RAW is reduced severalfold.
    let (mut camera_rgb, width, height) = if scale < 0.75 {
        let reduced = [
            (source.width as f32 * scale).ceil().max(1.) as usize,
            (source.height as f32 * scale).ceil().max(1.) as usize,
        ];
        let reduced_rgb = area_reduce(
            &source.rgb,
            [source.width, source.height],
            reduced,
            cancelled,
        )?;
        (std::borrow::Cow::Owned(reduced_rgb), reduced[0], reduced[1])
    } else {
        (
            std::borrow::Cow::Borrowed(source.rgb.as_slice()),
            source.width,
            source.height,
        )
    };
    let noise = values.color_noise_reduction.unwrap_or(25);
    let direct = !crop.enabled && orientation == 1 && output == [width, height];
    // Full frames are already displayed by a GPU. Avoid evaluating a nonlinear
    // camera table for every off-screen pixel; the fragment shader applies it
    // to each source texel before interpolation, preserving the processing order.
    #[cfg(any(windows, target_os = "macos"))]
    let defer = max_edge == 0
        && count >= 4_000_000
        && crate::raw_compute::display_hue_sat_enabled()
        && hsm.as_ref().is_some_and(|m| {
            m.dims.iter().all(|v| *v <= 256)
                && (m.encoding == dcp::Encoding::Linear || m.dims[2] == 1)
                && m.data.iter().all(|p| p[2] <= 4.)
        });
    #[cfg(not(any(windows, target_os = "macos")))]
    let defer = false;
    let pending_hue_sat = if defer { hsm.take() } else { None };
    let safe_scale = pending_hue_sat
        .as_ref()
        .map_or(1., |m| m.data.iter().map(|v| v[2]).fold(1_f32, f32::max));
    #[cfg(any(windows, target_os = "macos"))]
    let resident = if max_edge == 0 && count <= 60_000_000 && hsm.is_none() {
        crate::raw_compute::try_scene(
            &camera_rgb,
            width,
            noise,
            &crate::raw_compute::SceneParameters {
                output,
                uvs: uvs.map(|p| [p.x, p.y]),
                matrix,
                correction,
                limit: 65504. / safe_scale,
                direct,
            },
            cancelled,
        )?
    } else {
        None
    };
    #[cfg(any(windows, target_os = "macos"))]
    let on_gpu = resident.is_some();
    #[cfg(not(any(windows, target_os = "macos")))]
    let on_gpu = false;
    let geometry_started = std::time::Instant::now();
    let mut pixels: Vec<[f32; 4]> = Vec::new();
    if !on_gpu {
        if noise != 0 {
            #[cfg(any(windows, target_os = "macos"))]
            let accelerated = crate::raw_compute::try_reduce(&camera_rgb, width, noise, cancelled)?;
            #[cfg(not(any(windows, target_os = "macos")))]
            let accelerated: Option<Vec<[f32; 3]>> = None;
            if let Some(rgb) = accelerated {
                camera_rgb = std::borrow::Cow::Owned(rgb);
            } else {
                crate::raw_chroma::reduce(camera_rgb.to_mut(), width, noise, cancelled)?;
            }
        }
        pixels.try_reserve_exact(count)?;
        // Portrait rotation walks source columns. Visit small output tiles so those
        // source columns remain in cache; retain the exact original sampling math.
        crate::raw_parallel::bands(
            &mut pixels.spare_capacity_mut()[..count],
            output[0],
            32,
            |first_y, band| {
                ensure!(!cancelled(), "线性显影已取消");
                for first_x in (0..output[0]).step_by(32) {
                    for (by, row) in band.chunks_mut(output[0]).enumerate() {
                        let y = first_y + by;
                        let fy = (y as f32 + 0.5) / output[1] as f32;
                        let left = uvs[0].lerp(uvs[3], fy);
                        let right = uvs[1].lerp(uvs[2], fy);
                        for (x, pixel) in row.iter_mut().enumerate().skip(first_x).take(32) {
                            let rgb = if direct {
                                camera_rgb[y * width + x]
                            } else {
                                let p = left.lerp(right, (x as f32 + 0.5) / output[0] as f32);
                                let sx = (p.x * width as f32 - 0.5).clamp(0., (width - 1) as f32);
                                let sy = (p.y * height as f32 - 0.5).clamp(0., (height - 1) as f32);
                                let (x0, y0) = (sx as usize, sy as usize);
                                let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
                                let (dx, dy) = (sx - x0 as f32, sy - y0 as f32);
                                std::array::from_fn(|c| {
                                    (camera_rgb[y0 * width + x0][c] * (1. - dx)
                                        + camera_rgb[y0 * width + x1][c] * dx)
                                        * (1. - dy)
                                        + (camera_rgb[y1 * width + x0][c] * (1. - dx)
                                            + camera_rgb[y1 * width + x1][c] * dx)
                                            * dy
                                })
                            };
                            let mut out = wb::apply(
                                matrix,
                                std::array::from_fn(|i| f64::from(rgb[i]) * correction[i]),
                            )
                            .map(|x| x as f32);
                            if let Some(map) = &hsm {
                                out = map.apply(out);
                            }
                            ensure!(
                                out.iter()
                                    .all(|x| x.is_finite() && x.abs() < 65504. / safe_scale),
                                "显影结果超出浮点显示范围"
                            );
                            pixel.write([out[0], out[1], out[2], 1.]);
                        }
                    }
                }
                Ok(())
            },
        )?;
        // Every output element has been initialized above; errors leave length 0.
        unsafe {
            pixels.set_len(count);
        }
    }
    log::info!(
        "RAW color geometry_ms={:.3} orientation={orientation} pixels={count}",
        geometry_started.elapsed().as_secs_f64() * 1000.
    );
    if let Some(map) = &pending_hue_sat {
        log::info!("RAW display deferred hue-sat dims={:?}", map.dims);
    }
    Ok(Frame {
        #[cfg(any(windows, target_os = "macos"))]
        resident,
        size: output,
        pixels,
        recipe,
        pending_hue_sat,
        grey,
        temperature: white_known.then_some(temperature),
        tint: white_known.then_some(tint),
        profile: profile_name,
        warning,
        profile_fingerprint: fingerprint,
        make: source.make.clone(),
        model: source.model.clone(),
        requested_profile: name,
    })
}

fn area_reduce(
    pixels: &[[f32; 3]],
    size: [usize; 2],
    target: [usize; 2],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<[f32; 3]>> {
    ensure!(
        size[0].checked_mul(size[1]) == Some(pixels.len())
            && target.iter().all(|v| *v > 0)
            && target[0] <= size[0]
            && target[1] <= size[1],
        "浮点面积缩放尺寸无效"
    );
    let step = [
        size[0] as f64 / target[0] as f64,
        size[1] as f64 / target[1] as f64,
    ];
    let mut out = vec![[0.; 3]; target[0] * target[1]];
    crate::raw_parallel::rows(&mut out, target[0], |y, row| {
        if y % 16 == 0 {
            ensure!(!cancelled(), "线性缩放已取消");
        }
        let y0 = y as f64 * step[1];
        let y1 = (y + 1) as f64 * step[1];
        for (x, pixel) in row.iter_mut().enumerate() {
            let x0 = x as f64 * step[0];
            let x1 = (x + 1) as f64 * step[0];
            let mut sum = [0f64; 3];
            for sy in y0.floor() as usize..(y1.ceil() as usize).min(size[1]) {
                let wy = (y1.min((sy + 1) as f64) - y0.max(sy as f64)).max(0.);
                for sx in x0.floor() as usize..(x1.ceil() as usize).min(size[0]) {
                    let w = wy * (x1.min((sx + 1) as f64) - x0.max(sx as f64)).max(0.);
                    for (c, v) in sum.iter_mut().enumerate() {
                        *v += f64::from(pixels[sy * size[0] + sx][c]) * w;
                    }
                }
            }
            *pixel = sum.map(|v| (v / (step[0] * step[1])) as f32);
        }
        Ok(())
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn area_sampling_preserves_hdr_mean_and_cancels() {
        let p = vec![[0., -1., 2.], [4., 1., 4.], [4., 1., 4.], [0., -1., 2.]];
        assert_eq!(
            area_reduce(&p, [2, 2], [1, 1], &|| false).unwrap(),
            vec![[2., 0., 3.]]
        );
        assert!(area_reduce(&p, [2, 2], [1, 1], &|| true).is_err());
        let p = vec![[0., 0., 0.], [0., 0., 0.], [6., 6., 6.]];
        assert_eq!(
            area_reduce(&p, [3, 1], [2, 1], &|| false).unwrap(),
            vec![[0.; 3], [4.; 3]]
        );
    }
    #[test]
    fn color_normalization_removes_calibration_scale_before_interpolation() {
        let a = [[0.5, 0., 0.], [0., 0.5, 0.], [0., 0., 0.5]];
        assert_eq!(normalized_color(a).unwrap(), wb::IDENTITY);
        assert_eq!(normalized_color(wb::IDENTITY).unwrap(), wb::IDENTITY);
        assert!(normalized_color([[0.; 3]; 3]).is_err());
    }
    #[test]
    fn tone_landmarks_are_monotone_and_do_not_change_white_or_black() {
        let c = default_tone();
        assert_eq!(c.eval(0.), 0.);
        assert_eq!(c.eval(1.), 1.);
        assert!((c.eval(0.18) - 0.3880856).abs() < 1e-6);
        for i in 0..4095 {
            assert!(c.eval((i + 1) as f32 / 4095.) >= c.eval(i as f32 / 4095.));
        }
    }
    #[test]
    fn normalized_forward_keeps_neutral_white_and_rejects_singular_white() {
        let fm = [
            [0.4861, 0.3051, 0.1731],
            [0.2813, 0.6649, 0.0537],
            [0.1277, 0.0027, 0.6947],
        ];
        let white = wb::apply(normalized_forward(fm).unwrap(), [1.; 3]);
        assert_eq!(white, wb::D50);
        assert!(normalized_forward([[0.; 3]; 3]).is_err());
    }
    fn fixture() -> LinearDeveloped {
        LinearDeveloped {
            width: 4,
            height: 2,
            rgb: vec![[1.5, 1.5, 1.5]; 8],
            orientation: 1,
            temperature: Some(5500.),
            tint: Some(0.),
            camera_multipliers: [2., 1., 1.5, 1.],
            applied_mul: [2., 1., 1.5, 1.],
            camera_matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
            camera_to_srgb: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
            baseline_exposure: 0.,
            make: "Fixture".into(),
            model: "Fixture".into(),
            open_ms: 0.,
            unpack_ms: 0.,
            process_ms: 0.,
        }
    }
    #[test]
    fn scene_default_reduces_chroma_noise_without_blurring_luminance() {
        let mut source = fixture();
        source.width = 48;
        source.height = 32;
        source.rgb = (0..48 * 32)
            .map(|i| {
                let y = if i % 48 < 24 { 0.1 } else { 0.5 };
                let noise = if (i * 17 + i / 48 * 13) % 7 < 3 {
                    -0.02
                } else {
                    0.02
                };
                [y + noise, y - noise, y + noise]
            })
            .collect();
        let frame = prepare_using_profile(source, &Values::default(), 0, None, &|| false).unwrap();
        // Back to linear sRGB: inspect chroma on a constant-luminance patch.
        let inv = wb::inverse(wb::mul(
            wb::mul(
                wb::inverse(PROPHOTO_TO_XYZ).unwrap(),
                wb::adapt(wb::D65, wb::D50).unwrap(),
            ),
            SRGB_TO_XYZ,
        ))
        .unwrap();
        let rgb: Vec<_> = frame
            .pixels
            .iter()
            .map(|p| wb::apply(inv, [p[0] as f64, p[1] as f64, p[2] as f64]))
            .collect();
        let chroma: f64 = (4..28)
            .flat_map(|y| (4..20).map(move |x| y * 48 + x))
            .map(|i| (rgb[i][0] - rgb[i][1]).abs())
            .sum::<f64>()
            / (24. * 16.);
        assert!(chroma < 0.015, "unfiltered chroma {chroma}");
        for (i, p) in rgb.iter().enumerate() {
            let expected = if i % 48 < 24 { 0.1 } else { 0.5 };
            assert!(
                ((p[0] + 2. * p[1] + p[2]) * 0.25 - expected).abs() < 1e-5,
                "luminance edge blurred at {i}"
            );
        }
    }
    #[test]
    fn floating_geometry_keeps_headroom_and_defers_exposure() {
        let values = Values {
            exposure: Some(-2_000_000),
            orientation: Some(6),
            crop: Some(crate::crop::Crop {
                enabled: true,
                right: 500_000,
                ..Default::default()
            }),
            ..Default::default()
        };
        let frame = prepare_using_profile(fixture(), &values, 0, None, &|| false).unwrap();
        assert_eq!(frame.size, [2, 2]);
        assert!(
            frame.pixels.iter().all(|p| p[..3].iter().all(|v| *v > 1.4)),
            "camera headroom was clipped or exposure baked twice"
        );
        assert_eq!(frame.recipe.baseline_exposure, 0.);
        assert!(prepare_using_profile(fixture(), &values, 0, None, &|| true).is_err());
    }
    #[test]
    fn unknown_camera_white_balance_is_not_reported_as_a_default() {
        let mut source = fixture();
        source.temperature = None;
        source.tint = None;
        let frame = prepare_using_profile(source, &Values::default(), 0, None, &|| false).unwrap();
        assert_eq!((frame.temperature, frame.tint), (None, None));
    }
    #[test]
    fn grey_picker_uses_sensor_response_and_shared_white_model() {
        let t = 4850.;
        let tint = 17.;
        let xyz = wb::xyz(t, tint).unwrap();
        let camera_response = xyz.map(|v| v * 0.1);
        let initial = [2., 1., 1.5];
        let sampler = GreySampler {
            size: [3, 3],
            samples: vec![std::array::from_fn(|i| (camera_response[i] * initial[i]) as f32); 9],
            uvs: [[0., 0.], [1., 0.], [1., 1.], [0., 1.]],
            applied_mul: initial,
            native_matrix: wb::IDENTITY,
            profile_color: None,
            profile_illuminants: [2850., 6500.],
            profile_dual: false,
            calibration: [wb::IDENTITY; 2],
        };
        let actual = sampler.estimate([0.5, 0.5]).unwrap();
        if let WhiteBalance::Custom {
            temperature,
            tint: actual_tint,
        } = actual
        {
            assert!((temperature as f64 - t).abs() < 2.);
            assert!((actual_tint - tint as i32).abs() <= 1);
        } else {
            panic!("grey estimate is not Custom");
        }
        let mut clipped = sampler.clone();
        clipped.samples.fill([2.; 3]);
        assert!(clipped.estimate([0.5, 0.5]).is_err());
        for (channel, gain) in initial.iter().enumerate() {
            let mut clipped = sampler.clone();
            clipped.samples[4][channel] = *gain as f32;
            assert!(
                clipped.estimate([0.5, 0.5]).is_err(),
                "single-channel sensor clipping must be rejected"
            );
        }
        let mut mixed = sampler.clone();
        for index in [0, 1, 2, 3, 5] {
            mixed.samples[index][1] = 1.;
        }
        assert!(
            mixed.estimate([0.5, 0.5]).is_err(),
            "mostly clipped neighbourhood must be rejected"
        );
    }
}
