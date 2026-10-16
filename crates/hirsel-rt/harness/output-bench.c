#include "stdlib/native/library.h"
#include <unistd.h>
#include <stdint.h>
#include <string.h>
#include <signal.h>
#include <time.h>
#include <stdio.h>
static struct { hirsel_rt_v2_header header; uint64_t length; char bytes[64]; } text = {{UINT32_MAX,0},64,"012345678901234567890123456789012345678901234567890123456789012\n"};
static uint8_t entry(hirsel_rt_v2_ctx *ctx,uint8_t *dst,uint8_t *out,const uint64_t *io,const uint64_t *args) {
  uint64_t stdout; hirsel_rt_v2_unit_result result;
  hirsel_rt_v2_io_stdout(ctx,0,&stdout,io);
  for(unsigned i=0;i<200000;i++) {
    hirsel_rt_v2_out_print(ctx,0,&result,&stdout,&text.header,0,64);
    if(result.tag) return 1;
  }
  memset(out,0,4); return 0;
}
int main(int argc,char **argv) {
  struct timespec begin, end;
  clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &begin);
  int status = 0;
  signal(SIGPIPE,SIG_IGN);
  if(argc>1 && strcmp(argv[1],"c")==0) {
    for(unsigned i=0;i<200000;i++) if(write(1,text.bytes,64)!=64) return 1;
  } else {
    hirsel_rt_v2_launch_config config = {0};
    status = hirsel_rt_v2_launch(entry,4,0,0,&config);
  }
  clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &end);
  double seconds = (end.tv_sec - begin.tv_sec) + (end.tv_nsec - begin.tv_nsec) / 1e9;
  fprintf(stderr, "%.9f\n", seconds);
  return status;
}
