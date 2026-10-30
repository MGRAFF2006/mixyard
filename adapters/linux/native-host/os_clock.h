// SPDX-License-Identifier: MIT
#ifndef MIXYARD_NATIVE_OS_CLOCK_H
#define MIXYARD_NATIVE_OS_CLOCK_H
#include <stdbool.h>
#include <stdint.h>
#include <sys/stat.h>
#include <time.h>
// Diagnostic OS ordering only. Never a PipeWire/sample/hardware clock mapping.
static inline bool mx_os_monotonic_ns(uint64_t *out) {
    struct timespec now;
    *out = 0;
    if (clock_gettime(CLOCK_MONOTONIC, &now) || now.tv_sec < 0 || now.tv_nsec < 0 ||
        now.tv_nsec >= 1000000000 ||
        (uint64_t)now.tv_sec > (UINT64_MAX - (uint64_t)now.tv_nsec) / 1000000000) return false;
    *out = (uint64_t)now.tv_sec * 1000000000 + (uint64_t)now.tv_nsec;
    return true;
}
static inline bool mx_os_time_namespace(uint64_t *out) {
    struct stat state;
    *out = 0;
    if (stat("/proc/self/ns/time", &state)) return false;
    *out = (uint64_t)state.st_ino;
    return *out != 0;
}
#endif
