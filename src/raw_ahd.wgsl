// AHD arithmetic adapted from LibRaw src/demosaic/ahd_demosaic.cpp.
// Copyright 2019-2025 LibRaw LLC; dcraw code copyright 1997-2018 Dave Coffin.
// This file is distributed under CDDL 1.0, one of LibRaw's offered licenses.
// License: vendor/LibRaw-0.22.2.zip, LibRaw-0.22.2/LICENSE.CDDL.
// GPU implementation copyright 2026 FastCull contributors.
struct Params {
    width:u32, height:u32, input_h:u32, origin_y:u32,
    top:u32, output_h:u32, filters:u32, pad:u32,
    xyz0:vec4<f32>, xyz1:vec4<f32>, xyz2:vec4<f32>
}
@group(0) @binding(0) var<storage,read> source:array<u32>;
@group(0) @binding(1) var<storage,read_write> scratch:array<i32>;
@group(0) @binding(2) var<storage,read_write> output_rgb:array<u32>;
@group(0) @binding(3) var<storage,read> cube_root:array<f32>;
@group(0) @binding(4) var<uniform> p:Params;
fn idx(x:i32,y:i32)->u32 {return u32(y)*p.width+u32(x);}
fn color(x:i32,y:i32)->i32 {
    let shift=((((u32(y)+p.origin_y)<<1u)&14u)|(u32(x)&1u))<<1u;
    return i32((p.filters>>shift)&3u);
}
fn sample(x:i32,y:i32,c:i32)->i32 {
    let v=source[idx(x,y)*2u+u32(c)/2u];
    return i32((v>>((u32(c)&1u)*16u))&65535u);
}
fn green(x:i32,y:i32,d:i32)->i32 {return scratch[idx(x,y)*16u+u32(d)];}
fn lab(x:i32,y:i32,d:i32)->vec3<i32> {
    let o=idx(x,y)*16u+8u+u32(d)*3u;
    return vec3<i32>(scratch[o],scratch[o+1u],scratch[o+2u]);
}
fn inside(x:i32,y:i32,b:i32)->bool {
    let gy=y+i32(p.origin_y);
    return x>=b && x<i32(p.width)-b && gy>=b && gy<i32(p.height)-b
        && y>=b && y<i32(p.input_h)-b;
}
fn short(v:f32)->i32 {return (i32(v)<<16u)>>16u;}
// Preserve the CPU's intermediate float32 rounding rather than contracting
// multiply/adds at Lab decision thresholds. frexp/ldexp is exact here.
fn rounded(v:f32)->f32 {let parts=frexp(v);return ldexp(parts.fract,parts.exp);}
@compute @workgroup_size(16,8)
fn ahd_green(@builtin(global_invocation_id) id:vec3<u32>) {
    let x=i32(id.x);let y=i32(id.y);
    if(!inside(x,y,2)){return;}
    let c=color(x,y);let i=idx(x,y)*16u;
    if(c==1){let g=sample(x,y,1);scratch[i]=g;scratch[i+1u]=g;return;}
    let a=sample(x-1,y,1);let b=sample(x+1,y,1);
    let h=((a+sample(x,y,c)+b)*2-sample(x-2,y,c)-sample(x+2,y,c))>>2u;
    scratch[i]=clamp(h,min(a,b),max(a,b));
    let a2=sample(x,y-1,1);let b2=sample(x,y+1,1);
    let v=((a2+sample(x,y,c)+b2)*2-sample(x,y-2,c)-sample(x,y+2,c))>>2u;
    scratch[i+1u]=clamp(v,min(a2,b2),max(a2,b2));
}
@compute @workgroup_size(16,8)
fn ahd_rgb_lab(@builtin(global_invocation_id) id:vec3<u32>) {
    let x=i32(id.x);let y=i32(id.y);
    if(!inside(x,y,3)){return;}
    for(var d=0;d<2;d++) {
        let g=green(x,y,d);var rgb=vec3<i32>(0,g,0);
        let own=color(x,y);
        var c=2-own;
        if(c==1) {
            c=color(x,y+1);let other=2-c;
            let h=clamp(sample(x,y,1)+((sample(x-1,y,other)+sample(x+1,y,other)-green(x-1,y,d)-green(x+1,y,d))>>1u),0,65535);
            let v=clamp(sample(x,y,1)+((sample(x,y-1,c)+sample(x,y+1,c)-green(x,y-1,d)-green(x,y+1,d))>>1u),0,65535);
            rgb=select(vec3<i32>(h,g,v),vec3<i32>(v,g,h),c==0);
        } else {
            let sum=sample(x-1,y-1,c)+sample(x+1,y-1,c)+sample(x-1,y+1,c)+sample(x+1,y+1,c)
                -green(x-1,y-1,d)-green(x+1,y-1,d)-green(x-1,y+1,d)-green(x+1,y+1,d)+1;
            let missing=clamp(g+(sum>>2u),0,65535);let known=sample(x,y,own);
            rgb=select(vec3<i32>(missing,g,known),vec3<i32>(known,g,missing),own==0);
        }
        let o=idx(x,y)*16u+2u+u32(d)*3u;
        scratch[o]=rgb.x;scratch[o+1u]=rgb.y;scratch[o+2u]=rgb.z;
        var xyz=vec3<f32>(0.5);
        xyz+=vec3<f32>(rounded(p.xyz0.x*f32(rgb.x)),rounded(p.xyz1.x*f32(rgb.x)),rounded(p.xyz2.x*f32(rgb.x)));
        xyz+=vec3<f32>(rounded(p.xyz0.y*f32(rgb.y)),rounded(p.xyz1.y*f32(rgb.y)),rounded(p.xyz2.y*f32(rgb.y)));
        xyz+=vec3<f32>(rounded(p.xyz0.z*f32(rgb.z)),rounded(p.xyz1.z*f32(rgb.z)),rounded(p.xyz2.z*f32(rgb.z)));
        xyz=vec3<f32>(cube_root[u32(clamp(i32(xyz.x),0,65535))],cube_root[u32(clamp(i32(xyz.y),0,65535))],cube_root[u32(clamp(i32(xyz.z),0,65535))]);
        let l=idx(x,y)*16u+8u+u32(d)*3u;
        scratch[l]=short(64.*(rounded(116.*xyz.y)-16.));
        scratch[l+1u]=short(64.*500.*(xyz.x-xyz.y));
        scratch[l+2u]=short(64.*200.*(xyz.y-xyz.z));
    }
}
@compute @workgroup_size(16,8)
fn ahd_homogeneity(@builtin(global_invocation_id) id:vec3<u32>) {
    let x=i32(id.x);let y=i32(id.y);
    if(!inside(x,y,4)){return;}
    let offsets=array<vec2<i32>,4>(vec2<i32>(-1,0),vec2<i32>(1,0),vec2<i32>(0,-1),vec2<i32>(0,1));
    var ld:array<u32,8>;var ab:array<u32,8>;
    for(var d=0;d<2;d++) {for(var k=0;k<4;k++) {
        let delta=lab(x,y,d)-lab(x+offsets[k].x,y+offsets[k].y,d);
        ld[d*4+k]=u32(abs(delta.x));
        ab[d*4+k]=u32(delta.y*delta.y+delta.z*delta.z);
    }}
    let leps=min(max(ld[0],ld[1]),max(ld[6],ld[7]));
    let abeps=min(max(ab[0],ab[1]),max(ab[6],ab[7]));
    for(var d=0;d<2;d++) {
        var count=0;
        for(var k=0;k<4;k++){if(ld[d*4+k]<=leps && ab[d*4+k]<=abeps){count++;}}
        scratch[idx(x,y)*16u+14u+u32(d)]=count;
    }
}
@compute @workgroup_size(16,8)
fn ahd_combine(@builtin(global_invocation_id) id:vec3<u32>) {
    if(id.x>=p.width || id.y>=p.output_h){return;}
    let x=i32(id.x);let y=i32(id.y+p.top);
    let i=idx(x,y);let o=(id.y*p.width+id.x)*2u;
    var a=source[i*2u];var b=source[i*2u+1u];
    if(inside(x,y,5)) {
        var h=vec2<i32>(0);
        for(var dy=-1;dy<=1;dy++){for(var dx=-1;dx<=1;dx++){
            let j=idx(x+dx,y+dy)*16u+14u;
            h+=vec2<i32>(scratch[j],scratch[j+1u]);
        }}
        let base=i*16u+2u;
        var rgb:array<u32,3>;
        for(var c=0u;c<3u;c++) {
            let hval=scratch[base+c];let vval=scratch[base+3u+c];
            rgb[c]=u32(select(select(hval,vval,h.y>h.x),(hval+vval)>>1u,h.x==h.y));
        }
        a=rgb[0]|(rgb[1]<<16u);b=rgb[2]|(b&0xffff0000u);
    }
    output_rgb[o]=a;output_rgb[o+1u]=b;
}
