// SPDX-License-Identifier: MIT
// Actual Linux pthread inheritance and signalfd delivery; no PipeWire/SDK/device.
#define _GNU_SOURCE
#include <assert.h>
#include <poll.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/signalfd.h>
#include <unistd.h>
#include "stop_signals.h"
struct worker {
    pthread_mutex_t lock;
    pthread_cond_t changed;
    bool ready, fire, sent;
    int signal, int_blocked, term_blocked;
};
static void *send_stop(void *userdata) {
    struct worker *w = userdata;
    sigset_t inherited;
    assert(pthread_sigmask(SIG_SETMASK, NULL, &inherited) == 0);
    assert(pthread_mutex_lock(&w->lock) == 0);
    w->int_blocked = sigismember(&inherited, SIGINT);
    w->term_blocked = sigismember(&inherited, SIGTERM);
    w->ready = true;
    assert(pthread_cond_broadcast(&w->changed) == 0);
    while (!w->fire) assert(pthread_cond_wait(&w->changed, &w->lock) == 0);
    assert(pthread_mutex_unlock(&w->lock) == 0);
    // Process-directed signal, as in the native supervisor. An unmasked sender
    // receives its default action before it can notify the owner of completion.
    assert(kill(getpid(), w->signal) == 0);
    assert(pthread_mutex_lock(&w->lock) == 0);
    w->sent = true;
    assert(pthread_cond_broadcast(&w->changed) == 0);
    assert(pthread_mutex_unlock(&w->lock) == 0);
    return NULL;
}
int main(int argc, char **argv) {
    assert(argc == 3);
    bool early = !strcmp(argv[1], "early");
    assert(early || !strcmp(argv[1], "late"));
    int signal = !strcmp(argv[2], "TERM") ? SIGTERM : SIGINT;
    assert(!strcmp(argv[2], "TERM") || !strcmp(argv[2], "INT"));
    sigset_t signals;
    assert(sigemptyset(&signals) == 0);
    assert(sigaddset(&signals, SIGINT) == 0 && sigaddset(&signals, SIGTERM) == 0);
    struct sigaction defaults = { .sa_handler = SIG_DFL };
    assert(sigemptyset(&defaults.sa_mask) == 0);
    assert(sigaction(SIGINT, &defaults, NULL) == 0 && sigaction(SIGTERM, &defaults, NULL) == 0);
    assert(pthread_sigmask(SIG_UNBLOCK, &signals, NULL) == 0);
    if (early) assert(mx_native_block_stop_signals() == 0);
    struct worker w = { .lock = PTHREAD_MUTEX_INITIALIZER,
        .changed = PTHREAD_COND_INITIALIZER, .signal = signal };
    pthread_t thread;
    assert(pthread_create(&thread, NULL, send_stop, &w) == 0);
    assert(pthread_mutex_lock(&w.lock) == 0);
    while (!w.ready) assert(pthread_cond_wait(&w.changed, &w.lock) == 0);
    if (!early) assert(mx_native_block_stop_signals() == 0);
    sigset_t owner;
    assert(pthread_sigmask(SIG_SETMASK, NULL, &owner) == 0);
    assert(sigismember(&owner, SIGINT) == 1 && sigismember(&owner, SIGTERM) == 1);
    assert(w.int_blocked == (early ? 1 : 0) && w.term_blocked == (early ? 1 : 0));
    int fd = signalfd(-1, &signals, SFD_CLOEXEC | SFD_NONBLOCK);
    assert(fd >= 0);
    printf("{\"workerIntBlocked\":%d,\"workerTermBlocked\":%d,\"ownerBlocked\":true}\n", w.int_blocked, w.term_blocked);
    assert(fflush(stdout) == 0);
    w.fire = true;
    assert(pthread_cond_broadcast(&w.changed) == 0);
    // Do not consume a pending signal ahead of the worker in the late-mask
    // control. It must actually survive sending before the owner can acknowledge.
    while (!w.sent) assert(pthread_cond_wait(&w.changed, &w.lock) == 0);
    assert(pthread_mutex_unlock(&w.lock) == 0);
    struct pollfd ready = { .fd = fd, .events = POLLIN };
    assert(poll(&ready, 1, 1000) == 1 && (ready.revents & POLLIN));
    struct signalfd_siginfo info;
    assert(read(fd, &info, sizeof(info)) == sizeof(info));
    assert(info.ssi_signo == (unsigned)signal && info.ssi_pid == (unsigned)getpid());
    assert(pthread_join(thread, NULL) == 0);
    assert(close(fd) == 0);
    assert(pthread_cond_destroy(&w.changed) == 0 && pthread_mutex_destroy(&w.lock) == 0);
    printf("{\"ownerAck\":true,\"workerJoined\":true,\"signal\":%d}\n", signal);
    return 0;
}
