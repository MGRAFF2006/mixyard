#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* Test-only process-local I/O fault. No canonical operation or reply is mocked. */
static int (*original_fsync)(int);
static pthread_once_t initialized = PTHREAD_ONCE_INIT;
static atomic_int claimed;
static void initialize(void) { original_fsync = dlsym(RTLD_NEXT, "fsync"); }

int fsync(int fd) {
    pthread_once(&initialized, initialize);
    if (!original_fsync) { errno = ENOSYS; return -1; }
    const char *expected = getenv("MIXYARD_CONTROL_TEST_PENDING");
    const char *gate_text = getenv("MIXYARD_CONTROL_TEST_GATE_FD");
    char thread[16], link[64], path[PATH_MAX];
    if (expected && gate_text &&
        pthread_getname_np(pthread_self(), thread, sizeof thread) == 0 &&
        strcmp(thread, "situation-owner") == 0) {
        snprintf(link, sizeof link, "/proc/self/fd/%d", fd);
        ssize_t n = readlink(link, path, sizeof path - 1);
        if (n >= 0) {
            path[n] = '\0';
            if (strcmp(path, expected) == 0 && atomic_exchange(&claimed, 1) == 0) {
                char *end;
                long gate = strtol(gate_text, &end, 10);
                char release = 0;
                if (*end || gate < 0 || gate > INT_MAX || write((int)gate, "F", 1) != 1 ||
                    read((int)gate, &release, 1) != 1 || release != 'R') {
                    errno = EIO;
                    return -1;
                }
            }
        }
    }
    return original_fsync(fd);
}
