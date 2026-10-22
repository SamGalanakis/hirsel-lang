/* Finite vectors adapted from Rust f6c1107f8be13e48465e12c0fd26e6a39fe18e4f,
 * library/alloctests/tests/vec.rs:257-292 and core/src/ptr/mod.rs:490-567.
 * Copyright (c) The Rust Project Contributors; MIT OR Apache-2.0.
 * Full MIT notice: third-party/NOTICES.md, MIT (Rust).
 * Deviation: Hirsel explicit
 * copies replace Vec clone/clone_from; its inline byte layout is authoritative.
 * No Rust ownership, allocation failure order, authority or replay is imported.
 */
#include "hirsel_rt_v2.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>
@CALLS@
struct program_binding { const hirsel_rt_v2_layout *const *layouts; uint64_t count; };
extern const struct program_binding program __asm__("hirsel.program.registry");
static const unsigned char sheep[] = {0xc3,0xa9,0xf0,0x9f,0x90,0x91};
static uint64_t word(const uint8_t *p) { uint64_t n; memcpy(&n,p,8); return n; }
/* Reject before any read that would cross the supplied physical extent. */
static int item(const uint8_t *p, size_t size, const uint8_t *text, size_t len, uint64_t n) {
  return size >= 16 && word(p) == len && len == size-16 &&
    memcmp(p+8,text,len) == 0 && word(p+8+len) == n;
}
static size_t extent(unsigned v) {
  const size_t sizes[] = {16,19,22,16,46,106}; return sizes[v];
}
static int decode(unsigned v, const uint8_t *p, size_t size, uint64_t n) {
  if (size != extent(v)) return 0;
  if (v < 3) return item(p,size,v==0?(const uint8_t*)"":v==1?(const uint8_t*)"abc":sheep, v==0?0:v==1?3:6,n);
  unsigned count = v==3?0:v==4?1:3;
  if (word(p) != size-8 || word(p+8) != count) return 0;
  size_t at=16;
  for (unsigned i=0; i<count; ++i) {
    if (size-at < 8 || word(p+at) != 22) return 0;
    at+=8;
    if (size-at < 22 || !item(p+at,22,sheep,6,v==4?n:i)) return 0;
    at+=22;
  }
  return at==size;
}
static int separate(const void *a, size_t na, const void *b, size_t nb) {
  uintptr_t x=(uintptr_t)a,y=(uintptr_t)b;
  return na<=UINTPTR_MAX-x && nb<=UINTPTR_MAX-y && (x+na<=y || y+nb<=x);
}
static void check(unsigned v, hirsel_rt_v2_header *h, uint64_t n) {
  uint8_t *p=hirsel_rt_v2_payload(h);
  assert(p==(uint8_t*)h+8 && hirsel_rt_v2_capacity(h)>=extent(v));
  assert(decode(v,p,extent(v),n));
  const hirsel_rt_v2_layout *l=program.layouts[(h->descriptor & HIRSEL_RT_V2_INDEX_MASK)-2];
  assert(l->finalizer==NULL && l->step_count==0);
  /* ABI 37+ scans also carry type/value metadata. Those instructions are
   * not payload holders (core-contract/src/handle_scan.rs). */
  assert(l->scan_words!=UINT64_MAX && l->scan_words%8==0);
  for(uint64_t at=0;at<l->scan_words;at+=8) {
    uint64_t op=l->scan[at];assert(op==13 || op==15 || op==16);
  }
}
/* Link-only observation of compiler-selected local region placement. All
 * addresses stay in the C fixture; runtime ABI and source semantics are intact. */
static unsigned watching, copied, checkpoint, failure_copies, failure_index, growing, growth_moves;
static hirsel_rt_v2_header *observed[3];
void __real_hirsel_rt_v2_region_owner(hirsel_rt_v2_ctx*,hirsel_rt_v2_region*,hirsel_rt_v2_header*,uint32_t,uint64_t,uint64_t);
void __wrap_hirsel_rt_v2_region_owner(hirsel_rt_v2_ctx *ctx,hirsel_rt_v2_region *r,hirsel_rt_v2_header *h,uint32_t ix,uint64_t cap,uint64_t align) {
  __real_hirsel_rt_v2_region_owner(ctx,r,h,ix,cap,align);
  if(watching) {
    assert(observed[0]==NULL);observed[0]=h;
    assert(h->count==HIRSEL_RT_V2_COUNT_IMMORTAL && (h->descriptor>>29)==3);
    hirsel_rt_v2_stats s;hirsel_rt_v2_ctx_stats(ctx,&s);assert(s.active_regions>0 && s.region_active_bytes>0);
  }
}
hirsel_rt_v2_header *__real_hirsel_rt_v2_alloc(hirsel_rt_v2_ctx*,uint32_t,uint64_t,uint64_t,uint8_t,uint64_t);
hirsel_rt_v2_header *__wrap_hirsel_rt_v2_alloc(hirsel_rt_v2_ctx *ctx,uint32_t ix,uint64_t size,uint64_t align,uint8_t domain,uint64_t site) {
  hirsel_rt_v2_header *h=__real_hirsel_rt_v2_alloc(ctx,ix,size,align,domain,site);
  if(failure_index && ix==failure_index) ++failure_copies;
  if(watching && observed[0] && ix==(observed[0]->descriptor & HIRSEL_RT_V2_INDEX_MASK)) {
    assert(copied<2 && size==22);observed[++copied]=h;
  }
  return h;
}
hirsel_rt_v2_header *__real_hirsel_rt_v2_block_grow(hirsel_rt_v2_ctx*,hirsel_rt_v2_header*,uint64_t,uint64_t,uint64_t);
hirsel_rt_v2_header *__wrap_hirsel_rt_v2_block_grow(hirsel_rt_v2_ctx *ctx,hirsel_rt_v2_header *h,uint64_t keep,uint64_t extent,uint64_t align) {
  uint8_t saved[128];uint64_t count=0,capacity=0;
  if(growing) {
    assert(keep<=sizeof saved);capacity=hirsel_rt_v2_capacity(h);
    assert(keep<=capacity);memcpy(saved,hirsel_rt_v2_payload(h),keep);count=word(saved);
  }
  hirsel_rt_v2_header *out=__real_hirsel_rt_v2_block_grow(ctx,h,keep,extent,align);
  if(growing) {
    assert(!memcmp(saved,hirsel_rt_v2_payload(out),keep));
    assert(hirsel_rt_v2_capacity(out)>=extent);
    if(count>0 && extent>capacity && out!=h) ++growth_moves;
  }
  return out;
}
static void inspect_copies(void) {
  if(!watching || checkpoint || copied!=2) return;
  check(2,observed[0],7);check(2,observed[1],11);check(2,observed[2],19);
  for(unsigned i=0;i<3;++i)for(unsigned j=0;j<i;++j)
    assert(separate(observed[i],8+hirsel_rt_v2_capacity(observed[i]),observed[j],8+hirsel_rt_v2_capacity(observed[j])));
  checkpoint=1;
}
uint8_t __real_hirsel_rt_v2_release(hirsel_rt_v2_ctx*,hirsel_rt_v2_header*,uint32_t,const uint64_t*,uint64_t,uint64_t);
uint8_t __wrap_hirsel_rt_v2_release(hirsel_rt_v2_ctx *ctx,hirsel_rt_v2_header *h,uint32_t ix,const uint64_t *mask,uint64_t anchor,uint64_t site) {
  if(h==observed[1] || h==observed[2]) inspect_copies();
  return __real_hirsel_rt_v2_release(ctx,h,ix,mask,anchor,site);
}
/* Flat drop glue can call free directly; observe before that first free too. */
void __real_hirsel_rt_v2_free(hirsel_rt_v2_ctx*,hirsel_rt_v2_header*);
void __wrap_hirsel_rt_v2_free(hirsel_rt_v2_ctx *ctx,hirsel_rt_v2_header *h) {
  if(h==observed[1] || h==observed[2]) inspect_copies();
  __real_hirsel_rt_v2_free(ctx,h);
}
uint8_t __real_hirsel_rt_v2_region_exit(hirsel_rt_v2_ctx*,hirsel_rt_v2_region*,uint64_t,uint64_t);
uint8_t __wrap_hirsel_rt_v2_region_exit(hirsel_rt_v2_ctx *ctx,hirsel_rt_v2_region *r,uint64_t anchor,uint64_t site) {
  inspect_copies();return __real_hirsel_rt_v2_region_exit(ctx,r,anchor,site);
}
static void release(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *h) {
  assert(hirsel_rt_v2_release(ctx,h,h->descriptor & HIRSEL_RT_V2_INDEX_MASK,NULL,0,0)==0);
}
static uint8_t make(unsigned v, hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header **h) {
  switch(v) {
    case 0:return call_empty(ctx,NULL,h);
    case 1:return call_ascii(ctx,NULL,h);
    case 2:return call_utf8(ctx,NULL,h);
    case 3:return call_zero(ctx,NULL,h);
    case 4:return call_one(ctx,NULL,h);
    default: { growing=1;growth_moves=0;uint8_t status=call_growth(ctx,NULL,h);growing=0;assert(growth_moves>0);return status; }
  }
}
static uint8_t clone(unsigned v, hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *h, hirsel_rt_v2_header **out) {
  return v<3?call_clone_item(ctx,NULL,out,hirsel_rt_v2_payload(h),extent(v)):call_clone_batch(ctx,NULL,out,hirsel_rt_v2_payload(h),extent(v));
}
static void clean(hirsel_rt_v2_ctx *ctx) {
  hirsel_rt_v2_stats s; hirsel_rt_v2_ctx_stats(ctx,&s);
  assert(s.live_blocks==0 && s.live_bytes==0 && s.active_regions==0 && s.region_active_bytes==0);
  hirsel_rt_v2_audit a; hirsel_rt_v2_task_exit(ctx,&a);
  assert(a.live_blocks==0 && a.live_bytes==0 && a.active_regions==0 && a.retained_bytes==0);
  hirsel_rt_v2_ctx_destroy(ctx);
}
int main(void) {
  for(unsigned v=0;v<6;++v) for(unsigned regional=0;regional<2;++regional) for(unsigned reverse=0;reverse<2;++reverse) {
    fprintf(stderr,"flat vector=%u region=%u reverse=%u\n",v,regional,reverse);
    hirsel_rt_v2_config cfg={.abi_version=HIRSEL_RT_V2_ABI_VERSION,.flags=HIRSEL_RT_V2_CONFIG_DIAGNOSTIC,.layouts=program.layouts,.layout_count=(uint32_t)program.count};
    hirsel_rt_v2_ctx *ctx=NULL; assert(hirsel_rt_v2_ctx_create(&cfg,&ctx)==0);
    hirsel_rt_v2_header *original=NULL,*a=NULL,*b=NULL;
    assert(make(v,ctx,&original)==0); check(v,original,7);
    uint32_t original_count=original->count,original_descriptor=original->descriptor;
    uint8_t saved[106]; size_t size=extent(v); memcpy(saved,hirsel_rt_v2_payload(original),size);
    hirsel_rt_v2_region region;
    if(regional) {
      hirsel_rt_v2_region_enter(ctx,&region);
      uint8_t *storage=NULL; assert(hirsel_rt_v2_region_reserve(ctx,&region,16+8+size,8,(void**)&storage)==0);
      hirsel_rt_v2_header *r=(hirsel_rt_v2_header*)(storage+16);
      hirsel_rt_v2_region_owner(ctx,&region,r,original->descriptor & HIRSEL_RT_V2_INDEX_MASK,size,8);
      memcpy(hirsel_rt_v2_payload(r),saved,size); release(ctx,original); original=r;
      assert((r->descriptor>>HIRSEL_RT_V2_KIND_SHIFT)==3 && r->count==HIRSEL_RT_V2_COUNT_IMMORTAL);
      hirsel_rt_v2_stats s; hirsel_rt_v2_ctx_stats(ctx,&s);
      assert(s.active_regions==1 && s.region_active_bytes>0 && s.live_blocks==0);
    }
    assert(clone(v,ctx,original,&a)==0 && clone(v,ctx,original,&b)==0);
    hirsel_rt_v2_header *blocks[]={original,a,b};
    for(unsigned i=0;i<3;++i) {
      check(v,blocks[i],7);
      for(unsigned j=0;j<i;++j) assert(separate(blocks[i],8+hirsel_rt_v2_capacity(blocks[i]),blocks[j],8+hirsel_rt_v2_capacity(blocks[j])));
    }
    assert(a->count==1 && b->count==1);
    assert(original->count==(regional?HIRSEL_RT_V2_COUNT_IMMORTAL:original_count));
    assert(regional || original->descriptor==original_descriptor);
    if(v<3) {
      assert(call_write_item(ctx,&a,11)==0 && call_write_item(ctx,&b,19)==0);
    } else if(v!=3) {
      /* Fixed Item fields inside the flat Batch are writable in place. */
      uint64_t x=11,y=19; memcpy(hirsel_rt_v2_payload(a)+38,&x,8); memcpy(hirsel_rt_v2_payload(b)+38,&y,8);
    }
    uint8_t ea[106],eb[106]; memcpy(ea,saved,size);memcpy(eb,saved,size);
    if(v!=3) {
      uint64_t x=11,y=19;size_t at=v<3?size-8:38;
      memcpy(ea+at,&x,8);memcpy(eb+at,&y,8);
    }
    assert(!memcmp(ea,hirsel_rt_v2_payload(a),size));
    assert(!memcmp(eb,hirsel_rt_v2_payload(b),size));
    assert(!memcmp(saved,hirsel_rt_v2_payload(original),size));
    if(reverse) {
      if(regional) assert(hirsel_rt_v2_region_exit(ctx,&region,0,0)==0); else release(ctx,original);
      assert(!memcmp(ea,hirsel_rt_v2_payload(a),size));
      assert(!memcmp(eb,hirsel_rt_v2_payload(b),size));
      release(ctx,b);assert(!memcmp(ea,hirsel_rt_v2_payload(a),size));release(ctx,a);
    } else {
      release(ctx,a);assert(!memcmp(eb,hirsel_rt_v2_payload(b),size));release(ctx,b);check(v,original,7);
      if(regional) assert(hirsel_rt_v2_region_exit(ctx,&region,0,0)==0); else release(ctx,original);
    }
    clean(ctx);
  }
  /* Rust clone_from's zero/long/equal/short/long replacement discipline,
   * expressed with Hirsel's explicit copy and separately released old owner. */
  {
    hirsel_rt_v2_config cfg={.abi_version=HIRSEL_RT_V2_ABI_VERSION,.flags=HIRSEL_RT_V2_CONFIG_DIAGNOSTIC,.layouts=program.layouts,.layout_count=(uint32_t)program.count};
    hirsel_rt_v2_ctx *ctx=NULL;assert(hirsel_rt_v2_ctx_create(&cfg,&ctx)==0);
    hirsel_rt_v2_header *old=NULL;assert(make(3,ctx,&old)==0);
    const unsigned replacements[]={5,5,4,5};
    for(unsigned i=0;i<4;++i) {
      unsigned v=replacements[i];hirsel_rt_v2_header *src=NULL,*fresh=NULL;
      assert(make(v,ctx,&src)==0 && clone(v,ctx,src,&fresh)==0);
      assert(separate(old,8+hirsel_rt_v2_capacity(old),fresh,8+hirsel_rt_v2_capacity(fresh)));
      check(v,src,7);check(v,fresh,7);release(ctx,old);release(ctx,src);
      check(v,fresh,7);old=fresh;
    }
    release(ctx,old);clean(ctx);
  }
  /* Trigger a language failure after two explicit copies; caller retains original. */
  for(unsigned regional=0;regional<2;++regional) {
    hirsel_rt_v2_config cfg={.abi_version=HIRSEL_RT_V2_ABI_VERSION,.flags=HIRSEL_RT_V2_CONFIG_DIAGNOSTIC,.layouts=program.layouts,.layout_count=(uint32_t)program.count};
    hirsel_rt_v2_ctx *ctx=NULL; assert(hirsel_rt_v2_ctx_create(&cfg,&ctx)==0);
    hirsel_rt_v2_header *h=NULL; assert(make(2,ctx,&h)==0);
    hirsel_rt_v2_region r;
    if(regional) {
      hirsel_rt_v2_region_enter(ctx,&r); uint8_t *p=NULL;
      assert(hirsel_rt_v2_region_reserve(ctx,&r,46,8,(void**)&p)==0);
      hirsel_rt_v2_header *q=(hirsel_rt_v2_header*)(p+16);
      hirsel_rt_v2_region_owner(ctx,&r,q,h->descriptor & HIRSEL_RT_V2_INDEX_MASK,22,8);
      memcpy(hirsel_rt_v2_payload(q),hirsel_rt_v2_payload(h),22);release(ctx,h);h=q;
    }
    failure_index=h->descriptor & HIRSEL_RT_V2_INDEX_MASK;failure_copies=0;
    uint64_t out=0; assert(call_fail_item(ctx,&out,hirsel_rt_v2_payload(h),22,0)==1);
    failure_index=0;assert(failure_copies==2);
    assert(hirsel_rt_v2_failure_active(ctx)); check(2,h,7);
    hirsel_rt_v2_stats s;hirsel_rt_v2_ctx_stats(ctx,&s);assert(s.live_blocks==(regional?0:1));
    hirsel_rt_v2_failure_clear(ctx);
    if(regional) assert(hirsel_rt_v2_region_exit(ctx,&r,0,0)==0); else release(ctx,h);
    clean(ctx);
  }
  {
    hirsel_rt_v2_config cfg={.abi_version=HIRSEL_RT_V2_ABI_VERSION,.flags=HIRSEL_RT_V2_CONFIG_DIAGNOSTIC,.layouts=program.layouts,.layout_count=(uint32_t)program.count};
    hirsel_rt_v2_ctx *ctx=NULL;assert(hirsel_rt_v2_ctx_create(&cfg,&ctx)==0);
    uint64_t out=0;watching=1;
    assert(call_local_region(ctx,&out)==0 && out==37);
    watching=0;assert(checkpoint==1 && copied==2);clean(ctx);
  }
  uint8_t bytes[22]={6,0,0,0,0,0,0,0,0xc3,0xa9,0xf0,0x9f,0x90,0x91,7};
  assert(decode(2,bytes,22,7));
  assert(!decode(2,bytes,21,7)); bytes[0]=255;assert(!decode(2,bytes,22,7));
  assert(!separate(bytes,22,bytes+8,14));
  puts("native flat oracle: 24 copy/lifetime cells, 2 cleanup failures, 3 rejected controls, 4 replacements, compiler region checkpoint; zero live allocations");
}
