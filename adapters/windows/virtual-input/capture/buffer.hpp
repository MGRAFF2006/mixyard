// SPDX-License-Identifier: MIT
#pragma once
#include "../transport.hpp"
namespace mixyard_input {
// Shared allocation path; native caller supplies MmGetMdlByteCount and port methods.
template <typename Mdl, typename Port, typename Address, typename Cache, typename ByteCount>
Mdl allocate_capture_buffer(Port* port, Address high, uint32_t requested, Cache cache,
                            ByteCount byte_count, uint32_t** mapped) {
    *mapped = nullptr;
    auto* allocated = port->AllocatePagesForMdl(high, requested);
    if (!allocated) return nullptr;
    // A non-null MDL can describe a partial allocation. Never map/expose it.
    if (byte_count(allocated) < requested) {
        port->FreePagesFromMdl(allocated);
        return nullptr;
    }
    *mapped = static_cast<uint32_t*>(port->MapAllocatedPages(allocated, cache));
    if (!*mapped) {
        port->FreePagesFromMdl(allocated);
        return nullptr;
    }
    return allocated;
}
} // namespace mixyard_input
