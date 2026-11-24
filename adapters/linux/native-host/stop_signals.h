// SPDX-License-Identifier: MIT
#ifndef MIXYARD_NATIVE_STOP_SIGNALS_H
#define MIXYARD_NATIVE_STOP_SIGNALS_H
#include <errno.h>
#include <pthread.h>
#include <signal.h>
// Standalone process startup, BEFORE pw_init or any worker creation. PipeWire's
// signalfd registration masks only its calling thread; workers inherit this mask.
static inline int mx_native_block_stop_signals(void) {
    sigset_t signals;
    if (sigemptyset(&signals) || sigaddset(&signals, SIGINT) || sigaddset(&signals, SIGTERM))
        return errno;
    return pthread_sigmask(SIG_BLOCK, &signals, NULL);
}
#endif
