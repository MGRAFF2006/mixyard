/* Independent PRIVATE consuming app. Copies real graph PCM + clock frames into
 * a bounded SPSC queue; only the main loop writes the external capture file. */
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pipewire/pipewire.h>
#include <pipewire/filter.h>
#define FRAMES 2048u
#define SLOTS 64u
struct packet { uint64_t position, nsec; uint32_t frames, clock, rate_num, rate_denom, flags; float pcm[FRAMES * 2]; };
struct capture { struct pw_main_loop *loop; struct pw_filter *filter; void *ports[2];
    FILE *file; struct packet packets[SLOTS]; _Atomic unsigned read, write;
    _Atomic uint64_t drops; uint64_t recorded; bool error; };
static void process(void *data, struct spa_io_position *position) {
    struct capture *d = data; unsigned w = atomic_load(&d->write), r = atomic_load_explicit(&d->read, memory_order_acquire);
    uint32_t n = position->clock.duration;
    if (n > FRAMES || w - r == SLOTS) { atomic_fetch_add(&d->drops, n); return; }
    struct packet *p = &d->packets[w % SLOTS]; p->position = position->clock.position;
    p->nsec = position->clock.nsec; p->flags = position->clock.flags; p->frames = n; p->clock = position->clock.id; p->rate_num = position->clock.rate.num; p->rate_denom = position->clock.rate.denom;
    float *in[2]; for (unsigned c = 0; c < 2; c++) in[c] = pw_filter_get_dsp_buffer(d->ports[c], n);
    for (unsigned f = 0; f < n; f++) for (unsigned c = 0; c < 2; c++) p->pcm[f * 2 + c] = in[c] ? in[c][f] : 0;
    atomic_store_explicit(&d->write, w + 1, memory_order_release);
}
static void drain(void *data, uint64_t expirations) {
    (void)expirations; struct capture *d = data;
    unsigned r = atomic_load(&d->read), w = atomic_load_explicit(&d->write, memory_order_acquire);
    for (; r < w; r++) {
        struct packet *p = &d->packets[r % SLOTS];
        if (fwrite(p, offsetof(struct packet, pcm) + p->frames * 2 * sizeof(float), 1, d->file) != 1) {
            d->error = true; pw_main_loop_quit(d->loop); break;
        }
        d->recorded += p->frames;
    }
    atomic_store_explicit(&d->read, r, memory_order_release);
}
static void activate(void *data, int fd, uint32_t mask) {
    (void)fd; (void)mask; struct capture *d = data; char line[32];
    if (!fgets(line, sizeof(line), stdin)) { pw_main_loop_quit(d->loop); return; }
    if (!strcmp(line, "start\n") && pw_filter_set_active(d->filter, true) < 0) { d->error = true; pw_main_loop_quit(d->loop); }
}
static void quit(void *data, int sig) { (void)sig; pw_main_loop_quit(((struct capture *)data)->loop); }
static const struct pw_filter_events events = { PW_VERSION_FILTER_EVENTS, .process = process };
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    struct capture *d = calloc(1, sizeof(*d)); if (!d) return 2;
    atomic_init(&d->read, 0); atomic_init(&d->write, 0); atomic_init(&d->drops, 0);
    d->file = fopen(argv[2], "wb"); if (!d->file) return 2;
    sigset_t signals; sigemptyset(&signals); sigaddset(&signals, SIGTERM); sigaddset(&signals, SIGINT);
    if (sigprocmask(SIG_BLOCK, &signals, NULL) < 0) return 2;
    pw_init(&argc, &argv); d->loop = pw_main_loop_new(NULL); struct pw_loop *loop = pw_main_loop_get_loop(d->loop);
    d->filter = pw_filter_new_simple(loop, argv[1], pw_properties_new(PW_KEY_NODE_NAME, argv[1],
        PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY, "Capture", PW_KEY_MEDIA_ROLE, "DSP",
        PW_KEY_NODE_AUTOCONNECT, "false", NULL), &events, d);
    for (unsigned c = 0; c < 2; c++) d->ports[c] = pw_filter_add_port(d->filter, PW_DIRECTION_INPUT,
        PW_FILTER_PORT_FLAG_MAP_BUFFERS, 1, pw_properties_new(PW_KEY_FORMAT_DSP, "32 bit float mono audio",
            PW_KEY_PORT_NAME, c ? "in_FR" : "in_FL", PW_KEY_AUDIO_CHANNEL, c ? "FR" : "FL", NULL), NULL, 0);
    if (!d->ports[0] || !d->ports[1] || pw_filter_connect(d->filter, PW_FILTER_FLAG_RT_PROCESS | PW_FILTER_FLAG_INACTIVE, NULL, 0) < 0) return 2;
    setvbuf(stdin, NULL, _IONBF, 0); pw_loop_add_io(loop, 0, SPA_IO_IN | SPA_IO_HUP, false, activate, d);
    pw_loop_add_signal(loop, SIGTERM, quit, d); pw_loop_add_signal(loop, SIGINT, quit, d);
    struct spa_source *timer = pw_loop_add_timer(loop, drain, d);
    struct timespec interval = {.tv_nsec = 20000000}; pw_loop_update_timer(loop, timer, &interval, &interval, false);
    pw_main_loop_run(d->loop); pw_filter_destroy(d->filter); drain(d, 0);
    if (fclose(d->file)) d->error = true;
    printf("capture frames=%llu dropped=%llu write_error=%d\n", (unsigned long long)d->recorded,
        (unsigned long long)atomic_load(&d->drops), d->error);
    bool failure = d->error || atomic_load(&d->drops); pw_main_loop_destroy(d->loop); pw_deinit(); free(d); return failure ? 1 : 0;
}
