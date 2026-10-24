/* A freestanding glibc audit interface for explicit frozen DSO names.
 * glibc 2.28 elf/dl-load.c:1985-2019 defines la_objsearch's redirect/refusal;
 * elf/rtld.c:597-604 and elf/dl-init.c:53-78 pass the initial environment to
 * audit DSO constructors before la_version. No libc or target DSO is needed.
 *
 * The caller supplies HIRSEL_RUNTIME_MAP and its exact decimal byte length in
 * HIRSEL_RUNTIME_MAP_LENGTH. Format: R (closed replay) or C (discovery), then
 * byte-sorted request\0absolute-snapshot-path\0 pairs. Discovery additionally
 * supplies a new HIRSEL_RUNTIME_EVENTS path and HIRSEL_RUNTIME_EVENTS_LIMIT.
 * Events are S request\0 or O opened-path\0. All bounds come from the caller.
 * Runtime input derivation and ordinary-store admission belong to the host.
 */
typedef unsigned long word;
#if defined(__x86_64__)
static long call(long n, long a, long b, long c, long d, long e, long f) {
    register long r10 __asm__("r10") = d;
    register long r8 __asm__("r8") = e;
    register long r9 __asm__("r9") = f;
    long result;
    __asm__ volatile("syscall" : "=a"(result)
                     : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9)
                     : "rcx", "r11", "memory");
    return result;
}
enum { WRITE = 1, CLOSE = 3, LSEEK = 8, MMAP = 9, EXIT_GROUP = 231, OPENAT = 257 };
#elif defined(__aarch64__)
static long call(long n, long a, long b, long c, long d, long e, long f) {
    register long x8 __asm__("x8") = n;
    register long x0 __asm__("x0") = a;
    register long x1 __asm__("x1") = b;
    register long x2 __asm__("x2") = c;
    register long x3 __asm__("x3") = d;
    register long x4 __asm__("x4") = e;
    register long x5 __asm__("x5") = f;
    __asm__ volatile("svc 0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5) : "memory", "cc");
    return x0;
}
enum { WRITE = 64, CLOSE = 57, LSEEK = 62, MMAP = 222, EXIT_GROUP = 94, OPENAT = 56 };
#else
#error unsupported runtime auditor target
#endif

static const char *pairs, *end;
static int discovery;
static long events = -1;
static word remaining;

static word length(const char *s) { word n = 0; while (s[n]) ++n; return n; }
static int compare(const char *a, const char *b) {
    while (*a && (unsigned char)*a == (unsigned char)*b) { ++a; ++b; }
    return (unsigned char)*a - (unsigned char)*b;
}
static void write_all(long fd, const char *s, word n) {
    while (n) {
        long written = call(WRITE, fd, (long)s, n, 0, 0, 0);
        if (written == -4) continue;
        if (written <= 0 || (word)written > n) {
            call(EXIT_GROUP, 127, 0, 0, 0, 0, 0);
            __builtin_unreachable();
        }
        s += written; n -= written;
    }
}
__attribute__((noreturn)) static void refuse(void) {
    const char message[] = "hirsel: invalid or missing frozen runtime inputs\n";
    write_all(2, message, sizeof(message) - 1);
    call(EXIT_GROUP, 127, 0, 0, 0, 0, 0);
    __builtin_unreachable();
}
static const char *variable(char **env, const char *key) {
    for (; *env; ++env) {
        const char *a = *env, *b = key;
        while (*b && *a == *b) { ++a; ++b; }
        if (!*b && *a == '=') return a + 1;
    }
    return (const char *)0;
}
static word number(const char *s) {
    if (!s || !*s || (*s == '0' && s[1])) refuse();
    word n = 0;
    for (; *s; ++s) {
        if (*s < '0' || *s > '9' || n > (0x7fffffffffffffffUL - (*s - '0')) / 10) refuse();
        n = n * 10 + (*s - '0');
    }
    return n;
}
static const char *next(const char *s) {
    const char *p = s;
    while (p < end && *p) ++p;
    if (p == end) refuse();
    return p + 1;
}
__attribute__((visibility("hidden")))
void hirsel_audit_init(int argc, char **argv, char **env) {
    (void)argc; (void)argv;
    const char *path = variable(env, "HIRSEL_RUNTIME_MAP");
    word size = number(variable(env, "HIRSEL_RUNTIME_MAP_LENGTH"));
    if (!path || path[0] != '/' || !size) refuse();
    long fd = call(OPENAT, -100, (long)path, 0x80000, 0, 0, 0);
    if (fd < 0) refuse();
    long actual = call(LSEEK, fd, 0, 2, 0, 0, 0);
    if (actual < 0 || (word)actual != size) refuse();
    long mapped = call(MMAP, 0, size, 1, 2, fd, 0);
    if ((word)mapped >= (word)-4095L) refuse();
    call(CLOSE, fd, 0, 0, 0, 0, 0);
    const char *bytes = (const char *)mapped;
    if (*bytes != 'C' && *bytes != 'R') refuse();
    discovery = *bytes == 'C';
    pairs = bytes + 1; end = bytes + size;
    const char *previous = (const char *)0;
    for (const char *p = pairs; p < end;) {
        const char *request = p;
        p = next(p);
        if (!*request || p == end || *p != '/' || (previous && compare(previous, request) >= 0)) refuse();
        previous = request;
        p = next(p);
    }
    if (discovery) {
        path = variable(env, "HIRSEL_RUNTIME_EVENTS");
        remaining = number(variable(env, "HIRSEL_RUNTIME_EVENTS_LIMIT"));
        if (!path || path[0] != '/' || !remaining) refuse();
        events = call(OPENAT, -100, (long)path, 0x80000 | 1 | 64 | 128, 0600, 0, 0);
        if (events < 0) refuse();
    }
}
static void event(char tag, const char *name) {
    if (!discovery) return;
    word n = length(name) + 1;
    if (n >= remaining) refuse();
    remaining -= n + 1;
    write_all(events, &tag, 1); write_all(events, name, n);
}
unsigned int la_version(unsigned int version) {
    if (!pairs) refuse();
    return version < 1 ? version : 1;
}
char *la_objsearch(const char *name, word *cookie, unsigned int flag) {
    (void)cookie;
    if (flag == 1) event('S', name);
    for (const char *p = pairs; p < end;) {
        const char *request = p;
        const char *snapshot = next(p);
        p = next(snapshot);
        if (!compare(name, request)) return (char *)snapshot;
    }
    if (discovery) return (char *)name;
    const char message[] = "hirsel: uncaptured runtime library: ";
    write_all(2, message, sizeof(message) - 1);
    write_all(2, name, length(name)); write_all(2, "\n", 1);
    return (char *)0;
}
/* The public prefix of glibc's link_map, elf/link.h:95-107. */
struct link_map { word address; char *name; };
unsigned int la_objopen(struct link_map *map, long namespace_id, word *cookie) {
    (void)namespace_id; (void)cookie;
    event('O', map->name);
    return 0;
}
