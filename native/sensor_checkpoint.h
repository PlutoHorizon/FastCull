// Read-only diagnostic checkpoint. The visitor borrows LibRaw-owned memory only
// until it returns; no sensor pointer or decoder is retained by this interface.
struct FCSensorCheckpointV1 {
    uint32_t abi,stage,width,height,active_width,active_height,left,top;
    uint32_t channels,storage_channels,row_stride,filters,orientation,bits;
    uint32_t dng_version,decoder_flags,black,white;
    float camera_mul[4],pre_mul[4];
    uint64_t file_bytes,file_stamp,storage_len;
    const uint32_t *cblack;
    uint32_t cblack_len,reserved;
    const uint16_t *curve;
    uint32_t curve_len,flags;
    char decoder[32];
};
static_assert(sizeof(FCSensorCheckpointV1)==192,"sensor checkpoint ABI layout changed");
using FCSensorVisitV1=int(*)(void*,const FCSensorCheckpointV1*,const uint16_t*);

FC_EXPORT int fc_raw_sensor_checkpoint_v1(const FCPathChar *path,const FCDevelopParams *params,
    FCEntropyCompute entropy,void *entropy_context,FCSensorVisitV1 visit,void *visit_context,char *error) {
    fc_last_dispatch_valid=false;
    if(!path||!params||!error||!visit)return -1;
    if(params->abi!=1||params->max_edge||params->temperature!=0||params->tint!=0||params->exposure!=0)
        return fail(error,"Sensor checkpoint requires unadjusted full sensor parameters");
    std::unique_lock<std::mutex> gate(develop_gate,std::try_to_lock);
    if(!gate.owns_lock())return fail(error,"Another local RAW operation is active");
    try {
        uint64_t bytes,stamp,after_bytes,after_stamp;
        if(!file_stamp(path,bytes,stamp))return fail(error,"Cannot stamp RAW checkpoint input");
        DevelopRaw raw;ActiveDevelop active(raw);DevelopContext context{params};
        configure_develop_threads();
        raw.set_progress_handler(develop_progress,&context);
        raw.imgdata.rawparams.max_raw_memory_mb=1024;
        int code=open_camera(raw,path);
        if(code)return fail(error,libraw_strerror(code));
        libraw_decoder_info_t decoder{};
        raw.get_decoder_info(&decoder);
        if(context.cancelled())return fail(error,"Sensor checkpoint cancelled before unpack");
        code=raw.unpack_compute(entropy,entropy_context);
        if(code)return fail(error,libraw_strerror(code));
        if(context.cancelled())return fail(error,"Sensor checkpoint cancelled after unpack");
        const auto &d=raw.imgdata;
        const auto &s=d.rawdata.sizes;
        const auto &color=d.rawdata.color;
        FCSensorCheckpointV1 info{};
        info.abi=1;info.stage=1; // after unpack(), before raw2image/scale_colors/WB/AHD
        info.width=s.raw_width;info.height=s.raw_height;
        info.active_width=s.width;info.active_height=s.height;
        info.left=s.left_margin;info.top=s.top_margin;
        info.filters=d.idata.filters;info.orientation=orientation(s.flip);
        info.bits=color.raw_bps;info.dng_version=d.idata.dng_version;
        info.decoder_flags=decoder.decoder_flags;
        info.black=color.black;info.white=color.maximum;
        std::memcpy(info.camera_mul,color.cam_mul,sizeof(info.camera_mul));
        std::memcpy(info.pre_mul,color.pre_mul,sizeof(info.pre_mul));
        info.file_bytes=bytes;info.file_stamp=stamp;
        info.cblack=color.cblack;info.cblack_len=sizeof(color.cblack)/sizeof(color.cblack[0]);
        info.curve=color.curve;info.curve_len=sizeof(color.curve)/sizeof(color.curve[0]);
        // Decoder-owned DNG curve application is already included at this stage.
        info.flags=(decoder.decoder_flags&LIBRAW_DECODER_ADOBECOPYPIXEL)?1u:0u;
        std::snprintf(info.decoder,sizeof(info.decoder),"%s",decoder.decoder_name?decoder.decoder_name:"");
        const uint16_t *data=nullptr;
        if(d.rawdata.raw_image){info.channels=info.storage_channels=1;data=d.rawdata.raw_image;}
        else if(d.rawdata.color3_image){info.channels=info.storage_channels=3;data=&d.rawdata.color3_image[0][0];}
        else if(d.rawdata.color4_image&&d.idata.colors==3){info.channels=3;info.storage_channels=4;data=&d.rawdata.color4_image[0][0];}
        else return fail(error,"Unsupported sensor checkpoint storage (float/other layout)");
        if(!dimensions(info.width,info.height)||uint64_t(info.width)*info.height*info.channels>160000000)
            return fail(error,"Sensor checkpoint dimensions exceed limit");
        info.row_stride=s.raw_pitch?s.raw_pitch/2:info.width*info.storage_channels;
        if((s.raw_pitch&1)||info.row_stride<uint64_t(info.width)*info.storage_channels)
            return fail(error,"Invalid sensor checkpoint stride");
        info.storage_len=uint64_t(info.row_stride)*(info.height-1)+uint64_t(info.width)*info.storage_channels;
        if(!file_stamp(path,after_bytes,after_stamp)||bytes!=after_bytes||stamp!=after_stamp)
            return fail(error,"RAW changed during sensor checkpoint");
        const int status=visit(visit_context,&info,data);
        if(status<=0||context.cancelled())return fail(error,"Sensor checkpoint visitor cancelled/rejected");
        return 0;
    }catch(const std::exception &e){return fail(error,e.what());}
     catch(...){return fail(error,"Sensor checkpoint exception");}
}
