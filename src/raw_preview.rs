//! Local GPU tone/color controls and scene-referred floating-point previews.
//! CPU reference formulas and shared recipes; platform GPU backends release pixels after upload.

/// DNG table axes are H/S/V, with saturation the fastest varying element.
#[derive(Clone, Debug)]
pub struct HsvLookup {
    pub dims: [usize; 3],
    pub data: Vec<[f32; 3]>,
    pub encoding: u32,
}

fn valid_curve(curve: &[f32]) -> bool {
    curve.len() >= 2 && curve.len() <= 65536 && curve.iter().all(|v| v.is_finite())
}
fn valid_lookup(map: &HsvLookup) -> bool {
    lookup_dimensions_valid(map) && map.data.iter().flatten().all(|v| v.is_finite())
}
fn lookup_dimensions_valid(map: &HsvLookup) -> bool {
    map.dims.into_iter().all(|v| v > 0 && v <= 256)
        && map
            .dims
            .into_iter()
            .try_fold(1_usize, |a, b| a.checked_mul(b))
            == Some(map.data.len())
}
fn encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.max(0.).powf(1. / 2.4) - 0.055
    }
}
fn decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn rgb_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let lo = rgb.into_iter().fold(f32::INFINITY, f32::min);
    let hi = rgb.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let delta = hi - lo;
    let hue = if delta <= 1e-12 {
        0.
    } else if hi == rgb[0] {
        ((rgb[1] - rgb[2]) / delta).rem_euclid(6.)
    } else if hi == rgb[1] {
        (rgb[2] - rgb[0]) / delta + 2.
    } else {
        (rgb[0] - rgb[1]) / delta + 4.
    };
    [hue, if hi > 1e-12 { delta / hi } else { 0. }, hi]
}
fn hsv_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let [h, s, v] = hsv;
    let h = h.rem_euclid(6.);
    let c = v * s;
    let x = c * (1. - ((h % 2.) - 1.).abs());
    let a = match h.floor() as u32 {
        0 => [c, x, 0.],
        1 => [x, c, 0.],
        2 => [0., c, x],
        3 => [0., x, c],
        4 => [x, 0., c],
        _ => [c, 0., x],
    };
    a.map(|a| a + v - c)
}
fn look_up(map: &HsvLookup, rgb: [f32; 3]) -> [f32; 3] {
    if !lookup_dimensions_valid(map) {
        return rgb;
    }
    let mut hsv = rgb_hsv(rgb.map(|v| v.max(0.)));
    let encoded = map.encoding == 1 && map.dims[2] > 1;
    let value = if encoded { encode(hsv[2]) } else { hsv[2] };
    let [nh, ns, nv] = map.dims;
    let h = hsv[0] * nh as f32 / 6.;
    let s = hsv[1].clamp(0., 1.) * ns.saturating_sub(1) as f32;
    let v = value.clamp(0., 1.) * nv.saturating_sub(1) as f32;
    let base = [
        h.floor() as usize % nh,
        s.floor() as usize,
        v.floor() as usize,
    ];
    let next = [
        (base[0] + 1) % nh,
        (base[1] + 1).min(ns - 1),
        (base[2] + 1).min(nv - 1),
    ];
    let frac = [h.fract(), s.fract(), v.fract()];
    let mut result = [0.; 3];
    for dv in 0..2 {
        for dh in 0..2 {
            for ds in 0..2 {
                let index = [
                    if dh == 0 { base[0] } else { next[0] },
                    if ds == 0 { base[1] } else { next[1] },
                    if dv == 0 { base[2] } else { next[2] },
                ];
                let weight = (if dh == 0 { 1. - frac[0] } else { frac[0] })
                    * (if ds == 0 { 1. - frac[1] } else { frac[1] })
                    * (if dv == 0 { 1. - frac[2] } else { frac[2] });
                let entry = map.data[(index[2] * nh + index[0]) * ns + index[1]];
                for c in 0..3 {
                    result[c] += entry[c] * weight;
                }
            }
        }
    }
    hsv[0] = (hsv[0] + result[0] / 60.).rem_euclid(6.);
    hsv[1] = (hsv[1] * result[1]).clamp(0., 1.);
    hsv[2] = if encoded {
        decode((value * result[2]).max(0.))
    } else {
        (value * result[2]).max(0.)
    };
    hsv_rgb(hsv)
}
fn curve_value(v: f32, curve: &[f32]) -> f32 {
    // SDR clipping occurs after exposure, so a negative EV can still recover
    // scene values above one. Do not insert a second, non-profile shoulder.
    let v = v.clamp(0., 1.);
    if curve.len() < 2 {
        return v;
    }
    let x = v.clamp(0., 1.) * (curve.len() - 1) as f32;
    let i = (x as usize).min(curve.len() - 1);
    let j = (i + 1).min(curve.len() - 1);
    curve[i] + (curve[j] - curve[i]) * x.fract()
}
/// CPU reference for the GPU scene shaders, histogram and pixel validation.
/// The input is linear ProPhoto/D50; exposure is absolute, not a display delta.
pub fn evaluate_scene(rgb: [f32; 3], settings: Settings, recipe: &Recipe) -> [f32; 3] {
    let gain = 2f32.powf((settings.exposure + recipe.baseline_exposure).clamp(-20., 20.));
    let mut rgb = std::array::from_fn(|c| (rgb[c] * gain * settings.wb[c]).max(0.));
    let b = settings.basic;
    let mut y = rgb[0] * 0.2880402 + rgb[1] * 0.7118741 + rgb[2] * 0.0000857;
    if b[0] != 0. && y > 1e-9 {
        let next = 0.18 * (y / 0.18).powf(2f32.powf(b[0] * 0.8));
        rgb = rgb.map(|v| v * next / y);
        y = next;
    }
    let zone = y / (y + 0.18);
    let shift = b[2] * (1. - zone).powi(2) * 1.5
        + b[1] * zone * zone * 1.5
        + b[3] * zone.powi(4)
        + b[4] * (1. - zone).powi(4);
    rgb = rgb.map(|v| v * 2f32.powf(shift));
    if let Some(map) = &recipe.look {
        rgb = look_up(map, rgb);
    }
    if let Some(map) = &recipe.extra_look {
        rgb = look_up(map, rgb);
    }
    let lo = rgb.into_iter().fold(f32::INFINITY, f32::min);
    let hi = rgb.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let low = curve_value(lo, &recipe.curve);
    let high = curve_value(hi, &recipe.curve);
    rgb = if hi - lo > 1e-9 {
        rgb.map(|v| low + (v - lo) * (high - low) / (hi - lo))
    } else {
        [high; 3]
    };
    y = rgb[0] * 0.2880402 + rgb[1] * 0.7118741 + rgb[2] * 0.0000857;
    let saturation = if high > 1e-9 { (high - low) / high } else { 0. };
    let gain = (1. + b[5] * (1. - saturation)).max(0.) * (1. + b[6]).max(0.);
    rgb = rgb.map(|v| v + (v - y) * (gain - 1.));
    // ProPhoto/D50 -> Bradford D65 -> linear sRGB. Row sums are one.
    [
        2.034075 * rgb[0] - 0.727334 * rgb[1] - 0.306741 * rgb[2],
        -0.228813 * rgb[0] + 1.231_73 * rgb[1] - 0.002917 * rgb[2],
        -0.008570 * rgb[0] - 0.153287 * rgb[1] + 1.161857 * rgb[2],
    ]
    .map(|v| encode(v.clamp(0., 1.)).clamp(0., 1.))
}

#[cfg(not(any(windows, target_os = "macos")))]
const SCENE_FRAGMENT: &str = r#"#version 330
uniform sampler2D image; uniform sampler2D curve; uniform sampler3D look_table;
uniform float gain; uniform vec4 tone; uniform vec3 chroma; uniform vec3 wb;
uniform sampler3D extra_look_table; uniform bool extra_look_encoded; uniform bool look_encoded; in vec2 uv; out vec4 color;
float encode_s(float x) {return x<=0.0031308 ? 12.92*x : 1.055*pow(max(x,0.0),1.0/2.4)-0.055;}
float decode_s(float x) {return x<=0.04045 ? x/12.92 : pow((x+0.055)/1.055,2.4);}
vec3 rgb_hsv(vec3 x) {
    float lo=min(x.r,min(x.g,x.b)),hi=max(x.r,max(x.g,x.b)),d=hi-lo,h=0.0;
    if(d>1e-12) {if(hi==x.r)h=mod((x.g-x.b)/d,6.0);else if(hi==x.g)h=(x.b-x.r)/d+2.0;else h=(x.r-x.g)/d+4.0;}
    return vec3(h,hi>1e-12?d/hi:0.0,hi);
}
vec3 hsv_rgb(vec3 hsv) {
    float h=mod(hsv.x,6.0),c=hsv.z*hsv.y,x=c*(1.0-abs(mod(h,2.0)-1.0));vec3 a;
    if(h<1.0)a=vec3(c,x,0);else if(h<2.0)a=vec3(x,c,0);else if(h<3.0)a=vec3(0,c,x);
    else if(h<4.0)a=vec3(0,x,c);else if(h<5.0)a=vec3(x,0,c);else a=vec3(c,0,x);
    return a+vec3(hsv.z-c);
}
vec3 apply_look(vec3 rgb, sampler3D look_table, bool look_encoded) {
    vec3 hsv=rgb_hsv(max(rgb,vec3(0)));ivec3 sz=textureSize(look_table,0);
    float value=look_encoded?encode_s(hsv.z):hsv.z;
    vec3 c=vec3(hsv.x*float(sz.y)/6.0,clamp(hsv.y,0.0,1.0)*float(sz.x-1),clamp(value,0.0,1.0)*float(sz.z-1));
    ivec3 a=ivec3(floor(c)),b=ivec3((a.x+1)%sz.y,min(a.y+1,sz.x-1),min(a.z+1,sz.z-1));
    a.x=a.x%sz.y;vec3 f=fract(c),entry=vec3(0);
    for(int z=0;z<2;z++)for(int h=0;h<2;h++)for(int s=0;s<2;s++) {
        ivec3 p=ivec3(s==0?a.y:b.y,h==0?a.x:b.x,z==0?a.z:b.z);
        float w=(h==0?1.0-f.x:f.x)*(s==0?1.0-f.y:f.y)*(z==0?1.0-f.z:f.z);
        entry+=texelFetch(look_table,p,0).rgb*w;
    }
    hsv.x=mod(hsv.x+entry.x/60.0,6.0);hsv.y=clamp(hsv.y*entry.y,0.0,1.0);
    hsv.z=look_encoded?decode_s(max(value*entry.z,0.0)):max(value*entry.z,0.0);
    return hsv_rgb(hsv);
}
float curve_value(float x) {
    x=clamp(x,0.0,1.0);
    int n=textureSize(curve,0).x;float q=clamp(x,0.0,1.0)*float(n-1);int i=min(int(q),n-1),j=min(i+1,n-1);
    return mix(texelFetch(curve,ivec2(i,0),0).r,texelFetch(curve,ivec2(j,0),0).r,fract(q));
}
void main() {
    if(any(lessThan(uv,vec2(0)))||any(greaterThan(uv,vec2(1)))){color=vec4(0,0,0,1);return;}
    vec3 x=max(texture(image,uv).rgb*gain*wb,vec3(0));float y=dot(x,vec3(.2880402,.7118741,.0000857));
    if(tone.x!=0.0&&y>1e-9){float n=.18*pow(y/.18,exp2(tone.x*.8));x*=n/y;y=n;}
    float z=y/(y+.18),shift=tone.z*pow(1.0-z,2.0)*1.5+tone.y*z*z*1.5+tone.w*pow(z,4.0)+chroma.x*pow(1.0-z,4.0);
    x*=exp2(shift);x=apply_look(x,look_table,look_encoded);x=apply_look(x,extra_look_table,extra_look_encoded);
    float lo=min(x.r,min(x.g,x.b)),hi=max(x.r,max(x.g,x.b)),low=curve_value(lo),high=curve_value(hi);
    x=hi-lo>1e-9?vec3(low)+(x-vec3(lo))*(high-low)/(hi-lo):vec3(high);
    y=dot(x,vec3(.2880402,.7118741,.0000857));float sat=high>1e-9?(high-low)/high:0.0;
    float g=max(0.0,1.0+chroma.y*(1.0-sat))*max(0.0,1.0+chroma.z);x+=(x-vec3(y))*(g-1.0);
    x=vec3(dot(x,vec3(2.034075,-.727334,-.306741)),dot(x,vec3(-.228813,1.231730,-.002917)),dot(x,vec3(-.008570,-.153287,1.161857)));
    x=clamp(x,0.0,1.0);color=vec4(clamp(vec3(encode_s(x.r),encode_s(x.g),encode_s(x.b)),0.0,1.0),1);
}
"#;

#[cfg(test)]
mod scene_tests {
    use super::*;
    fn close(a: [f32; 3], b: [f32; 3]) {
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() < 0.00002, "{a:?} != {b:?}");
        }
    }
    #[test]
    fn neutral_axis_and_identity_curve_preserve_middle_gray() {
        let recipe = Recipe::default();
        let rgb = evaluate_scene([0.18; 3], Settings::default(), &recipe);
        close(rgb, [encode(0.18); 3]);
        assert!((curve_value(0.25, &[0., 0.5, 1.]) - 0.25).abs() < 1e-7);
        assert_eq!(curve_value(1.1, &[0., 1.]), 1.);
        assert_eq!(curve_value(2., &[0., 1.]), 1.);
    }
    #[test]
    fn exposure_is_applied_once_and_hdr_detail_can_be_recovered() {
        let recipe = Recipe::default();
        let settings = Settings {
            exposure: 1.,
            ..Default::default()
        };
        close(
            evaluate_scene([0.04; 3], settings, &recipe),
            evaluate_scene([0.08; 3], Settings::default(), &recipe),
        );
        let settings = Settings {
            exposure: -2.,
            ..Default::default()
        };
        let hdr = evaluate_scene([2.4, 1.2, 0.6], settings, &recipe);
        close(
            hdr,
            evaluate_scene([0.6, 0.3, 0.15], Settings::default(), &recipe),
        );
        let clipped = evaluate_scene([1., 1., 0.6], settings, &recipe);
        assert!((hdr[0] - clipped[0]).abs() > 0.1);
    }
    #[test]
    fn minus_hundred_saturation_is_achromatic_even_with_vibrance() {
        let mut settings = Settings::default();
        settings.basic[5] = 1.;
        settings.basic[6] = -1.;
        let rgb = evaluate_scene([0.45, 0.17, 0.05], settings, &Recipe::default());
        assert!((rgb[0] - rgb[1]).abs() < 0.00002);
        assert!((rgb[1] - rgb[2]).abs() < 0.00002);
    }
    #[test]
    fn lookup_wraps_hue_and_encodes_only_value_when_required() {
        let map = HsvLookup {
            dims: [4, 2, 2],
            data: (0..16)
                .map(|i| [if (i / 2) % 4 == 0 { 10. } else { -4. }, 1., 1.])
                .collect(),
            encoding: 0,
        };
        let a = look_up(&map, hsv_rgb([0.00001, 0.7, 0.3]));
        let b = look_up(&map, hsv_rgb([5.99999, 0.7, 0.3]));
        close(a, b);
        let mut map = HsvLookup {
            dims: [2, 2, 2],
            data: vec![[0., 1., 1.5]; 8],
            encoding: 1,
        };
        let encoded = look_up(&map, [0.2; 3]);
        close(encoded, [decode(encode(0.2) * 1.5); 3]);
        map.encoding = 0;
        close(look_up(&map, [0.2; 3]), [0.3; 3]);
        map.dims[2] = 1;
        map.data.truncate(4);
        map.encoding = 1;
        close(look_up(&map, [0.2; 3]), [0.3; 3]);
    }
    #[test]
    fn exposure_and_controls_remain_finite_across_working_headroom() {
        for exposure in [-5., 0., 5.] {
            for value in [-1., 0., 1.] {
                let settings = Settings {
                    exposure,
                    basic: [value; 7],
                    ..Default::default()
                };
                for input in [[0.; 3], [-0.1, 0.2, 10.], [32., 8., 2.]] {
                    let result = evaluate_scene(input, settings, &Recipe::default());
                    assert!(
                        result
                            .into_iter()
                            .all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
                    );
                }
            }
        }
    }
}
#[derive(Clone, Debug)]
pub struct Recipe {
    pub look: Option<HsvLookup>,
    pub extra_look: Option<HsvLookup>,
    pub curve: Vec<f32>,
    pub baseline_exposure: f32,
}
impl Default for Recipe {
    fn default() -> Self {
        Self {
            look: None,
            extra_look: None,
            curve: vec![0., 1.],
            baseline_exposure: 0.,
        }
    }
}
/// Scene-referred, linear ProPhoto/D50. Values above one retain sensor headroom.
pub struct LinearImage {
    pub size: [usize; 2],
    pub pixels: Vec<[f32; 4]>,
    pub recipe: Recipe,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub exposure: f32,
    pub basic: [f32; 7],
    pub wb: [f32; 3],
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            exposure: 0.,
            basic: [0.; 7],
            wb: [1.; 3],
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
#[path = "raw_preview_gl.rs"]
mod backend;
#[cfg(any(windows, target_os = "macos"))]
#[path = "raw_preview_wgpu.rs"]
mod backend;
pub use backend::{LinearTexture, Preview};
