#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

extern size_t ZSTD_decompress(void *, size_t, const void *, size_t);
extern unsigned ZSTD_isError(size_t);
extern uint64_t XXH64(const void *, size_t, uint64_t);

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    FILE *file = fopen(argv[1], "rb");
    if (!file) return 2;
    if (fseek(file, 0, SEEK_END)) return 2;
    long length = ftell(file);
    if (length < 0 || fseek(file, 0, SEEK_SET)) return 2;
    size_t n = (size_t)length;
    char *src = malloc(n);
    if (!src || fread(src, 1, n, file) != n) return 3;
    fclose(file);
    uint64_t sum = 0;
    for (int i = 0; i < 10; i++) {
        unsigned char *out = malloc(20000000);
        if (!out) return 3;
        size_t size = ZSTD_decompress(out, 20000000, src, n);
        if (ZSTD_isError(size) || !size) return 4;
        if (!i) sum = XXH64(out, size, 0);
        sum += size + out[0] + out[size - 1];
        free(out);
    }
    printf("%llu\n", (unsigned long long)sum);
    free(src);
}
