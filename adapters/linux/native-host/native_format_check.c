// SPDX-License-Identifier: MIT
// SDK-only POD build/parse regression. No context, device, server or native I/O.
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <spa/buffer/alloc.h>
#include "native_format.h"
static void check(struct spa_audio_info_raw info, uint32_t parameter_id, bool expected) {
    uint8_t storage[2048];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    const struct spa_pod *pod = spa_format_audio_raw_build(&builder, parameter_id, &info);
    assert(pod);
    struct spa_audio_info_raw actual = {0};
    int result = spa_format_audio_raw_parse(pod, &actual);
    assert(mx_native_format_matches(result, &actual) == expected);
    if (expected) {
        // This actual complete POD reproduces the old zero-only rejection.
        assert(result > 0);
        printf("exact request POD id=%u rawParse=%d format=%u rate=%u channels=%u positions=%u,%u\n",
            parameter_id, result, (unsigned)actual.format, actual.rate, actual.channels,
            actual.position[0], actual.position[1]);
    }
    uint32_t media_type = 0, media_subtype = 0;
    assert(spa_format_parse(pod, &media_type, &media_subtype) >= 0);
    assert(media_type == SPA_MEDIA_TYPE_audio && media_subtype == SPA_MEDIA_SUBTYPE_raw);
}
int main(void) {
    struct spa_audio_info_raw exact = mx_native_format_requested();
    check(exact, SPA_PARAM_EnumFormat, true);
    check(exact, SPA_PARAM_Format, true);
    struct spa_audio_info_raw changed = exact;
    changed.format = SPA_AUDIO_FORMAT_F32_LE; check(changed, SPA_PARAM_Format, false);
    changed = exact; changed.format = SPA_AUDIO_FORMAT_UNKNOWN; check(changed, SPA_PARAM_Format, false);
    changed = exact; changed.rate = 44100; check(changed, SPA_PARAM_Format, false);
    changed = exact; changed.rate = 0; check(changed, SPA_PARAM_Format, false);
    changed = exact; changed.channels = 0; check(changed, SPA_PARAM_Format, false);
    changed = exact; changed.channels = 1; check(changed, SPA_PARAM_Format, false);
    changed = exact; changed.position[0] = SPA_AUDIO_CHANNEL_FR;
    changed.position[1] = SPA_AUDIO_CHANNEL_FL; check(changed, SPA_PARAM_Format, false);
    changed = exact; changed.flags = SPA_AUDIO_FLAG_UNPOSITIONED;
    check(changed, SPA_PARAM_Format, false);
    // An object truncated before its body must not count as an empty valid raw format.
    const struct spa_pod truncated = { .size = 0, .type = SPA_TYPE_Object };
    struct spa_audio_info_raw invalid = {0};
    int negative = spa_format_audio_raw_parse(&truncated, &invalid);
    assert(negative < 0 && !mx_native_format_matches(negative, &invalid));
    invalid = exact;
    assert(!mx_native_format_matches(-EINVAL, &invalid));
    // Use the actual SDK initializer, not a guessed hand-filled chunk shape.
    // PipeWire buffers.c uses calloc + this same layout path; all-zero chunk and
    // header fields describe no valid PCM despite allocated data capacity.
    struct spa_meta initialized_meta = { .type = SPA_META_Header, .size = sizeof(struct spa_meta_header) };
    struct spa_data initialized_data = { .type = SPA_DATA_MemPtr, .flags = SPA_DATA_FLAG_READWRITE,
        .maxsize = 2048 };
    uint32_t alignment = 64;
    struct spa_buffer **initialized = spa_buffer_alloc_array(1, 0, 1, &initialized_meta, 1, &initialized_data, &alignment);
    assert(initialized && initialized[0]->datas[0].data);
    struct spa_chunk *initial_chunk = initialized[0]->datas[0].chunk;
    assert(initial_chunk && initial_chunk->offset == 0 && initial_chunk->size == 0 &&
        initial_chunk->stride == 0 && initial_chunk->flags == 0);
    assert(mx_native_startup_unavailable(true, initialized[0], 128)); // RED with EMPTY-only helper
    assert(!mx_native_startup_unavailable(false, initialized[0], 128));
    struct spa_meta_header *initial_header = spa_buffer_find_meta_data(initialized[0], SPA_META_Header, sizeof(*initial_header));
    assert(initial_header && !mx_native_header_current(initial_header, 123, 456));
    initial_chunk->flags = SPA_CHUNK_FLAG_CORRUPTED;
    assert(!mx_native_startup_unavailable(true, initialized[0], 128));
    initial_chunk->flags = 4; assert(!mx_native_startup_unavailable(true, initialized[0], 128));
    initial_chunk->flags = 0;
    initial_chunk->stride = 8; assert(!mx_native_startup_unavailable(true, initialized[0], 128));
    initial_chunk->stride = 16;
    for (uint32_t size = 1; size < 2048; ++size) {
        initial_chunk->size = size; assert(!mx_native_startup_unavailable(true, initialized[0], 128));
    }
    initial_chunk->size = 0; initial_chunk->offset = 16;
    assert(!mx_native_startup_unavailable(true, initialized[0], 128));
    initial_chunk->offset = 0; initialized[0]->n_metas = 0;
    assert(!mx_native_startup_unavailable(true, initialized[0], 128));
    free(initialized);
    double untouched[256]; memset(untouched, 0x7f, sizeof(untouched));
    struct spa_meta_header h = {0};
    struct spa_meta meta = { .type = SPA_META_Header, .size = sizeof(h), .data = &h };
    struct spa_chunk chunk = { .size = sizeof(untouched), .stride = 16, .flags = SPA_CHUNK_FLAG_EMPTY };
    struct spa_data data = { .type = SPA_DATA_MemPtr, .maxsize = sizeof(untouched),
        .data = untouched, .chunk = &chunk };
    struct spa_buffer buffer = { .n_datas = 1, .datas = &data, .n_metas = 1, .metas = &meta };
    assert(mx_native_startup_unavailable(true, &buffer, 128));
    assert(!mx_native_startup_unavailable(false, &buffer, 128)); // admitted/revoked: no startup exception
    assert(!mx_native_header_current(&h, 123, 456)); // unavailable is not fresh clock evidence
    // Actual startup observation: valid-data length is zero with exact EMPTY,
    // not a partial PCM quantum. It stays unavailable and proves no header time.
    chunk.size = 0;
    assert(mx_native_startup_unavailable(true, &buffer, 128)); // old full-size-only predicate rejects
    assert(!mx_native_startup_unavailable(false, &buffer, 128));
    assert(!mx_native_header_current(&h, 123, 456));
    chunk.flags = 0; assert(mx_native_startup_unavailable(true, &buffer, 128));
    chunk.flags = SPA_CHUNK_FLAG_EMPTY;
    for (uint32_t size = 1; size < sizeof(untouched); ++size) {
        chunk.size = size; assert(!mx_native_startup_unavailable(true, &buffer, 128));
    }
    chunk.size = sizeof(untouched)+1; assert(!mx_native_startup_unavailable(true, &buffer, 128));
    chunk.size = sizeof(untouched);
    chunk.offset = 16; assert(!mx_native_startup_unavailable(true, &buffer, 128)); chunk.offset = 0;
    chunk.stride = 8; assert(!mx_native_startup_unavailable(true, &buffer, 128)); chunk.stride = 16;
    chunk.flags |= SPA_CHUNK_FLAG_CORRUPTED; assert(!mx_native_startup_unavailable(true, &buffer, 128));
    chunk.flags = SPA_CHUNK_FLAG_EMPTY; buffer.n_metas = 0;
    assert(!mx_native_startup_unavailable(true, &buffer, 128)); buffer.n_metas = 1;
    h.seq = 123; h.pts = 456; assert(mx_native_header_current(&h, 123, 456));
    h.flags = SPA_META_HEADER_FLAG_GAP; assert(!mx_native_header_current(&h, 123, 456));
    puts("actual SPA initializer/request/parser and strictly unadmitted unavailable/header contract passed (no native I/O)");
    return 0;
}
