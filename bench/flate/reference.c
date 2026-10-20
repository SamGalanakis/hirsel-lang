#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include "zlib.h"
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    FILE *f = fopen(argv[1], "rb");
    if (!f || fseek(f, 0, SEEK_END)) return 2;
    long size = ftell(f);
    rewind(f);
    unsigned char *input = malloc(size), *output = malloc(compressBound(size));
    if (!input || !output || fread(input, 1, size, f) != (size_t)size) return 2;
    fclose(f);
    uint64_t sum = 0;
    for (int i = 0; i < 20; ++i) {
        z_stream s = {0};
        if (deflateInit2(&s, atoi(argv[2]), Z_DEFLATED, -15, 8, Z_DEFAULT_STRATEGY) != Z_OK) return 2;
        s.next_in = input; s.avail_in = size;
        s.next_out = output; s.avail_out = compressBound(size);
        if (deflate(&s, Z_FINISH) != Z_STREAM_END) return 2;
        sum += crc32(0, output, s.total_out);
        if (deflateEnd(&s) != Z_OK) return 2;
    }
    printf("%llu\n", (unsigned long long)sum);
    free(input); free(output);
    return 0;
}
