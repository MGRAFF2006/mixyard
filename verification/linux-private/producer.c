/* SPDX-License-Identifier: MIT. Deterministic PRIVATE PCM fixture, not a microphone. */
#include <math.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <stdatomic.h>
#include <pipewire/pipewire.h>
#include <pipewire/filter.h>
#define TRACE_SLOTS 32768u
struct tick { uint64_t position, nsec, wakeup; uint32_t duration, flags; };
struct producer { struct pw_main_loop *loop; struct pw_filter *filter; void *ports[2]; float period[2][480]; struct tick *trace; _Atomic unsigned traced, overflow; };
static void process(void *data, struct spa_io_position *p) {
    struct producer *d = data;
    unsigned index = atomic_load(&d->traced);
    if (index < TRACE_SLOTS) {
        struct timespec now; clock_gettime(CLOCK_MONOTONIC, &now);
        d->trace[index] = (struct tick){.position = p->clock.position, .nsec = p->clock.nsec,
            .wakeup = (uint64_t)now.tv_sec * 1000000000u + now.tv_nsec,
            .duration = p->clock.duration, .flags = p->clock.flags};
        atomic_store(&d->traced, index + 1);
    } else atomic_fetch_add(&d->overflow, 1);
    for (unsigned c = 0; c < 2; c++) {
        float *out = pw_filter_get_dsp_buffer(d->ports[c], p->clock.duration);
        if (out) for (unsigned f = 0; f < p->clock.duration; f++) out[f] = d->period[c][(p->clock.position + f) % 480];
    }
}
static const struct pw_filter_events events = { PW_VERSION_FILTER_EVENTS, .process = process };
static void quit(void *data, int sig) { (void)sig; pw_main_loop_quit(((struct producer *)data)->loop); }
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    struct producer d = {0}; d.trace = calloc(TRACE_SLOTS, sizeof(*d.trace));
    if (!d.trace) return 2;
    atomic_init(&d.traced, 0); atomic_init(&d.overflow, 0);
    for (unsigned c = 0; c < 2; c++) {
        char *end; double hz = strtod(argv[c + 2], &end);
        if (*end || !isfinite(hz) || hz < 100 || hz > 10000 || fmod(hz, 100)) return 2;
        for (unsigned f = 0; f < 480; f++) d.period[c][f] = .08 * sin(2 * M_PI * hz * f / 48000);
    }
    sigset_t signals; sigemptyset(&signals); sigaddset(&signals, SIGTERM); sigaddset(&signals, SIGINT);
    if (sigprocmask(SIG_BLOCK, &signals, NULL) < 0) return 2;
    pw_init(&argc, &argv); d.loop = pw_main_loop_new(NULL);
    struct pw_loop *loop = pw_main_loop_get_loop(d.loop);
    d.filter = pw_filter_new_simple(loop, argv[1], pw_properties_new(PW_KEY_NODE_NAME, argv[1],
        PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY, "Playback", PW_KEY_MEDIA_ROLE, "DSP",
        PW_KEY_NODE_AUTOCONNECT, "false", PW_KEY_NODE_ALWAYS_PROCESS, "true", NULL), &events, &d);
    for (unsigned c = 0; c < 2; c++) d.ports[c] = pw_filter_add_port(d.filter, PW_DIRECTION_OUTPUT,
        PW_FILTER_PORT_FLAG_MAP_BUFFERS, 1, pw_properties_new(PW_KEY_FORMAT_DSP, "32 bit float mono audio",
            PW_KEY_PORT_NAME, c ? "out_FR" : "out_FL", PW_KEY_AUDIO_CHANNEL, c ? "FR" : "FL", NULL), NULL, 0);
    if (!d.ports[0] || !d.ports[1] || pw_filter_connect(d.filter, PW_FILTER_FLAG_RT_PROCESS, NULL, 0) < 0) return 2;
    pw_loop_add_signal(loop, SIGINT, quit, &d); pw_loop_add_signal(loop, SIGTERM, quit, &d);
    pw_main_loop_run(d.loop); pw_filter_destroy(d.filter);
    for (unsigned i = 0; i < atomic_load(&d.traced); i++) {
        struct tick *t = &d.trace[i]; printf("producer-frame %llu %llu %llu %u %u\n",
            (unsigned long long)t->position, (unsigned long long)t->nsec, (unsigned long long)t->wakeup, t->duration, t->flags);
    }
    printf("producer-trace-overflow %u\n", atomic_load(&d.overflow));
    bool failed = atomic_load(&d.overflow) != 0; free(d.trace);
    pw_main_loop_destroy(d.loop); pw_deinit(); return failed ? 1 : 0;
}
