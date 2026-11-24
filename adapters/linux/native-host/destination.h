// SPDX-License-Identifier: MIT
#ifndef MIXYARD_PIPEWIRE_DESTINATION_H
#define MIXYARD_PIPEWIRE_DESTINATION_H
#include <stdbool.h>
#include <string.h>
#include "mixyard_host.h"

// A host output can close independently after destination DSP/queue failure.
// Always erase the native buffer first; only exact active PCM clears GAP.
static inline int mx_destination_copy(const mx_host *host, size_t index,
        bool rendered, double *destination, size_t samples, bool *silent) {
    memset(destination, 0, samples * sizeof(double));
    *silent = true;
    if (!rendered) return MX_OK;
    const double *output = NULL;
    size_t count = 0;
    int result = mx_host_output(host, index, &output, &count);
    if (result == MX_INACTIVE) return output == NULL && count == 0 ? MX_INACTIVE : MX_BUFFER;
    if (result != MX_OK) return result;
    if (output == NULL || count != samples) return MX_BUFFER;
    memcpy(destination, output, samples * sizeof(double));
    *silent = false;
    return MX_OK;
}
#endif
