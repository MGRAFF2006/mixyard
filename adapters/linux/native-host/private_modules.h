// SPDX-License-Identifier: MIT
#ifndef MIXYARD_PRIVATE_MODULES_H
#define MIXYARD_PRIVATE_MODULES_H
#include <stdbool.h>
#include <stddef.h>
#include <string.h>
// Exact modules in the owned server, not a general native-server allowlist.
static const char *const mx_private_modules[] = {
    "libpipewire-module-protocol-native",
    "libpipewire-module-client-node",
    "libpipewire-module-access",
    "libpipewire-module-spa-node-factory",
    "libpipewire-module-link-factory",
    "libpipewire-module-metadata"
};
static inline bool mx_private_module_allowed(const char *name) {
    if (!name) return false;
    for (size_t i = 0; i < sizeof(mx_private_modules) / sizeof(mx_private_modules[0]); ++i)
        if (!strcmp(name, mx_private_modules[i])) return true;
    return false;
}
#endif
