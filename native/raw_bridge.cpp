// FastCull C ABI over LibRaw. Each call owns a decoder; no shared camera state.
#define LIBRAW_NODLL
#include <libraw/libraw.h>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <ctime>
#include <exception>
#include <algorithm>
#include <cmath>
#include <chrono>
#include <mutex>
#include <memory>
#include <string>
#include <atomic>
#include <thread>
#include <cstdlib>
#include <array>
#include <vector>
#ifdef _OPENMP
#include <omp.h>
#endif
#include "white_balance.h"
#ifndef _WIN32
#include <sys/stat.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>
#endif
#ifdef _WIN32
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#endif

#ifdef _WIN32
#define FC_EXPORT extern "C" __declspec(dllexport)
using FCPathChar = wchar_t;
#else
#define FC_EXPORT extern "C" __attribute__((visibility("default")))
using FCPathChar = char; // UTF-8 on macOS, never wchar_t (32 bits on Unix).
#endif

struct FCInfo {
    uint32_t abi, width, height, orientation, preview_width, preview_height;
    float iso, aperture, shutter, focal;
    char make[64], model[64], lens[128], captured[32];
};
struct FCFrame {
    uint32_t kind, width, height, channels, bits, rendered;
    uint64_t len;
    const unsigned char *data;
    void *owner;
};
static int fail(char *error, const char *text) {
    snprintf(error, 256, "%s", text ? text : "LibRaw error");
    return -1;
}
static bool dimensions(unsigned w, unsigned h) {
    return w && h && uint64_t(w)*h <= 160000000;
}
static void configure_develop_threads() {
#ifdef _OPENMP
    unsigned count=std::min(4u,std::max(1u,std::thread::hardware_concurrency()));
    if(const char *value=std::getenv("FASTCULL_RAW_THREADS")) {
        const int requested=std::atoi(value);
        if(requested>=1 && requested<=8)count=unsigned(requested);
    }
    omp_set_dynamic(0);
    omp_set_num_threads(int(count));
#endif
}
static unsigned orientation(int flip) {
    static const unsigned map[] = {1,2,4,3,5,8,6,7};
    return flip >= 0 && flip < 8 ? map[flip] : 1;
}
struct ExifSize {unsigned width=0,height=0;};
static void size_callback(void *context,int tag,int type,int count,unsigned order,void *input,INT64) {
    if ((tag!=0xa002 && tag!=0xa003) || count!=1 || (type!=3 && type!=4)) return;
    auto *stream=static_cast<LibRaw_abstract_datastream*>(input);
    const auto saved=stream->tell();unsigned char bytes[4]={};
    const unsigned length=type==3?2:4;
    int read=stream->read(bytes,1,length);stream->seek(saved,SEEK_SET);
    if (read!=int(length)) return;
    unsigned value=0;
    if (order==0x4949) for(int i=int(length)-1;i>=0;--i) value=(value<<8)|bytes[i];
    else if(order==0x4d4d) for(unsigned i=0;i<length;++i) value=(value<<8)|bytes[i];
    else return;
    auto *size=static_cast<ExifSize*>(context);
    if(tag==0xa002)size->width=value;else size->height=value;
}
static int open_camera(LibRaw &raw,const FCPathChar *path,const void *buffer=nullptr,size_t length=0) {
    ExifSize exif;
    raw.set_exifparser_handler(size_callback,&exif);
    int code=buffer?raw.open_buffer(buffer,length):raw.open_file(path);
    raw.set_exifparser_handler(nullptr,nullptr);
    if(code)return code;
    // Camera output excludes small sensor border regions. Use an explicit
    // vendor crop when available, then the recorded EXIF output dimensions.
    raw.adjust_to_raw_inset_crop(1,0.95f);
    auto &s=raw.imgdata.sizes;
    // Older NEFs omit an output-area tag. A near-full preview can differ only
    // by the sensor border: at most 64 pixels and 1% on each axis. This never
    // treats a reduced-resolution thumbnail as the original-size image.
    const auto &thumb=raw.imgdata.thumbnail;
    if (!dimensions(exif.width,exif.height) && raw.imgdata.idata.maker_index==LIBRAW_CAMERAMAKER_Nikon &&
        dimensions(thumb.twidth,thumb.theight) && thumb.twidth<=s.width && thumb.theight<=s.height &&
        s.width-thumb.twidth<=64 && s.height-thumb.theight<=64 &&
        uint64_t(thumb.twidth)*100>=uint64_t(s.width)*99 && uint64_t(thumb.theight)*100>=uint64_t(s.height)*99) {
        exif.width=thumb.twidth;exif.height=thumb.theight;
    }
    if(dimensions(exif.width,exif.height) && exif.width<=s.width && exif.height<=s.height &&
       uint64_t(exif.width)*100>=uint64_t(s.width)*95 && uint64_t(exif.height)*100>=uint64_t(s.height)*95) {
        s.left_margin+=((s.width-exif.width)/2)&~1u;
        s.top_margin+=((s.height-exif.height)/2)&~1u;
        s.width=exif.width;s.height=exif.height;
        auto &backup=raw.imgdata.rawdata.sizes;
        backup.left_margin=s.left_margin;backup.top_margin=s.top_margin;
        backup.width=s.width;backup.height=s.height;
    }
    return 0;
}
static bool jpeg_size(const unsigned char *p, size_t n, unsigned &w, unsigned &h) {
    if (n < 4 || p[0]!=255 || p[1]!=216) return false;
    size_t i=2;
    while (i+4<=n) {
        if (p[i++]!=255) return false;
        while (i<n && p[i]==255) ++i;
        if (i>=n) return false;
        unsigned marker=p[i++];
        if (marker==218 || marker==217) return false;
        if (marker==1 || (marker>=208 && marker<=215)) continue;
        if (i+2>n) return false;
        unsigned len=(p[i]<<8)|p[i+1];
        if (len<2 || len>n-i) return false;
        if (marker>=192 && marker<=207 && marker!=196 && marker!=200 && marker!=204) {
            if (len<8) return false;
            h=(p[i+3]<<8)|p[i+4]; w=(p[i+5]<<8)|p[i+6];
            return dimensions(w,h);
        }
        i+=len;
    }
    return false;
}
FC_EXPORT int fc_raw_info(const FCPathChar *path, FCInfo *out, char *error) {
    try {
        LibRaw raw(LIBRAW_OPTIONS_NO_DATAERR_CALLBACK);
        raw.imgdata.rawparams.max_raw_memory_mb=1024;
        int code=open_camera(raw,path);
        if (code) return fail(error,libraw_strerror(code));
        const auto &d=raw.imgdata;
        if (!dimensions(d.sizes.width,d.sizes.height)) return fail(error,"RAW dimensions exceed 160 MP limit");
        memset(out,0,sizeof(*out)); out->abi=1;
        out->width=d.sizes.width; out->height=d.sizes.height;
        out->orientation=orientation(d.sizes.flip);
        out->preview_width=d.thumbnail.twidth;out->preview_height=d.thumbnail.theight;
        out->iso=d.other.iso_speed;out->aperture=d.other.aperture;
        out->shutter=d.other.shutter;out->focal=d.other.focal_len;
        snprintf(out->make,sizeof(out->make),"%s",d.idata.make);
        snprintf(out->model,sizeof(out->model),"%s",d.idata.model);
        snprintf(out->lens,sizeof(out->lens),"%s",d.lens.Lens);
        if (d.other.timestamp>0) {
            tm time={};
#ifdef _WIN32
            const bool valid_time = !localtime_s(&time,&d.other.timestamp);
#else
            const bool valid_time = localtime_r(&d.other.timestamp,&time) != nullptr;
#endif
            if (valid_time) strftime(out->captured,sizeof(out->captured),"%Y:%m:%d %H:%M:%S",&time);
        }
        return 0;
    } catch(const std::exception &e) { return fail(error,e.what()); }
      catch(...) { return fail(error,"LibRaw metadata exception"); }
}
FC_EXPORT int fc_raw_decode(const FCPathChar *path, int full, FCFrame *out, char *error) {
    try {
        memset(out,0,sizeof(*out));
        LibRaw raw(LIBRAW_OPTIONS_NO_DATAERR_CALLBACK);
        raw.imgdata.rawparams.max_raw_memory_mb=1024;
        auto &p=raw.imgdata.params;
        p.user_flip=0; p.use_camera_wb=1;p.no_auto_bright=1;p.output_color=1;p.output_bps=8;
        p.user_qual=3;p.half_size=full?0:1;
        int code=open_camera(raw,path);
        if (code) return fail(error,libraw_strerror(code));
        if (!dimensions(raw.imgdata.sizes.width,raw.imgdata.sizes.height)) return fail(error,"RAW dimensions exceed 160 MP limit");
        libraw_processed_image_t *image=nullptr;
        if (!raw.unpack_thumb()) image=raw.dcraw_make_mem_thumb(&code);
        unsigned w=image?image->width:0,h=image?image->height:0;
        if (image && image->type==LIBRAW_IMAGE_JPEG && !jpeg_size(image->data,image->data_size,w,h)) {
            LibRaw::dcraw_clear_mem(image);image=nullptr;w=h=0;
        }
        // Byte length is not a reliable proxy for resolution. Check additional
        // previews whose dimensions are larger or absent from the metadata.
        for (int i=1;i<raw.imgdata.thumbs_list.thumbcount;++i) {
            const auto &candidate=raw.imgdata.thumbs_list.thumblist[i];
            if (candidate.twidth && candidate.theight && uint64_t(candidate.twidth)*candidate.theight<=uint64_t(w)*h) continue;
            if (raw.unpack_thumb_ex(i)) continue;
            auto *next=raw.dcraw_make_mem_thumb(&code);
            if (!next) continue;
            unsigned nw=next->width,nh=next->height;
            bool valid=next->type==LIBRAW_IMAGE_JPEG?jpeg_size(next->data,next->data_size,nw,nh):dimensions(nw,nh);
            if (valid && uint64_t(nw)*nh>uint64_t(w)*h) {
                if (image) LibRaw::dcraw_clear_mem(image);
                image=next;w=nw;h=nh;
            } else LibRaw::dcraw_clear_mem(next);
        }
        if (image && ((image->type!=LIBRAW_IMAGE_JPEG && image->type!=LIBRAW_IMAGE_BITMAP) ||
            (image->type==LIBRAW_IMAGE_BITMAP && (image->bits!=8 || image->colors>4)))) {
            LibRaw::dcraw_clear_mem(image);image=nullptr;
        }
        bool rendered=false;
        if (!image || (full && (w+2<raw.imgdata.sizes.width || h+2<raw.imgdata.sizes.height))) {
            if (image) LibRaw::dcraw_clear_mem(image);
            image=nullptr;
            configure_develop_threads();
            if ((code=raw.unpack()) || (code=raw.dcraw_process())) return fail(error,libraw_strerror(code));
            image=raw.dcraw_make_mem_image(&code);rendered=true;
            if (!image) return fail(error,libraw_strerror(code));
            w=image->width;h=image->height;
        }
        if (!dimensions(w,h) || (image->type!=LIBRAW_IMAGE_JPEG && image->type!=LIBRAW_IMAGE_BITMAP)) {
            LibRaw::dcraw_clear_mem(image);return fail(error,"Unsupported preview encoding or dimensions");
        }
        out->kind=image->type==LIBRAW_IMAGE_JPEG?1:2;
        out->width=w;out->height=h;out->channels=image->colors;out->bits=image->bits;
        out->rendered=rendered?1:0;out->len=image->data_size;out->data=image->data;out->owner=image;
        return 0;
    } catch(const std::exception &e) { return fail(error,e.what()); }
      catch(...) { return fail(error,"LibRaw decode exception"); }
}
FC_EXPORT void fc_raw_release(void *image) {
    if (image) LibRaw::dcraw_clear_mem(static_cast<libraw_processed_image_t*>(image));
}

// Additive develop ABI. The existing info/decode/release culling ABI is unchanged.
using FCCancel = int (*)(void *);
struct FCDevelopParams {
    uint32_t abi, max_edge;
    double temperature, tint, exposure;
    FCCancel cancelled;
    void *context;
};
struct FCDevelopInfo {
    uint32_t abi, orientation;
    float camera_mul[4], applied_mul[4], camera_xyz[12];
    double temperature, tint, open_ms, unpack_ms, process_ms;
};
// Additive high-precision API. Returned image is linear 16-bit camera RGB;
// linear_scale restores green-normalized WB and may produce values above one.
struct FCLinearInfo {
    uint32_t abi, orientation;
    float camera_mul[4], applied_mul[4], camera_xyz[12], camera_to_srgb[12];
    double temperature, tint, baseline_exposure, linear_scale;
    double open_ms, unpack_ms, process_ms;
    char make[64], model[64];
};
static_assert(sizeof(FCLinearInfo)==320,"Linear RAW ABI layout changed");
static std::mutex develop_gate, active_guard;
static LibRaw *active_develop=nullptr;
static std::atomic<uint64_t> develop_cancel_generation{0};
struct DevelopContext {
    const FCDevelopParams *params;
    uint64_t cancel_generation=develop_cancel_generation.load(std::memory_order_acquire);
    std::chrono::steady_clock::time_point started=std::chrono::steady_clock::now();
    bool cancelled() const {
        return develop_cancel_generation.load(std::memory_order_acquire)!=cancel_generation ||
          (params->cancelled && params->cancelled(params->context)) ||
          std::chrono::steady_clock::now()-started>std::chrono::seconds(params->max_edge?30:90);
    }
};
static int develop_progress(void *ptr,enum LibRaw_progress,int,int) {
    return static_cast<DevelopContext*>(ptr)->cancelled()?1:0;
}
struct ActiveDevelop {
    ActiveDevelop(LibRaw &raw) { std::lock_guard<std::mutex> lock(active_guard); active_develop=&raw; }
    ~ActiveDevelop() { std::lock_guard<std::mutex> lock(active_guard); active_develop=nullptr; }
};
FC_EXPORT void fc_raw_develop_cancel() {
    std::lock_guard<std::mutex> lock(active_guard);
    develop_cancel_generation.fetch_add(1,std::memory_order_acq_rel);
    if(active_develop) active_develop->setCancelFlag();
}
static void temperature_uv(double temperature,double &u,double &v) {
    const double t=std::max(2000.0,std::min(50000.0,temperature));
    const double x=t<=4000 ? -0.2661239e9/(t*t*t)-0.2343580e6/(t*t)+0.8776956e3/t+0.179910 :
        -3.0258469e9/(t*t*t)+2.1070379e6/(t*t)+0.2226347e3/t+0.240390;
    const double y=t<=2222 ? -1.1063814*x*x*x-1.34811020*x*x+2.18555832*x-0.20219683 :
        (t<=4000 ? -0.9549476*x*x*x-1.37418593*x*x+2.09137015*x-0.16748867 :
         3.0817580*x*x*x-5.87338670*x*x+3.75112997*x-0.37001483);
    const double denominator=-2*x+12*y+3;
    u=4*x/denominator;v=6*y/denominator;
}
static void temperature_normal(double t,double &u,double &v) {
    double ua,va,ub,vb;
    temperature_uv(std::max(2000.0,t-1),ua,va);
    temperature_uv(std::min(50000.0,t+1),ub,vb);
    u=vb-va;v=ua-ub;
    const double n=std::sqrt(u*u+v*v);
    if(n>0){u/=n;v/=n;}
    if(v<0){u=-u;v=-v;}
}
static bool wb_multipliers(const float matrix[4][3],double t,double tint,float result[4]) {
    double u,v,nu,nv;temperature_uv(t,u,v);temperature_normal(t,nu,nv);
    // Local editor tint is defined as 1e-4 CIE 1960 uv normal displacement.
    // Adobe tint numbers use a different proprietary calibration.
    u+=nu*tint*0.0001;v+=nv*tint*0.0001;
    const double den=2*u-8*v+4;
    const double x=3*u/den,y=2*v/den;
    if(y<=0 || x<=0 || x+y>=1) return false;
    const double xyz[]={x/y,1.0,(1-x-y)/y};
    for(int c=0;c<3;++c) {
        double response=0;for(int k=0;k<3;++k)response+=matrix[c][k]*xyz[k];
        if(!std::isfinite(response)||response<=0.00001)return false;
        result[c]=float(1.0/response);
    }
    const float green=result[1];
    for(int c=0;c<3;++c)result[c]/=green;
    result[3]=result[1];return true;
}
static bool estimate_wb(const float matrix[4][3],const float mul[4],double &temperature,double &tint) {
    for(int c=0;c<3;++c)if(!std::isfinite(mul[c])||mul[c]<=0)return false;
    double a[3][4]={};
    for(int i=0;i<3;++i){for(int j=0;j<3;++j)a[i][j]=matrix[i][j];a[i][3]=1.0/mul[i];}
    for(int col=0;col<3;++col){
        int pivot=col;for(int row=col+1;row<3;++row)if(std::abs(a[row][col])>std::abs(a[pivot][col]))pivot=row;
        if(std::abs(a[pivot][col])<1e-8)return false;
        for(int k=0;k<4;++k)std::swap(a[pivot][k],a[col][k]);
        const double divisor=a[col][col];for(int k=col;k<4;++k)a[col][k]/=divisor;
        for(int row=0;row<3;++row)if(row!=col){const double n=a[row][col];for(int k=col;k<4;++k)a[row][k]-=n*a[col][k];}
    }
    const double X=a[0][3],Y=a[1][3],Z=a[2][3],den=X+15*Y+3*Z;
    if(X<=0 || Y<=0 || Z<=0 || den<=0)return false;
    const double u=4*X/den,v=6*Y/den;
    auto distance=[&](double mired){double tu,tv;temperature_uv(1e6/mired,tu,tv);return (u-tu)*(u-tu)+(v-tv)*(v-tv);};
    double lo=20,hi=500;
    for(int i=0;i<70;++i){const double a=lo+(hi-lo)/3,b=hi-(hi-lo)/3;if(distance(a)<distance(b))hi=b;else lo=a;}
    temperature=1e6/((lo+hi)/2);
    double tu,tv,nu,nv;temperature_uv(temperature,tu,tv);temperature_normal(temperature,nu,nv);
    tint=((u-tu)*nu+(v-tv)*nv)*10000;
    return std::isfinite(tint)&&std::abs(tint)<=150;
}
static void linear_exposure(void *pointer) {
    auto &raw=*static_cast<LibRaw*>(pointer);
    const double scale=raw.imgdata.params.exp_shift;
    const size_t count=size_t(raw.imgdata.sizes.width)*raw.imgdata.sizes.height;
    if(std::abs(scale-1.0)<1e-8)return;
    for(size_t i=0;i<count;++i) for(int c=0;c<4;++c)
        raw.imgdata.image[i][c]=ushort(std::min(65535.0,raw.imgdata.image[i][c]*scale));
}
struct FCBayerParams {
    uint32_t abi,width,height,filters;
    float xyz[3][4];
    const float *cbrt;
};
static_assert(sizeof(FCBayerParams)==72,"GPU Bayer ABI layout changed");
using FCBayerCompute=int(*)(void*,uint16_t*,const FCBayerParams*);
// Auditable unpack dispatch record shared with the Rust host. The bridge fills
// what it selected (decoder name, storage/active geometry, reason) and the host
// records the backend it actually adopted. Versioned by `abi`, so a host that
// passes no pointer or an older bridge simply sees no record.
struct FCEntropyDispatch {
    uint32_t abi,matched,reason,callback;
    uint32_t raw_width,raw_height,active_width,active_height;
    uint32_t filters,colors,dng_version,tiff_samples,shot_select,raw_pitch;
    char decoder[32];
};
static_assert(sizeof(FCEntropyDispatch)==88,"dispatch ABI layout changed");
// Keep the established callback ABI. Audit data travels through the separate
// fc_raw_last_unpack_dispatch export, never through a larger callback payload.
struct FCEntropyParams {uint32_t abi,kind,width,height,channels;};
static_assert(sizeof(FCEntropyParams)==20,"GPU entropy ABI layout changed");
using FCEntropyCompute=int(*)(void*,uint16_t*,const FCEntropyParams*);
// reason: 0 adopted, 1 decoder not in the supported set, 2 size limit,
// 3 no entropy callback requested, 4 unpack skipped (cached frame reused).
enum { FC_DISPATCH_ADOPTED=0, FC_DISPATCH_DECODER=1, FC_DISPATCH_SIZE=2, FC_DISPATCH_NO_CALLBACK=3, FC_DISPATCH_REUSED=4,
       FC_DISPATCH_DECLINED=5, FC_DISPATCH_CANCELLED=6, FC_DISPATCH_COMMIT_LAYOUT=7, FC_DISPATCH_NOT_CALLED=8 };
thread_local FCEntropyDispatch fc_last_dispatch{};
thread_local bool fc_last_dispatch_valid=false;
static void publish_dispatch_reason(uint32_t reason){
    FCEntropyDispatch record{};
    record.abi=2;record.reason=reason;
    fc_last_dispatch=record;fc_last_dispatch_valid=true;
}
struct ReadMapping {
    const void *data=nullptr;size_t length=0;
    ~ReadMapping(){reset();}
    void reset(){
        if(data){
#ifdef _WIN32
            UnmapViewOfFile(data);
#else
            munmap(const_cast<void*>(data),length);
#endif
        }
        data=nullptr;length=0;
    }
    void open(const FCPathChar *path,uint64_t bytes){
        reset();
        // Measured entropy decoding is CPU-bound; mapping did not consistently
        // improve all formats. Keep the proven file stream as the default.
        const char *mapping=std::getenv("FASTCULL_RAW_MMAP");
        if(bytes<1024*1024 || bytes>2ull*1024*1024*1024 || !mapping || std::strcmp(mapping,"1") || std::getenv("FASTCULL_RAW_FILE_IO"))return;
#ifdef _WIN32
        HANDLE file=CreateFileW(path,GENERIC_READ,FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE,nullptr,OPEN_EXISTING,FILE_ATTRIBUTE_NORMAL,nullptr);
        if(file==INVALID_HANDLE_VALUE)return;
        LARGE_INTEGER size={};
        if(GetFileSizeEx(file,&size) && uint64_t(size.QuadPart)==bytes){
            HANDLE mapping=CreateFileMappingW(file,nullptr,PAGE_READONLY,0,0,nullptr);
            if(mapping){data=MapViewOfFile(mapping,FILE_MAP_READ,0,0,size_t(bytes));CloseHandle(mapping);}
        }
        CloseHandle(file);
#else
        int file=::open(path,O_RDONLY);
        if(file<0)return;
        struct stat size={};
        if(!fstat(file,&size) && uint64_t(size.st_size)==bytes){
            void *mapped=mmap(nullptr,size_t(bytes),PROT_READ,MAP_PRIVATE,file,0);
            if(mapped!=MAP_FAILED)data=mapped;
        }
        close(file);
#endif
        if(data)length=size_t(bytes);
    }
};
struct DevelopRaw : LibRaw {
    ReadMapping mapping;
    FCBayerCompute compute=nullptr;
    void *compute_context=nullptr;
    void (LibRaw::*original_decoder)()=nullptr;
    FCEntropyCompute entropy_compute=nullptr;
    void *entropy_context=nullptr;
    uint32_t entropy_kind=0;
    FCEntropyDispatch *unpack_dispatch=nullptr;
    int get_decoder_info(libraw_decoder_info_t *out) override {
        if(!original_decoder)return LibRaw::get_decoder_info(out);
        auto current=load_raw;load_raw=original_decoder;
        int code=LibRaw::get_decoder_info(out);load_raw=current;return code;
    }
    void restore_decoder(){if(original_decoder)load_raw=original_decoder;original_decoder=nullptr;entropy_compute=nullptr;entropy_context=nullptr;entropy_kind=0;}
    void fill_dispatch(FCEntropyDispatch &record){
        const char *name=unpack_function_name();
        std::snprintf(record.decoder,sizeof(record.decoder),"%s",name?name:"(unknown)");
        record.raw_width=imgdata.sizes.raw_width;record.raw_height=imgdata.sizes.raw_height;
        record.active_width=imgdata.sizes.width;record.active_height=imgdata.sizes.height;
        record.raw_pitch=imgdata.sizes.raw_pitch;
        record.filters=imgdata.idata.filters;record.colors=imgdata.idata.colors;
        record.dng_version=imgdata.idata.dng_version;
        record.tiff_samples=libraw_internal_data.unpacker_data.tiff_samples;
        record.shot_select=imgdata.rawparams.shot_select;
    }
    int unpack_compute(FCEntropyCompute fn,void *ctx){
        FCEntropyDispatch record{};
        record.abi=2;
        fill_dispatch(record);
        struct Publish{
            DevelopRaw &raw;
            FCEntropyDispatch &record;
            ~Publish(){raw.unpack_dispatch=nullptr;record.abi=2;fc_last_dispatch=record;fc_last_dispatch_valid=true;}
        } publish{*this,record};
        unpack_dispatch=&record;
        if(!fn){record.reason=FC_DISPATCH_NO_CALLBACK;return LibRaw::unpack();}
        const char *name=unpack_function_name();
        if(!std::strcmp(name,"sony_ljpeg_load_raw()") && imgdata.idata.colors==3 && imgdata.idata.filters &&
            !(imgdata.sizes.raw_width&1) && !(imgdata.sizes.raw_height&1))
            entropy_kind=1;
        else if(!std::strcmp(name,"lossless_dng_load_raw()") && imgdata.idata.dng_version>=0x1010000 && !imgdata.idata.filters && imgdata.idata.colors==3 &&
            libraw_internal_data.unpacker_data.tiff_samples==3 && imgdata.rawparams.shot_select==0 &&
            (libraw_internal_data.unpacker_data.dng_frames[0]&255)==0)
            entropy_kind=2;
        if(!entropy_kind){record.reason=FC_DISPATCH_DECODER;return LibRaw::unpack();}
        if(uint64_t(imgdata.sizes.raw_width)*imgdata.sizes.raw_height*(entropy_kind==1?1:3)>160000000){
            record.reason=FC_DISPATCH_SIZE;return LibRaw::unpack();
        }
        record.matched=entropy_kind;record.reason=FC_DISPATCH_NOT_CALLED;
        original_decoder=load_raw;entropy_compute=fn;entropy_context=ctx;
        // Standard derived-to-base member-pointer conversion; the dynamic object
        // is always DevelopRaw. Decoder identification continues to see upstream.
        load_raw=static_cast<void(LibRaw::*)()>(&DevelopRaw::gpu_unpack_tiles);
        struct Restore {DevelopRaw &raw;~Restore(){raw.restore_decoder();}} restore{*this};
        int code=LibRaw::unpack();
        if(!code)fill_dispatch(record); // LibRaw may clear geometry on cancellation.
        return code;
    }
    void gpu_unpack_tiles(){
        FCEntropyParams params={1,entropy_kind,imgdata.sizes.raw_width,imgdata.sizes.raw_height,entropy_kind==1?1u:3u};
        const size_t count=size_t(params.width)*params.height*params.channels;
        std::vector<uint16_t> pixels(count);
        auto &dispatch=*unpack_dispatch;
        dispatch.callback=1;
        int status=entropy_compute(entropy_context,pixels.data(),&params);
        if(status<0){dispatch.reason=FC_DISPATCH_CANCELLED;throw LIBRAW_EXCEPTION_CANCELLED_BY_CALLBACK;}
        if(status==0){dispatch.reason=FC_DISPATCH_DECLINED;(this->*original_decoder)();return;}
        if(entropy_kind==1 && imgdata.rawdata.raw_image && imgdata.sizes.raw_pitch>=params.width*2){
            for(unsigned row=0;row<params.height;row++)
                std::memcpy(imgdata.rawdata.raw_image+size_t(row)*(imgdata.sizes.raw_pitch/2),pixels.data()+size_t(row)*params.width,size_t(params.width)*2);
        }else if(entropy_kind==2 && imgdata.image && !imgdata.rawdata.raw_image){
            // The same curve mapping as adobe_copy_pixel for a linear 3-channel
            // DNG; keep LibRaw's allocation, black-level and later color phases.
#ifdef _OPENMP
#pragma omp parallel for
#endif
            for(int row=0;row<int(params.height);row++)for(unsigned col=0;col<params.width;col++){
                const size_t i=size_t(row)*params.width+col;
                for(unsigned c=0;c<3;c++)imgdata.image[i][c]=imgdata.color.curve[pixels[i*3+c]];
            }
        }else{dispatch.reason=FC_DISPATCH_COMMIT_LAYOUT;(this->*original_decoder)();return;}
        dispatch.reason=FC_DISPATCH_CANCELLED;
        checkCancel();
        dispatch.reason=FC_DISPATCH_ADOPTED;
    }
    void set_compute(FCBayerCompute fn,void *ctx) {
        compute=fn;compute_context=ctx;
        callbacks.interpolate_bayer_cb=nullptr;
    }
    static void prepare_interpolate(void *pointer) {
        linear_exposure(pointer);
        auto &raw=*static_cast<DevelopRaw*>(pointer);
        auto &d=raw.imgdata;
        bool supported=raw.compute && d.params.user_qual==3 && d.idata.filters>1000 && d.idata.colors==3;
        unsigned counts[3]={};
        if(supported)for(int y=0;y<2;y++)for(int x=0;x<2;x++) {
            int c=raw.COLOR(y,x);
            if(c>2){supported=false;continue;}counts[c]++;
            if(c==raw.COLOR(y+1,x)||c==raw.COLOR(y,x+1))supported=false;
        }
        supported=supported&&counts[0]==1&&counts[1]==2&&counts[2]==1;
        if(supported)for(int y=0;y<8;y++)for(int x=0;x<8;x++)
            if(raw.COLOR(y,x)!=raw.COLOR(y%2,x%2))supported=false;
        raw.callbacks.interpolate_bayer_cb=supported?gpu_ahd:nullptr;
    }
    static void gpu_ahd(void *pointer) {
        auto &raw=*static_cast<DevelopRaw*>(pointer);
        auto &d=raw.imgdata;
        bool supported=d.idata.colors==3 && d.sizes.width>=16 && d.sizes.height>=16;
        for(unsigned y=0;y<8 && supported;y++)for(unsigned x=0;x<8;x++)
            if(raw.COLOR(y,x)>2 || raw.COLOR(y,x)!=raw.COLOR(y%2,x%2)) supported=false;
        if(supported && raw.compute) {
            raw.border_interpolate(5);
            FCBayerParams params={1,d.sizes.width,d.sizes.height,d.idata.filters,{}};
            for(int i=0;i<3;i++)for(int j=0;j<3;j++)for(int k=0;k<3;k++)
                params.xyz[i][j]+=float(LibRaw_constants::xyz_rgb[i][k]*d.color.rgb_cam[k][j]/LibRaw_constants::d65_white[i]);
            static const std::array<float,65536> table=[]{
                std::array<float,65536> result={};
                for(unsigned i=0;i<65536;i++){const float r=i/65535.0f;result[i]=r>0.008856f?std::pow(r,1.f/3.f):7.787f*r+16.f/116.f;}
                return result;
            }();
            params.cbrt=table.data();
            const int status=raw.compute(raw.compute_context,&d.image[0][0],&params);
            if(status>0)return;
            if(status<0)throw LIBRAW_EXCEPTION_CANCELLED_BY_CALLBACK;
        }
        raw.ahd_interpolate();
    }
    DevelopRaw() : LibRaw(LIBRAW_OPTIONS_NO_DATAERR_CALLBACK) { callbacks.pre_interpolate_cb=prepare_interpolate; }
    ~DevelopRaw(){recycle_datastream();mapping.reset();}
};
struct DevelopCache {
    std::unique_ptr<DevelopRaw> raw;
    std::basic_string<FCPathChar> path;
    uint64_t bytes=0,stamp=0;
    unsigned width=0,height=0;
    FCDevelopInfo basis={};
};
static DevelopCache develop_cache;
static DevelopCache linear_cache;
// Called only after the editor's worker has stopped. Keep the module/device
// alive while releasing mapped files and unpacked sensor planes between photos.
FC_EXPORT void fc_raw_develop_idle() {
    std::lock_guard<std::mutex> gate(develop_gate);
    develop_cache=DevelopCache{};
    linear_cache=DevelopCache{};
}
static bool file_stamp(const FCPathChar *path,uint64_t &bytes,uint64_t &stamp) {
#ifdef _WIN32
    WIN32_FILE_ATTRIBUTE_DATA data={};
    if(!GetFileAttributesExW(path,GetFileExInfoStandard,&data))return false;
    bytes=(uint64_t(data.nFileSizeHigh)<<32)|data.nFileSizeLow;
    stamp=(uint64_t(data.ftLastWriteTime.dwHighDateTime)<<32)|data.ftLastWriteTime.dwLowDateTime;
#else
    struct stat data={};if(stat(path,&data))return false;
    bytes=data.st_size;stamp=uint64_t(data.st_mtimespec.tv_sec)*1000000000ull+data.st_mtimespec.tv_nsec;
#endif
    return true;
}
struct CacheAttempt {
    DevelopCache &cache;
    bool success=false;
    ~CacheAttempt(){
        if(cache.raw){cache.raw->set_compute(nullptr,nullptr);cache.raw->set_progress_handler(nullptr,nullptr);cache.raw->free_image();}
        if(!success)cache.raw.reset();
    }
};
FC_EXPORT int fc_raw_develop(const FCPathChar *path,const FCDevelopParams *params,FCFrame *out,FCDevelopInfo *info,char *error) {
    memset(out,0,sizeof(*out));memset(info,0,sizeof(*info));
    if(!params || params->abi!=1 || !std::isfinite(params->exposure) || std::abs(params->exposure)>5 ||
       !std::isfinite(params->temperature)||!std::isfinite(params->tint)||std::abs(params->tint)>150 ||
       (params->temperature!=0 && (params->temperature<2000 || params->temperature>50000))) return fail(error,"Invalid RAW develop parameters");
    std::unique_lock<std::mutex> gate(develop_gate,std::try_to_lock);
    if(!gate.owns_lock())return fail(error,"Another local RAW develop is active");
    DevelopContext context{params};
#ifdef _WIN32
    struct NamedGate {HANDLE handle=nullptr;bool locked=false;~NamedGate(){if(locked)ReleaseMutex(handle);if(handle)CloseHandle(handle);}} host;
    host.handle=CreateMutexW(nullptr,FALSE,L"Local\\FastCull.LocalRawDevelop.v1");
    if(!host.handle)return fail(error,"Cannot create RAW develop mutex");
    while(!host.locked){
        const DWORD status=WaitForSingleObject(host.handle,50);
        if(status==WAIT_OBJECT_0 || status==WAIT_ABANDONED)host.locked=true;
        else if(status!=WAIT_TIMEOUT)return fail(error,"Cannot acquire RAW develop mutex");
        if(context.cancelled())return fail(error,"RAW develop cancelled");
        if(!host.locked && std::chrono::steady_clock::now()-context.started>std::chrono::seconds(3))return fail(error,"Another editor is developing RAW; retry shortly");
    }
#endif
    try {
        uint64_t bytes,stamp;
        if(!file_stamp(path,bytes,stamp))return fail(error,"Cannot read RAW file stamp");
        const bool reused=develop_cache.raw && develop_cache.path==path && develop_cache.bytes==bytes && develop_cache.stamp==stamp;
        if(!reused){develop_cache.raw.reset(new DevelopRaw);develop_cache.path=path;develop_cache.bytes=bytes;develop_cache.stamp=stamp;}
        CacheAttempt attempt{develop_cache};
        auto &raw=*develop_cache.raw;ActiveDevelop active(raw);
        configure_develop_threads();
        raw.clearCancelFlag();
        raw.set_progress_handler(develop_progress,&context);
        raw.imgdata.rawparams.max_raw_memory_mb=1024;
        auto &p=raw.imgdata.params;
        p.user_flip=0;p.use_camera_wb=1;p.use_camera_matrix=3;p.no_auto_bright=1;
        p.output_color=1;p.output_bps=8;p.gamm[0]=1.0/2.4;p.gamm[1]=12.92;
        p.user_qual=params->max_edge?0:3;p.exp_correc=0;p.exp_shift=float(std::exp2(params->exposure));
        std::memset(p.user_mul,0,sizeof(p.user_mul));
        auto started=std::chrono::steady_clock::now();
        int code=0;
        if(!reused){
            code=open_camera(raw,path);
            if(code)return fail(error,libraw_strerror(code));
            if(!dimensions(raw.imgdata.sizes.width,raw.imgdata.sizes.height))return fail(error,"RAW dimensions exceed 160 MP limit");
            develop_cache.basis={};develop_cache.basis.abi=1;
            develop_cache.basis.orientation=orientation(raw.imgdata.sizes.flip);
            std::memcpy(develop_cache.basis.camera_mul,raw.imgdata.color.cam_mul,sizeof(info->camera_mul));
            std::memcpy(develop_cache.basis.camera_xyz,raw.imgdata.color.cam_xyz,sizeof(info->camera_xyz));
            develop_cache.width=raw.imgdata.sizes.width;develop_cache.height=raw.imgdata.sizes.height;
        }
        *info=develop_cache.basis;
        const auto matrix=reinterpret_cast<const float(*)[3]>(info->camera_xyz);
        p.half_size=params->max_edge && std::max(develop_cache.width,develop_cache.height)/2>=params->max_edge;
        if(params->temperature>0){
            if(!wb_multipliers(matrix,params->temperature,params->tint,p.user_mul))return fail(error,"Camera color matrix unavailable for custom white balance");
            p.use_camera_wb=0;info->temperature=params->temperature;info->tint=params->tint;
        }else if(!estimate_wb(matrix,info->camera_mul,info->temperature,info->tint)){
            info->temperature=0;info->tint=0;
        }
        std::memcpy(info->applied_mul,params->temperature>0?p.user_mul:info->camera_mul,sizeof(info->applied_mul));
        info->open_ms=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-started).count();
        if(context.cancelled())return fail(error,"RAW develop cancelled");
        if(!reused){
            started=std::chrono::steady_clock::now();code=raw.unpack();
            if(code)return fail(error,libraw_strerror(code));
            info->unpack_ms=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-started).count();
            // No decoder/file handle is retained while the user saves metadata.
            // Re-processing uses only the cached unpacked sensor planes.
            raw.recycle_datastream();
        }
        if(context.cancelled())return fail(error,"RAW develop cancelled");
        started=std::chrono::steady_clock::now();code=raw.dcraw_process();
        if(code)return fail(error,libraw_strerror(code));
        auto *image=raw.dcraw_make_mem_image(&code);
        if(!image)return fail(error,libraw_strerror(code));
        if(image->type!=LIBRAW_IMAGE_BITMAP || image->bits!=8 || image->colors!=3 || !dimensions(image->width,image->height)){
            LibRaw::dcraw_clear_mem(image);return fail(error,"Unsupported developed RAW bitmap");
        }
        out->kind=2;out->width=image->width;out->height=image->height;out->channels=image->colors;out->bits=image->bits;
        out->rendered=1;out->len=image->data_size;out->data=image->data;out->owner=image;
        info->process_ms=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-started).count();
        // free_image() discards only the developed image; LibRaw's documented
        // repeat-processing flow restores sizes/color from rawdata next time.
        // Keep one unpacked photo, replace it on path/size/mtime changes.
        attempt.success=true;
        return 0;
    }catch(const std::exception &e){return fail(error,e.what());}catch(...){return fail(error,"LibRaw develop exception");}
}

static int develop_linear(const FCPathChar *path,const FCDevelopParams *params,FCFrame *out,FCLinearInfo *info,char *error,FCBayerCompute compute,void *compute_context,FCEntropyCompute entropy=nullptr,void *entropy_context=nullptr) {
    fc_last_dispatch_valid=false;
    if(!out||!info||!error||!path)return -1;
    std::memset(out,0,sizeof(*out));std::memset(info,0,sizeof(*info));
    if(!params||params->abi!=1||!std::isfinite(params->exposure)||std::abs(params->exposure)>5||
       !std::isfinite(params->temperature)||!std::isfinite(params->tint)||std::abs(params->tint)>150||
       (params->temperature!=0&&(params->temperature<2000||params->temperature>50000)))return fail(error,"Invalid linear RAW develop parameters");
    std::unique_lock<std::mutex> gate(develop_gate,std::try_to_lock);
    if(!gate.owns_lock())return fail(error,"Another local RAW develop is active");
    DevelopContext context{params};
#ifdef _WIN32
    struct NamedGate {HANDLE handle=nullptr;bool locked=false;~NamedGate(){if(locked)ReleaseMutex(handle);if(handle)CloseHandle(handle);}} host;
    host.handle=CreateMutexW(nullptr,FALSE,L"Local\\FastCull.LocalRawDevelop.v1");
    if(!host.handle)return fail(error,"Cannot create RAW develop mutex");
    while(!host.locked){
        const DWORD status=WaitForSingleObject(host.handle,50);
        if(status==WAIT_OBJECT_0||status==WAIT_ABANDONED)host.locked=true;
        else if(status!=WAIT_TIMEOUT)return fail(error,"Cannot acquire RAW develop mutex");
        if(context.cancelled())return fail(error,"Linear RAW develop cancelled");
        if(!host.locked&&std::chrono::steady_clock::now()-context.started>std::chrono::seconds(3))return fail(error,"Another editor is developing RAW; retry shortly");
    }
#endif
    try {
        uint64_t bytes,stamp;
        if(!file_stamp(path,bytes,stamp))return fail(error,"Cannot read RAW file stamp");
        const bool reused=linear_cache.raw&&linear_cache.path==path&&linear_cache.bytes==bytes&&linear_cache.stamp==stamp;
        if(!reused){linear_cache.raw.reset(new DevelopRaw);linear_cache.path=path;linear_cache.bytes=bytes;linear_cache.stamp=stamp;}
        CacheAttempt attempt{linear_cache};auto &raw=*linear_cache.raw;ActiveDevelop active(raw);
        raw.set_compute(params->max_edge?nullptr:compute,compute_context);
        configure_develop_threads();
        raw.clearCancelFlag();raw.set_progress_handler(develop_progress,&context);
        raw.imgdata.rawparams.max_raw_memory_mb=1024;
        auto &p=raw.imgdata.params;
        p.user_flip=0;p.use_camera_wb=1;p.use_auto_wb=0;p.use_camera_matrix=3;p.no_auto_bright=1;
        p.output_color=0;p.output_bps=16;p.gamm[0]=1.;p.gamm[1]=1.;
        p.user_qual=params->max_edge?0:3;p.exp_correc=0;p.exp_shift=1.;p.bright=1.;
        // Normalize the largest WB multiplier, instead of clipping a multiplied
        // red/blue channel at sensor white. The scale is restored in float.
        p.highlight=1;std::memset(p.user_mul,0,sizeof(p.user_mul));
        auto started=std::chrono::steady_clock::now();int code=0;
        if(!reused){
            raw.mapping.open(path,bytes);
            code=open_camera(raw,path,raw.mapping.data,raw.mapping.length);if(code)return fail(error,libraw_strerror(code));
            if(!dimensions(raw.imgdata.sizes.width,raw.imgdata.sizes.height))return fail(error,"RAW dimensions exceed 160 MP limit");
            linear_cache.basis={};linear_cache.basis.abi=1;
            linear_cache.basis.orientation=orientation(raw.imgdata.sizes.flip);
            std::memcpy(linear_cache.basis.camera_mul,raw.imgdata.color.cam_mul,sizeof(info->camera_mul));
            std::memcpy(linear_cache.basis.camera_xyz,raw.imgdata.color.cam_xyz,sizeof(info->camera_xyz));
            linear_cache.width=raw.imgdata.sizes.width;linear_cache.height=raw.imgdata.sizes.height;
        }
        info->abi=1;info->orientation=linear_cache.basis.orientation;
        std::memcpy(info->camera_mul,linear_cache.basis.camera_mul,sizeof(info->camera_mul));
        std::memcpy(info->camera_xyz,linear_cache.basis.camera_xyz,sizeof(info->camera_xyz));
        const auto matrix=reinterpret_cast<const float(*)[3]>(info->camera_xyz);
        p.half_size=params->max_edge&&std::max(linear_cache.width,linear_cache.height)/2>=params->max_edge;
        if(params->temperature>0){
            if(!fc_wb::multipliers(matrix,params->temperature,params->tint,p.user_mul))return fail(error,"Camera color matrix unavailable for linear white balance");
            p.use_camera_wb=0;info->temperature=params->temperature;info->tint=params->tint;
        }
        info->open_ms=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-started).count();
        if(context.cancelled())return fail(error,"Linear RAW develop cancelled");
        if(!reused){
            started=std::chrono::steady_clock::now();code=raw.unpack_compute(entropy,entropy_context);
            if(code)return fail(error,libraw_strerror(code));
            info->unpack_ms=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-started).count();
            raw.recycle_datastream();
            raw.mapping.reset();
        }else{
            publish_dispatch_reason(FC_DISPATCH_REUSED);
        }
        if(context.cancelled())return fail(error,"Linear RAW develop cancelled");
        started=std::chrono::steady_clock::now();code=raw.dcraw_process();
        if(code)return fail(error,libraw_strerror(code));
        if(context.cancelled())return fail(error,"Linear RAW develop cancelled");
        const auto &color=raw.imgdata.color;
        const double green=color.pre_mul[1];
        if(!std::isfinite(green)||green<=0.000001)return fail(error,"Invalid linear RAW white balance scale");
        info->linear_scale=1./green;
        for(unsigned c=0;c<4;++c){
            if(!std::isfinite(color.pre_mul[c])||color.pre_mul[c]<0)return fail(error,"Invalid linear RAW white balance");
            info->applied_mul[c]=float(color.pre_mul[c]/green);
        }
        std::memcpy(info->camera_to_srgb,color.rgb_cam,sizeof(info->camera_to_srgb));
        if(params->temperature==0&&!fc_wb::estimate(matrix,info->applied_mul,info->temperature,info->tint)){info->temperature=0;info->tint=0;}
        const double baseline=color.dng_levels.baseline_exposure;
        info->baseline_exposure=std::isfinite(baseline)&&std::abs(baseline)<=16?baseline:0.;
        std::snprintf(info->make,sizeof(info->make),"%s",raw.imgdata.idata.make);
        std::snprintf(info->model,sizeof(info->model),"%s",raw.imgdata.idata.model);
        auto *image=raw.dcraw_make_mem_image(&code);
        if(!image)return fail(error,libraw_strerror(code));
        if(context.cancelled()){
            LibRaw::dcraw_clear_mem(image);return fail(error,"Linear RAW develop cancelled");
        }
        if(image->type!=LIBRAW_IMAGE_BITMAP||image->bits!=16||image->colors!=3||!dimensions(image->width,image->height)){
            LibRaw::dcraw_clear_mem(image);return fail(error,"Unsupported linear RAW bitmap");
        }
        out->kind=2;out->width=image->width;out->height=image->height;out->channels=image->colors;out->bits=image->bits;
        out->rendered=1;out->len=image->data_size;out->data=image->data;out->owner=image;
        info->process_ms=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-started).count();
        attempt.success=true;return 0;
    }catch(const std::exception &e){return fail(error,e.what());}catch(...){return fail(error,"LibRaw linear develop exception");}
}
FC_EXPORT int fc_raw_develop_linear(const FCPathChar *path,const FCDevelopParams *params,FCFrame *out,FCLinearInfo *info,char *error) {
    return develop_linear(path,params,out,info,error,nullptr,nullptr);
}
FC_EXPORT int fc_raw_develop_linear_gpu(const FCPathChar *path,const FCDevelopParams *params,FCFrame *out,FCLinearInfo *info,char *error,FCBayerCompute compute,void *compute_context) {
    return develop_linear(path,params,out,info,error,compute,compute_context);
}
FC_EXPORT int fc_raw_develop_linear_compute(const FCPathChar *path,const FCDevelopParams *params,FCFrame *out,FCLinearInfo *info,char *error,FCBayerCompute compute,void *compute_context,FCEntropyCompute entropy,void *entropy_context) {
    return develop_linear(path,params,out,info,error,compute,compute_context,entropy,entropy_context);
}
// Audit hook: what the last linear develop on this thread used for unpacking.
FC_EXPORT int fc_raw_last_unpack_dispatch(FCEntropyDispatch *out) {
    if(!out||!fc_last_dispatch_valid)return 0;
    *out=fc_last_dispatch;return 1;
}

#include "sensor_checkpoint.h"
