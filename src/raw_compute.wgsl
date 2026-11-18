// Two-pass camera-RGB chroma filter. Stripe halos preserve global boundaries.
struct Params { width:u32, height:u32, output_height:u32, top:u32,
    radius:u32, scale:f32, strength:f32, pad:u32 }
@group(0) @binding(0) var<storage,read> source:array<f32>;
@group(0) @binding(1) var<storage,read_write> horizontal:array<vec2<f32>>;
@group(0) @binding(2) var<storage,read_write> output_rgb:array<f32>;
@group(0) @binding(3) var<uniform> p:Params;
fn guide(i:u32)->vec3<f32> {
    let r=source[3u*i];let g=source[3u*i+1u];let b=source[3u*i+2u];
    let lum=(r+2.*g+b)*0.25;
    return vec3<f32>(lum,(r-lum)/max(lum,0.000001),(b-lum)/max(lum,0.000001));
}
fn weight(a:vec3<f32>,b:vec3<f32>,distance:u32)->f32 {
    let dy=(a.x-b.x)/(0.002+0.15*max(a.x,b.x));
    let delta=a.yz-b.yz;
    let dc=dot(delta,delta)/0.72;
    let d=f32(distance)/p.scale;
    let lo=min(u32(d),2u);
    let knots=array<f32,4>(6.,4.,1.,0.25);
    let spatial=knots[lo]+(knots[lo+1u]-knots[lo])*(d-f32(lo));
    return spatial/(1.+dy*dy*dy*dy+dc*dc*dc*dc);
}
@compute @workgroup_size(16,8)
fn filter_x(@builtin(global_invocation_id) id:vec3<u32>) {
    if(id.x>=p.width || id.y>=p.height){return;}
    let i=id.y*p.width+id.x;let center=guide(i);
    let left=max(0,i32(id.x)-i32(p.radius));
    let right=min(i32(p.width)-1,i32(id.x)+i32(p.radius));
    var sum=vec2<f32>(0.);var total=0.;
    for(var x=left;x<=right;x++) {
        let sample=guide(id.y*p.width+u32(x));
        let w=weight(center,sample,u32(abs(i32(id.x)-x)));
        sum+=sample.yz*w;total+=w;
    }
    horizontal[i]=sum/total;
}
fn filtered(x:u32,output_y:u32)->vec3<f32> {
    let y=output_y+p.top;let i=y*p.width+x;let center=guide(i);
    let first=max(0,i32(y)-i32(p.radius));
    let last=min(i32(p.height)-1,i32(y)+i32(p.radius));
    var sum=vec2<f32>(0.);var total=0.;
    for(var sy=first;sy<=last;sy++) {
        let j=u32(sy)*p.width+x;
        let w=weight(center,guide(j),u32(abs(i32(y)-sy)));
        sum+=horizontal[j]*w;total+=w;
    }
    let chroma=center.yz+(sum/total-center.yz)*p.strength;
    let r=center.x+chroma.x*max(center.x,0.000001);
    let b=center.x+chroma.y*max(center.x,0.000001);
    return vec3<f32>(r,(4.*center.x-r-b)*0.5,b);
}
@compute @workgroup_size(16,8)
fn filter_y(@builtin(global_invocation_id) id:vec3<u32>) {
    if(id.x>=p.width || id.y>=p.output_height){return;}
    let rgb=filtered(id.x,id.y); let o=(id.y*p.width+id.x)*3u;
    output_rgb[o]=rgb.x;output_rgb[o+1u]=rgb.y;output_rgb[o+2u]=rgb.z;
}
