// Independent tiled lossless JPEG (SOF3), predictor 1. One lane owns one tile.
struct Tile {
    data:u32, bytes:u32, output:u32, width:u32,
    height:u32, channels:u32, bits:u32, pad:u32,
    h0:u32,h1:u32,h2:u32,h3:u32
}
struct Params {tiles:u32,status:u32,pad0:u32,pad1:u32}
@group(0) @binding(0) var<storage,read> encoded:array<u32>;
@group(0) @binding(1) var<storage,read> huffman:array<u32>;
@group(0) @binding(2) var<storage,read> tiles:array<Tile>;
@group(0) @binding(3) var<storage,read_write> decoded:array<u32>;
@group(0) @binding(4) var<uniform> p:Params;
fn big_endian(v:u32)->u32 {
    return ((v&255u)<<24u)|((v&65280u)<<8u)|((v>>8u)&65280u)|(v>>24u);
}
fn peek(base:u32,position:u32,count:u32)->u32 {
    if(count==0u){return 0u;}
    let i=base+position/32u;let shift=position&31u;
    var word=big_endian(encoded[i]);
    if(shift!=0u){word=(word<<shift)|(big_endian(encoded[i+1u])>>(32u-shift));}
    return word>>(32u-count);
}
fn lookup(t:Tile,c:u32)->u32 {
    switch(c){case 0u:{return t.h0;}case 1u:{return t.h1;}case 2u:{return t.h2;}default:{return t.h3;}}
}
@compute @workgroup_size(1)
fn entropy(@builtin(global_invocation_id) id:vec3<u32>) {
    if(id.x>=p.tiles){return;}
    let t=tiles[id.x];let status=p.status+id.x;
    decoded[status]=1u;
    var position=0u;
    var first:array<i32,4>;var left:array<i32,4>;
    for(var c=0u;c<4u;c++){first[c]=i32(1u<<(t.bits-1u));left[c]=0;}
    for(var y=0u;y<t.height;y++) {for(var x=0u;x<t.width;x++) {for(var c=0u;c<t.channels;c++) {
        let table=lookup(t,c);
        let word=peek(t.data,position,32u);
        var entry=huffman[table+(word>>20u)];
        if((entry&0x80000000u)!=0u){entry=huffman[table+(entry&0x7fffffffu)+((word>>16u)&15u)];}
        let n=entry>>8u;let cat=entry&255u;
        if(n==0u || cat>16u){decoded[status]=2u;return;}
        position+=n;
        let extra=select(cat,0u,cat==16u);
        if(position+extra>t.bytes*8u){decoded[status]=3u;return;}
        var magnitude=0u;
        if(extra!=0u){magnitude=(word<<n)>>(32u-extra);}
        position+=extra;
        var diff=0;
        if(cat==16u){diff=-32768;}
        else if(cat!=0u){
            diff=i32(magnitude);
            if((magnitude&(1u<<(cat-1u)))==0u){diff-=i32((1u<<cat)-1u);}
        }
        var sum=left[c]+diff;
        if(x==0u){first[c]+=diff;sum=first[c];}
        let value=u32(sum)&65535u;
        if(value>=(1u<<t.bits)){decoded[status]=4u;return;}
        left[c]=i32(value);
        decoded[t.output+(y*t.width+x)*t.channels+c]=value;
    }}}
    decoded[status]=0u;
}
