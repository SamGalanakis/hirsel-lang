// Supplemental resource row B: explicit goto-cleanup on every exit path.
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef STATS
static uint64_t drops, copies;
#define COUNT(k,n) ((k)+=(n))
#else
#define COUNT(k,n) ((void)0)
#endif
static uint64_t digest(const unsigned char *b,size_t n){uint64_t h=UINT64_C(14695981039346656037);for(size_t j=0;j<n;j++)h=(h^b[j])*UINT64_C(1099511628211);return h;}
typedef struct {unsigned char id;bool closed,trace;} Resource;
static void close_resource(Resource *r){assert(!r->closed);r->closed=true;COUNT(drops,1);if(r->trace)assert(putchar(r->id)!=EOF);}
static bool attempt(uint64_t i,uint64_t modulus,bool trace,unsigned char destination[16]){
    Resource a={.id='8',.trace=trace},b={.id='7',.trace=trace};
    unsigned char source[16],transaction[16];bool failed=true;
    for(size_t j=0;j<16;j++)source[j]=(unsigned char)(i%127+j);
    memcpy(transaction,source,16);COUNT(copies,16);
    for(size_t j=0;j<16;j++){assert(transaction[j]<255);transaction[j]++;}
    if(i%modulus==0)goto cleanup;
    memcpy(destination,transaction,16);COUNT(copies,16);failed=false;
cleanup:
    close_resource(&b);close_resource(&a);assert(a.closed&&b.closed);return failed;
}
#ifndef MODULUS
#define MODULUS 17
#endif
static unsigned char *workload(const char *input,bool trace,size_t *length){
    char *end;uint64_t n=strtoull(input,&end,10);assert(*end=='\n'&&end[1]==0&&n>0&&n<=1000000);
    const uint64_t modulus=MODULUS;
    uint64_t total=0;
    for(uint64_t i=0;i<n;i++){
        unsigned char destination[16]={0};bool failed=attempt(i,modulus,trace,destination);assert(failed==(i%modulus==0));
        uint64_t h=i;uint64_t events[]={1,2,3,4,failed?6:5,7,8};
        for(size_t j=0;j<7;j++)h=h*131+events[j];
        total+=h^digest(destination,16);
    }
    unsigned char *out=malloc(32);assert(out);int count=snprintf((char*)out,32,"%"PRIu64"\n",total);assert(count>0&&count<32);*length=(size_t)count;return out;
}
int main(int argc,char **argv){
    assert(argc>=5&&argc<=7);bool emit=argc>=6&&!strcmp(argv[5],"--emit"),trace=argc==7&&!strcmp(argv[6],"--trace");
    assert(!trace||emit);assert(!strcmp(argv[4],"system")||!strcmp(argv[4],"mimalloc"));
    assert(!strcmp(argv[1],MODULUS==2?"resource-drops/heavy":"resource-drops/throughput"));
    FILE *f=fopen(argv[2],"rb");assert(f);char input[64]={0};size_t length=fread(input,1,63,f);assert(length>0&&length<63&&!ferror(f));assert(!fclose(f));
    char *end;uint64_t rounds=strtoull(argv[3],&end,10);assert(rounds&&!*end&&(!emit||rounds==1));uint64_t sum=0;
    for(uint64_t r=0;r<rounds;r++){size_t length;unsigned char *out=workload(input,trace,&length);sum+=digest(out,length);if(emit)assert(fwrite(out,1,length,stdout)==length);free(out);}
    if(!emit)printf("%016"PRIx64"\n",sum);
#ifdef STATS
    fprintf(stderr,"{\"allocation_requests\":%"PRIu64",\"requested_bytes\":%"PRIu64",\"copied_bytes\":%"PRIu64",\"release_requests\":%"PRIu64",\"finalizer_calls\":%"PRIu64"}\n",rounds,rounds*32,copies,rounds,drops);
#endif
    return 0;
}
