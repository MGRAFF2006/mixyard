// SPDX-License-Identifier: MIT
#include <assert.h>
#include <stdio.h>
#include "consumer_copy.h"
static struct mx_consumer_metadata metadata(uint64_t epoch, uint64_t sequence) {
    return (struct mx_consumer_metadata){
        .stamp = { .generation = 7, .revision = 1, .clock_id = 11,
            .sequence = sequence, .first_frame = sequence * 128,
            .captured_ns = 100 + sequence, .deadline_ns = 1000 + sequence },
        .consumer_epoch = epoch, .native_frame = 1000 + sequence * 128 };
}
static void cleared(const struct mx_consumer_copy *q) {
    const unsigned char *bytes = (const unsigned char *)q->slots;
    for (size_t i = 0; i < sizeof(q->slots); ++i) assert(bytes[i] == 0);
    assert(!q->active && !q->count);
}
int main(void) {
    struct mx_consumer_copy a = {0}, sibling = {0};
    assert(mx_consumer_open(&a, 1, 7, 11, 1000));
    assert(mx_consumer_open(&sibling, 1, 7, 11, 1000));
    double native[MX_CONSUMER_SAMPLES];
    for (size_t i = 0; i < MX_CONSUMER_SAMPLES; ++i) native[i] = i ? -0.0 : 0.25 + 0x1p-45;
    double expected[MX_CONSUMER_SAMPLES]; memcpy(expected, native, sizeof(expected));
    struct mx_consumer_metadata first = metadata(1, 0);
    assert(mx_consumer_push(&a, 101, &first, native, MX_CONSUMER_SAMPLES) == MX_COPY_OK);
    assert(mx_consumer_push(&sibling, 101, &first, native, MX_CONSUMER_SAMPLES) == MX_COPY_OK);
    memset(native, 0x7f, sizeof(native)); first.native_frame = 99;
    const struct mx_consumer_packet *packet = NULL;
    assert(mx_consumer_front(&a, 1, 7, 11, 102, &packet) == MX_COPY_OK);
    assert(packet->metadata.native_frame == 1000 && !memcmp(packet->pcm, expected, sizeof(expected)));
    assert(mx_consumer_pop(&a, 1));
    for (size_t i = 0; i < sizeof(a.slots[0]); ++i) assert(((unsigned char *)&a.slots[0])[i] == 0);
    assert(mx_consumer_open(&a, 2, 7, 11, 1000));
    for (uint64_t seq = 0; seq < MX_CONSUMER_SLOTS; ++seq) {
        struct mx_consumer_metadata m = metadata(2, seq);
        assert(mx_consumer_push(&a, 110, &m, expected, MX_CONSUMER_SAMPLES) == MX_COPY_OK);
    }
    struct mx_consumer_metadata full = metadata(2, MX_CONSUMER_SLOTS);
    assert(mx_consumer_push(&a, 110, &full, expected, MX_CONSUMER_SAMPLES) == MX_COPY_FULL);
    cleared(&a);
    assert(mx_consumer_front(&sibling, 1, 7, 11, 110, &packet) == MX_COPY_OK);
    assert(!memcmp(packet->pcm, expected, sizeof(expected)));
    assert(mx_consumer_open(&a, 3, 7, 11, 1000));
    assert(!mx_consumer_pop(&a, 2)); // Old consumer cannot discard replacement PCM.
    assert(!mx_consumer_open(&a, 3, 7, 11, 1000));
    first = metadata(2, 0);
    assert(mx_consumer_push(&a, 110, &first, expected, MX_CONSUMER_SAMPLES) == MX_COPY_STALE_BINDING && a.active);
    first = metadata(3, 0);
    assert(mx_consumer_push(&a, 110, &first, expected, MX_CONSUMER_SAMPLES) == MX_COPY_OK);
    assert(mx_consumer_front(&a, 3, 7, 11, 1001, &packet) == MX_COPY_EXPIRED && !packet);
    cleared(&a);
    assert(mx_consumer_open(&a, 4, 7, 11, 1000));
    first = metadata(4, 0); first.native_frame = 999;
    assert(mx_consumer_push(&a, 110, &first, expected, MX_CONSUMER_SAMPLES) == MX_COPY_INVALID);
    cleared(&a);
    assert(mx_consumer_open(&a, 5, 7, 11, 1000));
    first = metadata(5, 0); expected[0] = NAN;
    assert(mx_consumer_push(&a, 110, &first, expected, MX_CONSUMER_SAMPLES) == MX_COPY_INVALID);
    cleared(&a); mx_consumer_close(&sibling); cleared(&sibling);
    puts("owned consumer copy: byte precision, binding/epoch replay, expiry, terminal overflow, sibling isolation and full scrub passed");
    return 0;
}
