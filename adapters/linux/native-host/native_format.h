// SPDX-License-Identifier: MIT
#ifndef MIXYARD_NATIVE_FORMAT_H
#define MIXYARD_NATIVE_FORMAT_H
#include <stdbool.h>
#include <spa/param/audio/format-utils.h>
#include <spa/buffer/buffer.h>
static inline struct spa_audio_info_raw mx_native_format_requested(void) {
    return (struct spa_audio_info_raw){ .format = SPA_AUDIO_FORMAT_F64_LE, .rate = 48000,
        .channels = 2, .position = {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR} };
}
static inline bool mx_native_format_matches(int parse_result, const struct spa_audio_info_raw *info) {
    // SPA returns a nonnegative collected-property count, not zero-only success.
    return parse_result >= 0 && info->format == SPA_AUDIO_FORMAT_F64_LE &&
        info->rate == 48000 && info->channels == 2 &&
        info->position[0] == SPA_AUDIO_CHANNEL_FL && info->position[1] == SPA_AUDIO_CHANNEL_FR;
}
// Before first admission/warming only: initialized zero valid bytes are not PCM.
// SPA allocation zeroes chunk fields; EMPTY is an optional neutral-data marker,
// not the only way for valid-data length to be zero. No clock/header is proved.
static inline bool mx_native_startup_unavailable(bool warming, const struct spa_buffer *buffer, uint32_t frames) {
    if (!warming || !buffer || buffer->n_datas != 1 || !buffer->datas || !frames || frames > 128) return false;
    const struct spa_data *d = &buffer->datas[0];
    if (!d->data || !d->chunk || d->maxsize < frames * 16 || d->chunk->offset != 0 ||
        spa_buffer_find_meta_data(buffer, SPA_META_Header, sizeof(struct spa_meta_header)) == NULL) return false;
    const struct spa_chunk *chunk = d->chunk;
    if (chunk->size == 0)
        return (chunk->stride == 0 || chunk->stride == 16) &&
            (chunk->flags == 0 || chunk->flags == SPA_CHUNK_FLAG_EMPTY);
    // Full explicitly neutral quantum may also be unavailable. Positive partial,
    // corrupted/unknown flags and all admitted cases still reach strict faults.
    return chunk->size == frames * 16 && chunk->stride == 16 && chunk->flags == SPA_CHUNK_FLAG_EMPTY;
}
static inline bool mx_native_header_current(const struct spa_meta_header *header,
        uint64_t native_frame, uint64_t native_ns) {
    return header && native_ns <= INT64_MAX && !header->flags && !header->offset && !header->dts_offset &&
        header->seq == native_frame && header->pts == (int64_t)native_ns;
}
#endif
