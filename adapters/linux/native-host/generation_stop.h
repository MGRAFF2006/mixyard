// SPDX-License-Identifier: MIT
#ifndef MIXYARD_NATIVE_GENERATION_STOP_H
#define MIXYARD_NATIVE_GENERATION_STOP_H
#include "consumer_copy.h"
// Exclusive graph DataLoop owner only, after closing new admission. Never a
// concurrent queue clear or proof that already-submitted native buffers vanished.
static inline void mx_native_generation_stop(mx_host *host,
        struct mx_consumer_copy *consumers, size_t count) {
    if (host) mx_host_revoke(host);
    for (size_t i = 0; i < count; ++i) mx_consumer_close(&consumers[i]);
}
#endif
