// SPDX-License-Identifier: MIT
#ifndef MIXYARD_NATIVE_FORMAT_H
#define MIXYARD_NATIVE_FORMAT_H
#include <stdbool.h>
#include <spa/param/audio/format-utils.h>
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
#endif
