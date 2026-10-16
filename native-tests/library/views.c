#include "../../stdlib/native/library.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stddef.h>

_Static_assert(sizeof(hirsel_rt_v2_view_cursor) == 32, "cursor");
_Static_assert(offsetof(hirsel_rt_v2_view_cursor, position) == 24, "position");
_Static_assert(sizeof(hirsel_rt_v2_scalar_option) == 8, "scalar option");
_Static_assert(sizeof(hirsel_rt_v2_position_option) == 16, "position option");

static void release(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *owner) {
    assert(hirsel_rt_v2_release(ctx, owner, 0, NULL, 0) == 0);
}
static size_t utf8(uint32_t c, unsigned char *out) {
    if(c < 0x80) { out[0]=(unsigned char)c; return 1; }
    if(c < 0x800) { out[0]=0xc0|(c>>6); out[1]=0x80|(c&63); return 2; }
    if(c < 0x10000) { out[0]=0xe0|(c>>12); out[1]=0x80|((c>>6)&63); out[2]=0x80|(c&63); return 3; }
    out[0]=0xf0|(c>>18); out[1]=0x80|((c>>12)&63); out[2]=0x80|((c>>6)&63); out[3]=0x80|(c&63); return 4;
}
static void vector(hirsel_rt_v2_ctx *ctx, char *line) {
    unsigned char text[2048]; memcpy(text,"prefix",6);
    uint32_t scalars[256]; uint64_t positions[257];
    size_t length=6, count=0;
    char *comment=strchr(line,'#'); if(comment) *comment=0;
    for(char *token=strtok(line," \t\r\n"); token; token=strtok(NULL," \t\r\n")) {
        if(strcmp(token,"÷")!=0 && strcmp(token,"×")!=0) {
            assert(count < 256);
            uint32_t scalar=(uint32_t)strtoul(token,NULL,16);
            positions[count]=length-6; scalars[count++]=scalar;
            length+=utf8(scalar,text+length);
        }
    }
    assert(count); positions[count]=length-6;
    memcpy(text+length,"suffix",6);
    hirsel_rt_v2_header *owner=hirsel_rt_v2_alloc(ctx,0,8+length+6,8,HIRSEL_RT_V2_DOMAIN_HEAP,0);
    unsigned char *payload=(unsigned char*)owner+HIRSEL_RT_V2_HEADER_SIZE;
    uint64_t byte_count=length+6; memcpy(payload,&byte_count,8); memcpy(payload+8,text,length+6);
    hirsel_rt_v2_view_cursor cursor;
    hirsel_rt_v2_scalar_option scalar;
    assert(hirsel_rt_v2_unicode_scalars_open(ctx,NULL,&cursor,owner,6,length-6)==0 && owner->count==1);
    for(size_t i=0;i<count;i++) {
        assert(hirsel_rt_v2_unicode_scalars_next(ctx,NULL,&scalar,&cursor)==0);
        assert(scalar.tag==1 && scalar.value==scalars[i]);
    }
    assert(hirsel_rt_v2_unicode_scalars_next(ctx,NULL,&scalar,&cursor)==0 && scalar.tag==0);
    hirsel_rt_v2_position_option pos;
    assert(hirsel_rt_v2_scalar_positions_open(ctx,NULL,&cursor,owner,6,length-6)==0);
    for(size_t i=0;i<=count;i++) {
        assert(hirsel_rt_v2_scalar_positions_next(ctx,NULL,&pos,&cursor)==0);
        assert(pos.tag==1 && pos.value==positions[i]);
    }
    assert(hirsel_rt_v2_scalar_positions_next(ctx,NULL,&pos,&cursor)==0 && pos.tag==0);
    assert(hirsel_rt_v2_scalar_positions_open(ctx,NULL,&cursor,owner,6,0)==0);
    assert(hirsel_rt_v2_scalar_positions_next(ctx,NULL,&pos,&cursor)==0 && pos.tag==1 && pos.value==0);
    assert(hirsel_rt_v2_scalar_positions_next(ctx,NULL,&pos,&cursor)==0 && pos.tag==0);
    release(ctx,owner);
}
int main(int argc,char **argv) {
    assert(argc==2);
    FILE *vectors=fopen(argv[1],"r"); assert(vectors);
    hirsel_rt_v2_config config={0}; config.abi_version=HIRSEL_RT_V2_ABI_VERSION; config.flags=HIRSEL_RT_V2_CONFIG_DIAGNOSTIC;
    hirsel_rt_v2_ctx *ctx=NULL; assert(hirsel_rt_v2_ctx_create(&config,&ctx)==0);
    char line[4096]; size_t cases=0;
    while(fgets(line,sizeof line,vectors)) {
        char *data=line; while(*data==' ' || *data=='\t') data++;
        if(*data=='#' || *data=='\n' || *data=='\r' || *data==0) continue;
        vector(ctx,data); cases++;
    }
    assert(cases==1093); fclose(vectors);
    hirsel_rt_v2_stats stats; hirsel_rt_v2_ctx_stats(ctx,&stats); assert(stats.live_blocks==0);
    hirsel_rt_v2_ctx_destroy(ctx);
    return 0;
}
