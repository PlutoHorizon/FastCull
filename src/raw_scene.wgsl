// Full precision scene geometry. NR output remains a GPU texture; no full
// camera-RGB readback or CPU ProPhoto bitmap is needed for display.
struct Geometry {
    size:vec4<u32>, // source w/h, output w/h
    uv01:vec4<f32>, uv23:vec4<f32>,
    row0:vec4<f32>, row1:vec4<f32>, row2:vec4<f32>,
    correction:vec4<f32>, limits:vec4<f32>,
}
@group(0) @binding(0) var camera:texture_2d<f32>;
@group(0) @binding(1) var scene:texture_storage_2d<rgba32float,write>;
@group(0) @binding(2) var<uniform> g:Geometry;
@group(0) @binding(3) var<storage,read_write> samples:array<vec4<f32>>;
@group(0) @binding(4) var<storage,read_write> invalid:atomic<u32>;
@group(0) @binding(5) var<storage,read> coordinates:array<f32>;
// Preserve CPU float32 operation boundaries. The runtime zero bit mask stops
// DXC contracting products/adds into FMAs, which shifts high-resolution UVs.
fn r(v:f32)->f32 {return bitcast<f32>(bitcast<u32>(v)^bitcast<u32>(g.limits.z));}
fn r2(v:vec2<f32>)->vec2<f32> {return vec2<f32>(r(v.x),r(v.y));}
fn r3(v:vec3<f32>)->vec3<f32> {return vec3<f32>(r(v.x),r(v.y),r(v.z));}
fn sample_camera(p:vec2<f32>)->vec3<f32> {
    let s=clamp(r2(r2(p*vec2<f32>(g.size.xy))-0.5),vec2<f32>(0.),vec2<f32>(g.size.xy)-1.);
    let a=vec2<u32>(s);let b=min(a+1u,g.size.xy-1u);let d=s-vec2<f32>(a);
    // Match CPU's explicit bilinear order (avoid mix's different rounding).
    let top=r3(r3(textureLoad(camera,a,0).xyz*(1.-d.x))+r3(textureLoad(camera,vec2<u32>(b.x,a.y),0).xyz*d.x));
    let bottom=r3(r3(textureLoad(camera,vec2<u32>(a.x,b.y),0).xyz*(1.-d.x))+r3(textureLoad(camera,b,0).xyz*d.x));
    return r3(r3(top*(1.-d.y))+r3(bottom*d.y));
}
@compute @workgroup_size(16,8)
fn geometry(@builtin(global_invocation_id) id:vec3<u32>) {
    if(any(id.xy>=g.size.zw)){return;}
    var rgb:vec3<f32>;
    if(g.limits.y>0.){rgb=textureLoad(camera,id.xy,0).xyz;}
    else {
        let f=vec2<f32>(coordinates[id.x],coordinates[g.size.z+id.y]);
        // Match egui/emath's (1-t)*a+t*b ordering.
        let left=r2(r2((1.-f.y)*g.uv01.xy)+r2(f.y*g.uv23.zw));
        let right=r2(r2((1.-f.y)*g.uv01.zw)+r2(f.y*g.uv23.xy));
        rgb=sample_camera(r2(r2((1.-f.x)*left)+r2(f.x*right)));
    }
    let v=rgb*g.correction.xyz;
    let out=vec3<f32>(dot(g.row0.xyz,v),dot(g.row1.xyz,v),dot(g.row2.xyz,v));
    if(any(abs(out)>=vec3<f32>(g.limits.x)) || any(out!=out)) {atomicStore(&invalid,1u);}
    textureStore(scene,id.xy,vec4<f32>(out,1.));
}
@group(1) @binding(0) var completed:texture_2d<f32>;
@compute @workgroup_size(16,8)
fn gather(@builtin(global_invocation_id) id:vec3<u32>) {
    let n=min(g.size.zw,vec2<u32>(128u));
    if(any(id.xy>=n)){return;}
    let p=(2u*id.xy+1u)*g.size.zw/(2u*n);
    samples[id.y*n.x+id.x]=textureLoad(completed,p,0);
}
