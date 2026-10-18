#include <stdint.h>
#include <stddef.h>

static uint8_t byte;
typedef int32_t (*callback)(void *);
static int32_t answer(void *ctx) { (void)ctx; return 17; }
void *nullable_pointer(int32_t present) { return present ? &byte : NULL; }
void *required_pointer(int32_t present) { return nullable_pointer(present); }
callback nullable_function(int32_t present) { return present ? answer : NULL; }
callback required_function(int32_t present) { return nullable_function(present); }
int32_t pointer_present(void *value) { return value == &byte; }
int32_t function_value(void *ctx, callback value) { return value ? value(ctx) : -1; }
void nullable_out(int32_t present, void **pointer, callback *function) {
    *pointer = nullable_pointer(present);
    *function = nullable_function(present);
}
void required_out(int32_t present, void **pointer) { *pointer = nullable_pointer(present); }
void required_function_out(int32_t present, callback *function) { *function = nullable_function(present); }
int32_t nullable_visit(void *ctx, void *(*f)(void *, void *, callback)) {
    return f(ctx, NULL, NULL) == NULL && f(ctx, &byte, answer) == &byte;
}
void required_visit(void *ctx, void (*f)(void *, void *), int32_t present) { f(ctx, nullable_pointer(present)); }
void required_function_visit(void *ctx, void (*f)(void *, callback), int32_t present) { f(ctx, nullable_function(present)); }
void nested_visit(void *ctx, void (*f)(void *, void *)) { f(ctx, &byte); f(ctx, NULL); }
int32_t pointer_taken(void *value) { return pointer_present(value); }
int32_t function_taken(void *ctx, callback value) { return function_value(ctx, value); }
