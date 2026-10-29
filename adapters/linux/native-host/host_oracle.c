// SPDX-License-Identifier: MIT
// Explicit clock injection and real host linkage; this never loads PipeWire.
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "timeline.h"

static bool callback;
static size_t heap_ops;
void *__real_malloc(size_t); void *__real_calloc(size_t, size_t);
void *__real_realloc(void *, size_t); void __real_free(void *);
void *__wrap_malloc(size_t n) { if (callback) ++heap_ops; return __real_malloc(n); }
void *__wrap_calloc(size_t n, size_t s) { if (callback) ++heap_ops; return __real_calloc(n, s); }
void *__wrap_realloc(void *p, size_t n) { if (callback) ++heap_ops; return __real_realloc(p, n); }
void __wrap_free(void *p) { if (callback) ++heap_ops; __real_free(p); }

int main(int argc, char **argv) {
    assert(argc == 2);
    char text[32768], error[1024];
    FILE *file = fopen(argv[1], "r"); assert(file);
    size_t len = fread(text, 1, sizeof(text), file); assert(len < sizeof(text)); assert(!ferror(file)); fclose(file);
    mx_host *host = NULL;
    int rc = mx_host_create((uint8_t *)text, len, &host, error, sizeof(error));
    if (rc) { fprintf(stderr, "create %d: %s\n", rc, error); return 1; }
    assert(mx_host_source_count(host) == 3 && mx_host_output_count(host) == 3);
    mx_graph_timeline timeline = {.clock_id = 42};
    double pcm[3][256];
    for (size_t f = 0; f < 128; ++f) {
        pcm[0][2*f] = 0.25 + 0x1p-42; pcm[0][2*f+1] = -0.125;
        pcm[1][2*f] = 0.5; pcm[1][2*f+1] = 0.375;
        pcm[2][2*f] = 0.875; pcm[2][2*f+1] = 0.875;
    }
    for (uint64_t seq = 0; seq < 512; ++seq) {
        mx_stamp stamp;
        uint64_t now = 1000000000 + seq * 2666667;
        assert(mx_graph_stamp(&timeline, 42, 987654 + 128*seq, now, 128, 1, &stamp));
        assert(stamp.sequence == seq && stamp.first_frame == 128*seq && stamp.captured_ns == now);
        mx_input inputs[3];
        for (size_t s = 0; s < 3; ++s) inputs[s] = (mx_input){ (uint8_t *)pcm[s], sizeof(pcm[s]), MX_F64LE, 48000, 2, stamp };
        callback = true;
        assert(mx_host_process(host, now, stamp, 128, inputs, 3) == MX_OK);
        for (size_t r = 0; r < 3; ++r) {
            const double *out = NULL; size_t count = 0;
            assert(mx_host_output(host, r, &out, &count) == MX_OK && count == 256);
            for (size_t f = 0; f < 128; ++f) for (size_t ch = 0; ch < 2; ++ch) {
                double a = pcm[0][2*f+ch];
                double expected = r == 0 ? a : r == 1 ? a * 0.25 + pcm[1][2*f+ch] * 0.125 : a * 0.5;
                assert(out[2*f+ch] == expected);
            }
        }
        callback = false;
    }
    mx_stamp ignored;
    assert(!mx_graph_stamp(&timeline, 43, timeline.next_native_frame, 3000000000, 128, 1, &ignored));
    assert(!mx_graph_stamp(&timeline, 42, timeline.next_native_frame+128, 3000000000, 128, 1, &ignored));
    callback = true; mx_host_revoke(host);
    const double *out = (double *)1; size_t count = 1;
    assert(mx_host_output(host, 0, &out, &count) == MX_INACTIVE && out == NULL && count == 0);
    callback = false;
    assert(heap_ops == 0);
    mx_host_destroy(host);
    puts("PASS linked canonical host: 512 Float64/DSP/independent-output blocks, explicit native frame epoch, rejected clock/gap, revoke, zero callback heap operations");
}
