/* Diagnostic LD_PRELOAD companion for rss. glibc's public mallinfo2 interface
 * (04e750e7, malloc/malloc.c:4878-4936, https://sourceware.org/git/glibc.git)
 * owns the accounting;
 * these are allocator bytes, never claimed to be resident pages. Sample before
 * blocking readiness waits, outside allocator calls. Never use for timings. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <malloc.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/epoll.h>
#include <unistd.h>

static int (*real_poll)(struct pollfd *, nfds_t, int);
static int (*real_epoll_wait)(int, struct epoll_event *, int, int);

__attribute__((constructor)) static void init(void) {
    real_poll = dlsym(RTLD_NEXT, "poll");
    real_epoll_wait = dlsym(RTLD_NEXT, "epoll_wait");
}

static void snapshot(void) {
    /* Optional attribution experiment: the delta is libc-reclaimable RSS. */
    if (getenv("RSS_MALLOC_TRIM")) (void)malloc_trim(0);
    struct mallinfo2 m = mallinfo2();
    char line[256];
    int length = snprintf(line, sizeof line,
        "rss-malloc pid=%d tid=%d arena=%zu allocated=%zu free=%zu mmap=%zu top=%zu\n",
        getpid(), gettid(), m.arena, m.uordblks, m.fordblks, m.hblkhd, m.keepcost);
    if (length > 0 && length < (int)sizeof line)
        (void)write(STDERR_FILENO, line, (size_t)length);
}

int poll(struct pollfd *fds, nfds_t count, int timeout) {
    if (timeout != 0) snapshot();
    return real_poll(fds, count, timeout);
}

int epoll_wait(int fd, struct epoll_event *events, int count, int timeout) {
    if (timeout != 0) snapshot();
    return real_epoll_wait(fd, events, count, timeout);
}
