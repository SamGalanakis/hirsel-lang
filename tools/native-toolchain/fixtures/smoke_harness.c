/* C side of the toolchain smoke: calls every function in smoke.ll at its boundary values
 * (C to object) and defines smoke_host_scale for it to call (object to C). It prints one
 * line per case; smoke.expected is the exact output on every target. */
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct __attribute__((packed)) {
  uint8_t tag;
  int64_t value;
  uint16_t extra;
} Packed;

_Static_assert(sizeof(Packed) == 11, "the packed struct has no padding");

uint8_t smoke_add_i64(int64_t *out, int64_t a, int64_t b);
uint8_t smoke_div_i64(int64_t *out, int64_t a, int64_t b);
uint8_t smoke_shl_u64(uint64_t *out, uint64_t a, uint64_t n);
uint8_t smoke_mul_i128(__int128 *out, const __int128 *a, const __int128 *b);
uint8_t smoke_packed_sum(int64_t *out, const Packed *p);
uint8_t smoke_call_host(int64_t *out, int64_t a);
double smoke_mul_add(double a, double b, double c);
double smoke_mul_then_add(double a, double b, double c);
double smoke_div_f64(double a, double b);

/* Saturating triple, so the object's checked add after the call can overflow. */
int64_t smoke_host_scale(int64_t a) { return a > INT64_MAX / 3 ? INT64_MAX : a * 3; }

static void i64_case(const char *name, uint8_t status, int64_t value) {
  if (status == 0)
    printf("%s ok %" PRId64 "\n", name, value);
  else
    printf("%s fail %u\n", name, (unsigned)status);
}

static void i128_case(const char *name, uint8_t status, __int128 value) {
  unsigned __int128 bits = (unsigned __int128)value;
  if (status == 0)
    printf("%s ok 0x%016" PRIx64 "%016" PRIx64 "\n", name, (uint64_t)(bits >> 64), (uint64_t)bits);
  else
    printf("%s fail %u\n", name, (unsigned)status);
}

static void f64_case(const char *name, double value) {
  uint64_t bits;
  memcpy(&bits, &value, sizeof bits);
  printf("%s 0x%016" PRIx64 "\n", name, bits);
}

int main(void) {
  int64_t r = 0;
  uint64_t u = 0;
  __int128 w = 0;

  i64_case("add max-1+1", smoke_add_i64(&r, INT64_MAX - 1, 1), r);
  i64_case("add max+1", smoke_add_i64(&r, INT64_MAX, 1), r);
  i64_case("add min+-1", smoke_add_i64(&r, INT64_MIN, -1), r);
  i64_case("add min+max", smoke_add_i64(&r, INT64_MIN, INT64_MAX), r);

  i64_case("div 7/-2", smoke_div_i64(&r, 7, -2), r);
  i64_case("div min/-1", smoke_div_i64(&r, INT64_MIN, -1), r);
  i64_case("div 1/0", smoke_div_i64(&r, 1, 0), r);
  i64_case("div min/1", smoke_div_i64(&r, INT64_MIN, 1), r);

  i64_case("shl 1<<63", smoke_shl_u64(&u, 1, 63), (int64_t)u);
  i64_case("shl 1<<64", smoke_shl_u64(&u, 1, 64), (int64_t)u);
  i64_case("shl 1<<max", smoke_shl_u64(&u, 1, UINT64_MAX), (int64_t)u);

  __int128 two_63 = (__int128)1 << 63;
  __int128 max128 = (__int128)(((unsigned __int128)1 << 127) - 1);
  __int128 a = two_63, b = two_63;
  i128_case("mul 2^63*2^63", smoke_mul_i128(&w, &a, &b), w);
  a = max128, b = -1;
  i128_case("mul max*-1", smoke_mul_i128(&w, &a, &b), w);
  a = (__int128)1 << 64, b = (__int128)1 << 63;
  i128_case("mul 2^64*2^63", smoke_mul_i128(&w, &a, &b), w);
  a = -((__int128)1 << 64), b = (__int128)1 << 63;
  i128_case("mul -2^64*2^63", smoke_mul_i128(&w, &a, &b), w);

  /* One byte before the struct makes its i64 field unaligned in memory. */
  unsigned char storage[1 + sizeof(Packed)];
  Packed *p = (Packed *)(storage + 1);
  Packed fields = {.tag = 0xff, .value = INT64_MAX - 0x100fe, .extra = 0xffff};
  memcpy(p, &fields, sizeof fields);
  i64_case("packed near-max", smoke_packed_sum(&r, p), r);
  fields.value += 1;
  memcpy(p, &fields, sizeof fields);
  i64_case("packed overflow", smoke_packed_sum(&r, p), r);

  i64_case("host 5", smoke_call_host(&r, 5), r);
  i64_case("host max/3", smoke_call_host(&r, INT64_MAX / 3), r);
  i64_case("host max", smoke_call_host(&r, INT64_MAX), r);

  double e = 0x1p-30, one_e = 1.0 + e;
  f64_case("mul_add", smoke_mul_add(one_e, one_e, -(1.0 + 2 * e)));
  f64_case("mul_then_add", smoke_mul_then_add(one_e, one_e, -(1.0 + 2 * e)));
  f64_case("div subnormal", smoke_div_f64(0x1p-1022, 4.0));
  return 0;
}
