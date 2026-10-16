// Indexed ownership and algorithms match rust/src/main.rs.
#include <assert.h>
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>
#include <errno.h>

#ifdef STATS
static uint64_t counters[9];
#define COUNT(k,n) (counters[k]+=(uint64_t)(n))
#define PEAK(n) (counters[8]=counters[8]>(n)?counters[8]:(n))
#else
#define COUNT(k,n) ((void)0)
#define PEAK(n) ((void)0)
#endif
static uint64_t add(uint64_t a,uint64_t b){uint64_t n;if(__builtin_add_overflow(a,b,&n))abort();return n;}
static uint64_t mul(uint64_t a,uint64_t b){uint64_t n;if(__builtin_mul_overflow(a,b,&n))abort();return n;}
static uint64_t digest(const unsigned char*b,size_t n){uint64_t h=UINT64_C(14695981039346656037);for(size_t i=0;i<n;i++)h=(h^b[i])*UINT64_C(1099511628211);return h;}
// Heap variants use malloc/free; mimalloc replaces those linked symbols.
static void*alloc(size_t n){COUNT(0,1);COUNT(1,n);void*p=malloc(n?n:1);if(!p)abort();return p;}
static void release(void*p){if(p){COUNT(3,1);free(p);}}
static unsigned char*copy(const unsigned char*b,size_t n){unsigned char*p=alloc(n);memcpy(p,b,n);COUNT(2,n);return p;}
typedef struct {unsigned char tag;uint64_t value,rc;size_t a,b,len;unsigned char*bytes;} Node;
typedef struct {Node**nodes;size_t len,cap;} Pool;
static Pool pool(size_t cap){Pool p={alloc(mul(cap+1,sizeof(Node*))),1,cap+1};p.nodes[0]=NULL;return p;}
static Node*get(Pool*p,size_t i){assert(i<p->len&&p->nodes[i]);return p->nodes[i];}
static size_t node(Pool*p,unsigned char tag,uint64_t value,size_t a,size_t b,const unsigned char*bytes,size_t len){assert(p->len<p->cap);Node*v=alloc(sizeof(Node));*v=(Node){tag,value,1,a,b,len,bytes?copy(bytes,len):NULL};p->nodes[p->len]=v;return p->len++;}
static void remove_node(Pool*p,size_t i){Node*v=get(p,i);release(v->bytes);release(v);p->nodes[i]=NULL;}
static void drop_pool(Pool*p){for(size_t i=1;i<p->len;i++)if(p->nodes[i])remove_node(p,i);release(p->nodes);}
static void drop_list(Pool*p,size_t i){while(i){Node*v=get(p,i);COUNT(4,1);assert(v->rc);if(--v->rc)break;size_t next=v->a;remove_node(p,i);i=next;}}
typedef struct {unsigned char*b;size_t len,cap;} Buffer;
static Buffer buffer(size_t cap){return(Buffer){alloc(cap),0,cap};}
static void byte(Buffer*b,unsigned char x){assert(b->len<b->cap);b->b[b->len++]=x;}
static void append(Buffer*b,const unsigned char*s,size_t n){assert(n<=b->cap-b->len);memcpy(b->b+b->len,s,n);b->len+=n;}
static void number(Buffer*b,uint64_t v){char out[32];int n=snprintf(out,sizeof(out),"%"PRIu64"\n",v);assert(n>0);append(b,(unsigned char*)out,(size_t)n);}
// Conventional binary-tree representation: exactly two child links per node.
typedef struct Tree {struct Tree*a,*b;} Tree;
_Static_assert(sizeof(Tree)==16,"the baseline tree contains only two links");
#ifdef ARENA
#include "binarytrees-arena.h"
#else
static Tree*tree(size_t d){Tree*a=NULL,*b=NULL;if(d){a=tree(d-1);b=tree(d-1);}Tree*t=alloc(sizeof(*t));*t=(Tree){a,b};return t;}
static uint64_t check_tree(Tree*t){return t?add(1,add(check_tree(t->a),check_tree(t->b))):0;}
static void drop_tree(Tree*t,Tree**pending,size_t cap){size_t n=0;pending[n++]=t;while(n){Tree*v=pending[--n];if(v->a){assert(n<cap);pending[n++]=v->a;}if(v->b){assert(n<cap);pending[n++]=v->b;}release(v);}}
static uint64_t binary(size_t d){assert(d>=4&&d<=20);size_t cap=d+3;Tree**pending=alloc(cap*sizeof(*pending));Tree*long_tree=tree(d),*stretch=tree(d+1);uint64_t total=check_tree(stretch);drop_tree(stretch,pending,cap);
    for(size_t k=4;k<=d;k+=2)for(size_t j=0;j<((size_t)1<<(d-k));j++){Tree*t=tree(k);total=add(total,check_tree(t));drop_tree(t,pending,cap);}
    total=add(total,check_tree(long_tree));drop_tree(long_tree,pending,cap);release(pending);return total;
}
#endif
static size_t parse_expr(Pool*p,const unsigned char*b,size_t len,size_t*i){assert(*i<len);unsigned char c=b[(*i)++];if(c=='('){size_t a=parse_expr(p,b,len,i);assert(*i<len);unsigned char op=b[(*i)++];assert(op=='+'||op=='*');size_t r=parse_expr(p,b,len,i);assert(*i<len&&b[(*i)++]==')');return node(p,op=='+'?2:3,0,a,r,NULL,0);}assert(c=='x'||(c>='0'&&c<='9'));return node(p,c=='x'?1:0,c=='x'?3:c-'0',0,0,NULL,0);}
static uint64_t eval(Pool*p,size_t i){Node*v=get(p,i);if(v->tag<2)return v->value;uint64_t a=eval(p,v->a),b=eval(p,v->b);return v->tag==2?add(a,b):mul(a,b);}
static bool hex(unsigned char c){return(c>='0'&&c<='9')||(c>='a'&&c<='f')||(c>='A'&&c<='F');}
static size_t quoted(const unsigned char*b,size_t len,size_t*i){size_t start=*i;assert(*i<len&&b[(*i)++]=='"');while(1){assert(*i<len);unsigned char c=b[(*i)++];if(c=='"')break;assert(c>=32);if(c=='\\'){assert(*i<len);unsigned char e=b[(*i)++];if(e=='u'){for(int k=0;k<4;k++){assert(*i<len&&hex(b[*i]));(*i)++;}}else assert(strchr("\"\\/bfnrt",e));}}return *i-start;}
static size_t parse_json(Pool*p,const unsigned char*b,size_t len,size_t*i){assert(*i<len);unsigned char c=b[*i];if(c=='"'){size_t start=*i;size_t n=quoted(b,len,i);return node(p,0,0,0,0,b+start,n);}if(c=='['||c=='{'){bool obj=c=='{';(*i)++;unsigned char close=obj?'}':']';size_t first=0,last=0;
    while(1){assert(*i<len);if(b[*i]==close)break;if(first){assert(b[(*i)++]==',');}size_t child;
        if(obj){size_t start=*i;size_t n=quoted(b,len,i);assert(*i<len&&b[(*i)++]==':');size_t val=parse_json(p,b,len,i);child=node(p,3,0,val,0,b+start,n);}else child=parse_json(p,b,len,i);
        if(!first)first=child;else get(p,last)->b=child;last=child;
    }(*i)++;return node(p,obj?2:1,0,first,0,NULL,0);}
    size_t start=*i;while(*i<len&&!strchr(",]}",b[*i]))(*i)++;size_t n=*i-start;assert(n>0);
    if(!((n==4&&!memcmp(b+start,"true",4))||(n==5&&!memcmp(b+start,"false",5))||(n==4&&!memcmp(b+start,"null",4)))){char tmp[64];assert(n<sizeof(tmp));memcpy(tmp,b+start,n);tmp[n]=0;char*end;errno=0;strtod(tmp,&end);assert(!errno&&end==tmp+n);}
    return node(p,0,0,0,0,b+start,n);
}
static void emit_json(Pool*p,size_t i,Buffer*out){Node*v=get(p,i);switch(v->tag){case 0:append(out,v->bytes,v->len);break;case 1:case 2:byte(out,v->tag==1?'[':'{');for(size_t child=v->a;child;child=get(p,child)->b){if(child!=v->a)byte(out,',');emit_json(p,child,out);}byte(out,v->tag==1?']':'}');break;case 3:append(out,v->bytes,v->len);byte(out,':');emit_json(p,v->a,out);break;default:abort();}}
static void put64(Buffer*b,uint64_t v){for(int k=0;k<8;k++)byte(b,(unsigned char)(v>>(k*8)));}
static uint64_t read64(const unsigned char*b){uint64_t v=0;for(int k=0;k<8;k++)v|=(uint64_t)b[k]<<(k*8);return v;}
static bool whitespace(unsigned char c){return c==' '||c=='\n'||c=='\r'||c=='\t'||c=='\v'||c=='\f';}
__attribute__((noinline)) static uint64_t transform(uint64_t x,uint64_t env){return add(x,env);}
__attribute__((noinline)) static uint64_t multiply(uint64_t x,uint64_t env){return mul(x,env);}
typedef struct {uint64_t(*call)(uint64_t,uint64_t);uint64_t env;} Stage;
typedef struct {uint8_t generation;bool live,retired;uint64_t value;} Slot;
static Buffer run_case(const char*name,const unsigned char*b,size_t len){Buffer out=buffer(!strncmp(name,"persistent-list-map/",20)?mul(strtoull((char*)b,NULL,10),44)+256:mul(len,2)+256);uint64_t sum=0;
    if(!strcmp(name,"expr-compiler")){unsigned char*tokens=copy(b,len);Pool p=pool(len);size_t i=0;while(i<len){size_t root=parse_expr(&p,tokens,len,&i);sum=add(sum,eval(&p,root));assert(i<len&&tokens[i++]=='\n');}number(&out,sum);drop_pool(&p);release(tokens);return out;}
    if(!strcmp(name,"json-roundtrip")){Pool p=pool(len);size_t i=0;size_t root=parse_json(&p,b,len,&i);assert(i==len);emit_json(&p,root,&out);assert(out.len==len&&!memcmp(out.b,b,len));drop_pool(&p);return out;}
    if(!strcmp(name,"wordfreq")){Pool p=pool(len);size_t*buckets=alloc(256*sizeof(size_t));memset(buckets,0,256*sizeof(size_t));size_t pos=0;
        while(pos<len){while(pos<len&&whitespace(b[pos]))pos++;if(pos==len)break;size_t start=pos;while(pos<len&&!whitespace(b[pos]))pos++;size_t n=pos-start;unsigned char*token=copy(b+start,n);size_t h=(digest(token,n)^0x4d3142)%256;size_t i=buckets[h];while(i&&(get(&p,i)->len!=n||memcmp(get(&p,i)->bytes,token,n)))i=get(&p,i)->a;
            if(!i){size_t id=node(&p,0,1,buckets[h],0,token,n);buckets[h]=id;}else get(&p,i)->value=add(get(&p,i)->value,1);release(token);}
        for(size_t i=1;i<p.len;i++){Node*v=get(&p,i);append(&out,v->bytes,v->len);byte(&out,'\t');number(&out,v->value);}drop_pool(&p);release(buckets);return out;}
    char*end;errno=0;size_t n=strtoull((char*)b,&end,10);assert(!errno&&*end=='\n'&&n>0&&n<=1000000);
    if(!strcmp(name,"binarytrees"))sum=binary(n);
    else if(!strcmp(name,"batch-validation")){Pool p=pool(n);size_t head=0;const unsigned char*pos=(unsigned char*)end+1;for(size_t i=0;i<n;i++){char*e;uint64_t code=strtoull((char*)pos,&e,10);assert(e!=(char*)pos&&*e==','&&code<=255);pos=(unsigned char*)e+1;uint64_t amount=strtoull((char*)pos,&e,10);assert(e!=(char*)pos&&*e=='\n');pos=(unsigned char*)e+1;head=node(&p,(unsigned char)code,amount,head,0,NULL,0);}assert(pos==b+len);while(head){Node*v=get(&p,head);sum=add(sum,v->tag<25&&v->value<=900?mul(v->value,v->tag+1):10000+v->tag);head=v->a;}drop_pool(&p);}
    else if(!strcmp(name,"records-messages/scalar")){uint64_t(*rows)[4]=alloc(mul(n,sizeof(*rows)));for(size_t i=0;i<n;i++){rows[i][0]=i;rows[i][1]=i%7;rows[i][2]=i%13;rows[i][3]=1;}for(int k=0;k<16;k++)for(size_t i=0;i<n;i++){rows[i][0]=add(rows[i][0],rows[i][3]);rows[i][1]=add(rows[i][1],2);rows[i][2]=add(rows[i][2],rows[i][0]%11);}for(size_t i=0;i<n;i++)for(int j=0;j<4;j++)sum=add(sum,rows[i][j]);release(rows);}
    else if(!strcmp(name,"records-messages/flat")){Buffer flat=buffer(mul(n,151));for(size_t i=0;i<n;i++){size_t l=24+i%104;put64(&flat,i);put64(&flat,i%7);put64(&flat,l);for(size_t j=0;j<l;j++)byte(&flat,97+(i+j*7)%26);}size_t pos=0,index=0;while(pos<flat.len){assert(24<=flat.len-pos);uint64_t id=read64(flat.b+pos),kind=read64(flat.b+pos+8);size_t l=read64(flat.b+pos+16);assert(l<=flat.len-pos-24);sum+=id^kind^digest(flat.b+pos+24,l);if(index%16==0){unsigned char*c=copy(flat.b+pos,24+l);uint64_t v=id^0x123;for(int k=0;k<8;k++)c[k]=(unsigned char)(v>>(k*8));sum+=v^kind^digest(c+24,l);release(c);}pos+=24+l;index++;}release(flat.b);}
    else if(!strcmp(name,"persistent-list-map/unique")||!strcmp(name,"persistent-list-map/shared")){bool shared=!strcmp(name,"persistent-list-map/shared");Pool p=pool(n*2);size_t root=0;for(size_t i=n;i>0;i--)root=node(&p,0,i-1,root,0,NULL,0);if(shared){get(&p,root)->rc++;COUNT(4,1);}size_t cur=root,mapped=0;
        while(cur){Node*v=get(&p,cur);size_t next=v->a;uint64_t value=add(v->value,1);if(shared){COUNT(6,1);mapped=node(&p,0,value,mapped,0,NULL,0);}else{COUNT(5,1);v->value=value;v->a=mapped;mapped=cur;}cur=next;}get(&p,mapped)->value=add(get(&p,mapped)->value,7);for(cur=mapped;cur;cur=get(&p,cur)->a)number(&out,get(&p,cur)->value);drop_list(&p,mapped);if(shared){append(&out,(unsigned char*)"---\n",4);for(cur=root;cur;cur=get(&p,cur)->a)number(&out,get(&p,cur)->value);drop_list(&p,root);drop_list(&p,root);}drop_pool(&p);return out;}
    else if(!strcmp(name,"saxpy-checked/f32")){float*x=alloc(n*sizeof(float)),*y=alloc(n*sizeof(float));for(size_t i=0;i<n;i++){x[i]=(float)(i%127)*0.25f;y[i]=(float)(i%31)*0.125f;}for(size_t i=0;i<n;i++)y[i]=1.5f*x[i]+y[i];for(size_t i=0;i<n;i++){uint32_t bits;memcpy(&bits,y+i,4);sum+=bits;}release(x);release(y);}
    else if(!strcmp(name,"saxpy-checked/f64")){double*x=alloc(n*sizeof(double)),*y=alloc(n*sizeof(double));for(size_t i=0;i<n;i++){x[i]=(double)(i%127)*0.25;y[i]=(double)(i%31)*0.125;}for(size_t i=0;i<n;i++)y[i]=1.5*x[i]+y[i];for(size_t i=0;i<n;i++){uint64_t bits;memcpy(&bits,y+i,8);sum+=bits;}release(x);release(y);}
    else if(!strcmp(name,"saxpy-checked/integer")){uint32_t*x=alloc(n*sizeof(uint32_t)),*y=alloc(n*sizeof(uint32_t));for(size_t i=0;i<n;i++){x[i]=i==n/2?UINT32_MAX:i%1000;y[i]=i%97;}for(size_t i=0;i<n;i++){uint32_t v;if(__builtin_mul_overflow(x[i],3u,&v)||__builtin_add_overflow(v,y[i],&v)){sum=add(sum,1000000000+i);break;}y[i]=v;sum=add(sum,v);}release(x);release(y);}
    else if(!strcmp(name,"slots-churn")){Slot*slots=alloc((n/255+3)*sizeof(Slot));memset(slots,0,(n/255+3)*sizeof(Slot));slots[0]=(Slot){1,true,false,10};slots[1]=(Slot){1,true,false,20};slots[2].generation=1;size_t active=2;for(size_t i=0;i<n;i++){Slot*s=slots+active;assert(!s->retired);uint8_t generation=s->generation;s->live=true;s->value=i+1;size_t old=active;uint64_t h=i;for(size_t j=0;j<=active;j++)if(slots[j].live){h=((h*131+j)*131+slots[j].generation)*131+slots[j].value;}sum+=h;s->live=false;if(generation==255){s->retired=true;sum++;active++;slots[active]=(Slot){.generation=1};}else s->generation++;Slot*probe=slots+old;assert(probe->generation!=generation||probe->retired||!probe->live);sum++;}release(slots);}
    else if(!strcmp(name,"closure-iter/scoped")){for(size_t i=0;i<n;i++){uint64_t x=add(i,3);if(x%2==0)sum=add(sum,mul(x,2));}}
    else if(!strcmp(name,"closure-iter/erased")){volatile Stage*stages=alloc(2*sizeof(Stage));stages[0]=(Stage){transform,3};stages[1]=(Stage){multiply,2};for(size_t i=0;i<n;i++){uint64_t x=stages[0].call(i,stages[0].env);if(x%2==0)sum=add(sum,stages[1].call(x,stages[1].env));}release((void*)stages);}
    else if(!strcmp(name,"resource-failure/throughput")||!strcmp(name,"resource-failure/heavy")){size_t modulus=!strcmp(name,"resource-failure/heavy")?2:17;for(size_t i=0;i<n;i++){struct Resource{uint8_t id;bool closed;}*r=alloc(2*sizeof(*r));r[0]=(struct Resource){1,false};r[1]=(struct Resource){2,false};unsigned char source[16],transaction[16],destination[16]={0};for(size_t j=0;j<16;j++)source[j]=(unsigned char)(i%127+j);memcpy(transaction,source,16);COUNT(2,16);for(size_t j=0;j<16;j++){assert(transaction[j]<255);transaction[j]++;}if(i%modulus!=0){memcpy(destination,transaction,16);COUNT(2,16);}uint64_t h=i;uint64_t events[]={1,2,3,4,i%modulus==0?6:5};for(int j=0;j<5;j++)h=h*131+events[j];for(int j=1;j>=0;j--){assert(!r[j].closed);r[j].closed=true;h=h*131+(r[j].id==2?7:8);}assert(r[0].closed&&r[1].closed);sum+=h^digest(destination,16);release(r);}}
    else abort();number(&out,sum);return out;
}
int main(int argc,char**argv){assert(argc==5||argc==6);FILE*f=fopen(argv[2],"rb");assert(f);assert(!fseek(f,0,SEEK_END));long size=ftell(f);assert(size>0);rewind(f);unsigned char*b=malloc((size_t)size+1);assert(b&&fread(b,1,(size_t)size,f)==(size_t)size);b[size]=0;assert(!fclose(f));char*end;size_t rounds=strtoull(argv[3],&end,10);assert(!*end&&rounds);bool emit=argc==6&&!strcmp(argv[5],"--emit");assert(!emit||rounds==1);
#ifdef ARENA
    assert(!strcmp(argv[4],"arena")&&!strcmp(argv[1],"binarytrees"));
#else
    assert(!strcmp(argv[4],"system")||!strcmp(argv[4],"mimalloc"));
#endif
    uint64_t checksum=0;
    for(size_t i=0;i<rounds;i++){Buffer out=run_case(argv[1],b,(size_t)size);checksum+=digest(out.b,out.len);if(emit)assert(fwrite(out.b,1,out.len,stdout)==out.len);release(out.b);}if(!emit)printf("%016"PRIx64"\n",checksum);
#ifdef STATS
    fprintf(stderr,"{\"allocation_requests\":%"PRIu64",\"requested_bytes\":%"PRIu64",\"copied_bytes\":%"PRIu64",\"release_requests\":%"PRIu64",\"count_operations\":%"PRIu64",\"reuse_hits\":%"PRIu64",\"reuse_misses\":%"PRIu64",\"arena_reserved_bytes_sum\":%"PRIu64",\"arena_peak_reserved_bytes\":%"PRIu64"}\n",counters[0],counters[1],counters[2],counters[3],counters[4],counters[5],counters[6],counters[7],counters[8]);
#endif
    free(b);return 0;
}
