// SPDX-License-Identifier: MIT
// Exact contracts inspected for the two hosted diagnostic SDKs; not a product
// compatibility policy. No generic ENOTSUP/unknown-version fallback is allowed.
#ifndef MIXYARD_NATIVE_POOL_CONTRACT_H
#define MIXYARD_NATIVE_POOL_CONTRACT_H
#include <stdbool.h>
#include <errno.h>
#include <string.h>
static inline bool mx_pool_position_contract(const char *headers, const char *runtime, int result) {
    if (!headers || !runtime || strcmp(headers, runtime)) return false;
    if (!strcmp(headers, "1.0.5")) return result == -ENOTSUP;
    if (!strcmp(headers, "1.4.8")) return result == 0;
    return false;
}
#endif
