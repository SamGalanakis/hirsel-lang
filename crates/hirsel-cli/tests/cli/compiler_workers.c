/* Linux pthread_create fault injection. Deliberately header-free so the test
 * uses the pinned LLVM without depending on host C development headers. Only
 * children of hirsel-compiler are refused; no host limits or runtime workers
 * are touched. The first compiler stack is always allowed. */
extern void *dlsym(void *, const char *);
extern char *getenv(const char *);
extern unsigned long pthread_self(void);
extern int pthread_getname_np(unsigned long, char *, unsigned long);

int pthread_create(void *thread, const void *attr,
                   void *(*start)(void *), void *arg) {
    static unsigned attempts;
    int (*real_create)(void *, const void *, void *(*)(void *), void *) =
        dlsym((void *)-1, "pthread_create");
    const char *after = getenv("HIRSEL_TEST_SPAWN_FAIL_AFTER");
    char name[16] = {0};
    pthread_getname_np(pthread_self(), name, sizeof name);
    const char compiler[] = "hirsel-compiler";
    unsigned i = 0;
    while (name[i] == compiler[i] && compiler[i]) ++i;
    if (after && name[i] == compiler[i] &&
        __atomic_fetch_add(&attempts, 1, __ATOMIC_RELAXED) >= (unsigned)(after[0] - '0')) {
        return 11; /* EAGAIN */
    }
    return real_create(thread, attr, start, arg);
}
