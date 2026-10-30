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
// Only a new, unadmitted candidate may classify a fully described native EMPTY
// buffer as unavailable. SPA chunk.size is valid-data bytes; EMPTY may carry
// zero bytes or a full neutral quantum. Positive partial sizes remain invalid.
// This never reads PCM or proves a source clock/header.
static inline bool mx_native_startup_empty(bool warming, const struct spa_buffer *buffer, uint32_t frames) {
    if (!warming || !buffer || buffer->n_datas != 1 || !buffer->datas || !frames || frames > 128) return false;
    const struct spa_data *d = &buffer->datas[0];
    return d->data && d->chunk && d->maxsize >= frames * 16 && d->chunk->offset == 0 &&
        (d->chunk->size == 0 || d->chunk->size == frames * 16) && d->chunk->stride == 16 &&
        d->chunk->flags == SPA_CHUNK_FLAG_EMPTY &&
        spa_buffer_find_meta_data(buffer, SPA_META_Header, sizeof(struct spa_meta_header)) != NULL;
}
static inline bool mx_native_header_current(const struct spa_meta_header *header,
        uint64_t native_frame, uint64_t native_ns) {
    return header && native_ns <= INT64_MAX && !header->flags && !header->offset && !header->dts_offset &&
        header->seq == native_frame && header->pts == (int64_t)native_ns;
}
#endif
