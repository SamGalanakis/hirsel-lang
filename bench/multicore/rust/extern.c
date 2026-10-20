#include <stdint.h>
// Separate translation unit, without LTO: one opaque, bounded foreign call.
uint64_t b1_identity(uint64_t value) { return value; }
