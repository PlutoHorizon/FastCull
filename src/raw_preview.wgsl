// Same display formulas as raw_preview::evaluate_scene. Input stays float32
// ProPhoto/D50 until the final display transform. No sRGB texture auto-decode.
struct Params {
    bounds: vec4<f32>,
    uv01: vec4<f32>,
    uv23: vec4<f32>,
    tone: vec4<f32>,
    chroma: vec4<f32>,
    wb_gain: vec4<f32>,
    look_dims: vec4<f32>,
    display_info: vec4<f32>,
    extra_look_dims: vec4<f32>,
}
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var<storage, read> curve: array<f32>;
@group(0) @binding(2) var<storage, read> look_table: array<vec4<f32>>;
@group(0) @binding(3) var<uniform> p: Params;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> Vertex {
    let indices = array<u32, 6>(0, 1, 2, 0, 2, 3);
    let positions = array<vec2<f32>, 4>(p.bounds.xy,
        vec2<f32>(p.bounds.z, p.bounds.y), p.bounds.zw, vec2<f32>(p.bounds.x, p.bounds.w));
    let uvs = array<vec2<f32>, 4>(p.uv01.xy, p.uv01.zw, p.uv23.xy, p.uv23.zw);
    let i = indices[index];
    return Vertex(vec4<f32>(positions[i], 0, 1), uvs[i]);
}
fn encode_s(x: f32) -> f32 {
    if x <= 0.0031308 { return 12.92 * x; }
    return 1.055 * pow(max(x, 0.0), 1.0 / 2.4) - 0.055;
}
fn decode_s(x: f32) -> f32 {
    if x <= 0.04045 { return x / 12.92; }
    return pow((x + 0.055) / 1.055, 2.4);
}
fn encode_rgb(x: vec3<f32>) -> vec3<f32> { return vec3<f32>(encode_s(x.r), encode_s(x.g), encode_s(x.b)); }
fn decode_rgb(x: vec3<f32>) -> vec3<f32> { return vec3<f32>(decode_s(x.r), decode_s(x.g), decode_s(x.b)); }
fn output_color(x: vec3<f32>) -> vec4<f32> {
    // Egui normally uses an Unorm target. Compensate if the surface is sRGB.
    if p.display_info.y > 0.5 { return vec4<f32>(decode_rgb(x), 1); }
    return vec4<f32>(x, 1);
}
fn sample_image(uv: vec2<f32>) -> vec3<f32> {
    // Explicit bilinear sampling supports RGBA32Float without requesting the
    // optional FLOAT32_FILTERABLE feature. Clamp exactly at texel edges.
    let size = vec2<i32>(textureDimensions(image));
    let q = uv * vec2<f32>(size) - vec2<f32>(0.5);
    let a = vec2<i32>(floor(q));
    let f = fract(q);
    let lo = vec2<i32>(0); let hi = size - vec2<i32>(1);
    let x0 = textureLoad(image, clamp(a, lo, hi), 0).rgb;
    let x1 = textureLoad(image, clamp(a + vec2<i32>(1, 0), lo, hi), 0).rgb;
    let x2 = textureLoad(image, clamp(a + vec2<i32>(0, 1), lo, hi), 0).rgb;
    let x3 = textureLoad(image, clamp(a + vec2<i32>(1), lo, hi), 0).rgb;
    return mix(mix(x0, x1, f.x), mix(x2, x3, f.x), f.y);
}
fn positive_mod(x: f32, y: f32) -> f32 { return x - y * floor(x / y); }
fn rgb_hsv(x: vec3<f32>) -> vec3<f32> {
    let lo = min(x.r, min(x.g, x.b)); let hi = max(x.r, max(x.g, x.b));
    let d = hi - lo; var h = 0.0;
    if d > 1e-12 {
        if hi == x.r { h = positive_mod((x.g - x.b) / d, 6.0); }
        else if hi == x.g { h = (x.b - x.r) / d + 2.0; }
        else { h = (x.r - x.g) / d + 4.0; }
    }
    var s = 0.0; if hi > 1e-12 { s = d / hi; }
    return vec3<f32>(h, s, hi);
}
fn hsv_rgb(hsv: vec3<f32>) -> vec3<f32> {
    let h = positive_mod(hsv.x, 6.0); let c = hsv.z * hsv.y;
    let x = c * (1.0 - abs(positive_mod(h, 2.0) - 1.0));
    var a: vec3<f32>;
    if h < 1.0 { a = vec3<f32>(c,x,0); } else if h < 2.0 { a = vec3<f32>(x,c,0); }
    else if h < 3.0 { a = vec3<f32>(0,c,x); } else if h < 4.0 { a = vec3<f32>(0,x,c); }
    else if h < 5.0 { a = vec3<f32>(x,0,c); } else { a = vec3<f32>(c,0,x); }
    return a + vec3<f32>(hsv.z - c);
}
fn apply_look(rgb: vec3<f32>, info: vec4<f32>, offset: u32) -> vec3<f32> {
    var hsv = rgb_hsv(max(rgb, vec3<f32>(0)));
    let dims = vec3<u32>(info.xyz);
    let encoded = info.w > 0.5 && dims.z > 1u;
    var value = hsv.z; if encoded { value = encode_s(value); }
    let q = vec3<f32>(hsv.x * f32(dims.x) / 6.0,
        clamp(hsv.y,0.0,1.0) * f32(dims.y-1u), clamp(value,0.0,1.0) * f32(dims.z-1u));
    let a = vec3<u32>(floor(q));
    let b = vec3<u32>((a.x+1u)%dims.x, min(a.y+1u,dims.y-1u), min(a.z+1u,dims.z-1u));
    let f = fract(q); var entry = vec3<f32>(0);
    for (var v=0u; v<2u; v++) { for (var h=0u; h<2u; h++) { for (var s=0u; s<2u; s++) {
        let i = vec3<u32>(select(a.x%dims.x,b.x,h==1u),select(a.y,b.y,s==1u),select(a.z,b.z,v==1u));
        let w = select(1.0-f.x,f.x,h==1u)*select(1.0-f.y,f.y,s==1u)*select(1.0-f.z,f.z,v==1u);
        entry += look_table[offset+(i.z*dims.x+i.x)*dims.y+i.y].rgb * w;
    } } }
    hsv.x = positive_mod(hsv.x + entry.x / 60.0, 6.0);
    hsv.y = clamp(hsv.y * entry.y,0.0,1.0);
    if encoded { hsv.z = decode_s(max(value*entry.z,0.0)); }
    else { hsv.z = max(value*entry.z,0.0); }
    return hsv_rgb(hsv);
}
fn curve_value(input: f32) -> f32 {
    // Exposure precedes SDR clipping; the profile curve supplies its own tone.
    let x = clamp(input, 0.0, 1.0);
    let n = u32(p.display_info.x); let q = clamp(x,0.0,1.0)*f32(n-1u);
    let i = min(u32(q),n-1u); let j = min(i+1u,n-1u);
    return mix(curve[i],curve[j],fract(q));
}
@fragment fn fs_scene(in: Vertex) -> @location(0) vec4<f32> {
    if any(in.uv < vec2<f32>(0)) || any(in.uv > vec2<f32>(1)) { return vec4<f32>(0,0,0,1); }
    var x = max(sample_image(in.uv) * p.wb_gain.w * p.wb_gain.xyz, vec3<f32>(0));
    let weights = vec3<f32>(0.2880402,0.7118741,0.0000857);
    var y = dot(x,weights);
    if p.tone.x != 0.0 && y > 1e-9 { let n = 0.18*pow(y/0.18,exp2(p.tone.x*0.8)); x *= n/y; y=n; }
    let z=y/(y+0.18);
    let shift=p.tone.z*pow(1.0-z,2.0)*1.5+p.tone.y*z*z*1.5+p.tone.w*pow(z,4.0)+p.chroma.x*pow(1.0-z,4.0);
    x=apply_look(x*exp2(shift),p.look_dims,0u);
    x=apply_look(x,p.extra_look_dims,u32(p.display_info.z));
    let lo=min(x.r,min(x.g,x.b)); let hi=max(x.r,max(x.g,x.b));
    let low=curve_value(lo); let high=curve_value(hi);
    if hi-lo>1e-9 { x=vec3<f32>(low)+(x-vec3<f32>(lo))*(high-low)/(hi-lo); } else { x=vec3<f32>(high); }
    y=dot(x,weights); var sat=0.0; if high>1e-9 { sat=(high-low)/high; }
    let gain=max(0.0,1.0+p.chroma.y*(1.0-sat))*max(0.0,1.0+p.chroma.z);
    x += (x-vec3<f32>(y))*(gain-1.0);
    x=vec3<f32>(dot(x,vec3<f32>(2.034075,-0.727334,-0.306741)),
        dot(x,vec3<f32>(-0.228813,1.231730,-0.002917)),dot(x,vec3<f32>(-0.008570,-0.153287,1.161857)));
    return output_color(clamp(encode_rgb(clamp(x,vec3<f32>(0),vec3<f32>(1))),vec3<f32>(0),vec3<f32>(1)));
}
@fragment fn fs_jpeg(in: Vertex) -> @location(0) vec4<f32> {
    if any(in.uv < vec2<f32>(0)) || any(in.uv > vec2<f32>(1)) { return vec4<f32>(0,0,0,1); }
    let linear=clamp(decode_rgb(sample_image(in.uv))*p.wb_gain.w*p.wb_gain.xyz,vec3<f32>(0),vec3<f32>(1));
    var x=encode_rgb(linear); var y=dot(x,vec3<f32>(0.2126,0.7152,0.0722));
    let lift=p.tone.z*pow(1.0-y,2.0)*0.35+p.tone.y*y*y*0.35+p.tone.w*pow(y,4.0)*0.25+p.chroma.x*pow(1.0-y,4.0)*0.25;
    x=clamp((x-vec3<f32>(0.5))*(1.0+p.tone.x*0.8)+vec3<f32>(0.5+lift),vec3<f32>(0),vec3<f32>(1));
    y=dot(x,vec3<f32>(0.2126,0.7152,0.0722));
    let sat=max(x.r,max(x.g,x.b))-min(x.r,min(x.g,x.b));
    let gain=max(0.0,1.0+p.chroma.y*(1.0-sat))*max(0.0,1.0+p.chroma.z);
    return output_color(clamp(vec3<f32>(y)+(x-vec3<f32>(y))*gain,vec3<f32>(0),vec3<f32>(1)));
}
