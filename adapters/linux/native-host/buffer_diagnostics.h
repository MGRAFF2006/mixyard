// SPDX-License-Identifier: MIT
#ifndef MIXYARD_BUFFER_DIAGNOSTICS_H
#define MIXYARD_BUFFER_DIAGNOSTICS_H
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <stdatomic.h>
#include <inttypes.h>
#include <stdio.h>
#define MX_NATIVE_POOL_SLOTS 8
#define MX_NATIVE_REGISTRATIONS 128
_Static_assert(ATOMIC_POINTER_LOCK_FREE == 2 && ATOMIC_LLONG_LOCK_FREE == 2 && ATOMIC_LONG_LOCK_FREE == 2,
    "native diagnostic identities must not introduce callback locks");
struct mx_relative_region { bool known, overlap; int64_t offset; };
struct mx_buffer_layout {
    uint32_t capacity;
    struct mx_relative_region chunk, header;
};
static inline struct mx_relative_region mx_relative_region(const void *data, size_t capacity,
        const void *region, size_t length) {
    uintptr_t d = (uintptr_t)data, r = (uintptr_t)region;
    struct mx_relative_region result = {0};
    if (!data || !region || !capacity || !length || capacity > UINTPTR_MAX-d || length > UINTPTR_MAX-r) return result;
    uintptr_t delta = r >= d ? r-d : d-r;
    if (delta > INT64_MAX) return result;
    result.known = true; result.offset = r >= d ? (int64_t)delta : -(int64_t)delta;
    result.overlap = d < r+length && r < d+capacity;
    return result;
}
static inline void mx_print_relative_region(const struct mx_relative_region *r) {
    printf("{\"known\":%s,\"offsetFromPcm\":", r->known ? "true" : "false");
    if (r->known) printf("%" PRId64, r->offset); else printf("null");
    printf(",\"overlapsPcm\":%s}", r->overlap ? "true" : "false");
}
static inline void mx_print_buffer_layout(const struct mx_buffer_layout *r) {
    printf("{\"capacity\":%u,\"chunk\":", r->capacity); mx_print_relative_region(&r->chunk);
    printf(",\"header\":"); mx_print_relative_region(&r->header); printf("}");
}
// Opaque token identities only, never dereferenced or serialized. Add/remove
// events publish IDs; only a live callback may inspect its actual native buffer.
struct mx_buffer_id_slot { _Atomic uintptr_t token; _Atomic uint64_t id; };
struct mx_buffer_ids { struct mx_buffer_id_slot slots[MX_NATIVE_POOL_SLOTS]; };
static inline bool mx_buffer_register(struct mx_buffer_ids *ids, const void *token, uint64_t id) {
    if (!token || !id) return false;
    for (unsigned i = 0; i < MX_NATIVE_POOL_SLOTS; ++i)
        if (atomic_load(&ids->slots[i].token) == (uintptr_t)token) return false;
    for (unsigned i = 0; i < MX_NATIVE_POOL_SLOTS; ++i) {
        uintptr_t empty = 0;
        if (atomic_compare_exchange_strong(&ids->slots[i].token, &empty, (uintptr_t)token)) {
            atomic_store(&ids->slots[i].id, id); return true;
        }
    }
    return false;
}
static inline uint64_t mx_buffer_identity(const struct mx_buffer_ids *ids, const void *token) {
    for (unsigned i = 0; i < MX_NATIVE_POOL_SLOTS; ++i) {
        uint64_t first = atomic_load(&ids->slots[i].id);
        if (first && atomic_load(&ids->slots[i].token) == (uintptr_t)token &&
            atomic_load(&ids->slots[i].id) == first) return first;
    }
    return 0; // Missing/in-flight registration is unknown, never invented identity.
}
static inline uint64_t mx_buffer_unregister(struct mx_buffer_ids *ids, const void *token) {
    for (unsigned i = 0; i < MX_NATIVE_POOL_SLOTS; ++i) {
        if (atomic_load(&ids->slots[i].token) == (uintptr_t)token) {
            uint64_t id = atomic_exchange(&ids->slots[i].id, 0);
            atomic_store(&ids->slots[i].token, 0); return id;
        }
    }
    return 0;
}
#endif
