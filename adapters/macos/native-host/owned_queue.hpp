// SPDX-License-Identifier: MIT
#pragma once
#include "../worker_bridge.hpp"
#include <cstddef>
#include <memory>
#include <new>

namespace mixyard::macos {
#ifdef MIXYARD_NATIVE_HOST_TEST
namespace detail { void queueScrubbed(const void*, size_t) noexcept; }
#endif
// Exclusive, quiescent control destruction only. Queue callbacks/engine readers
// must have stopped and their owning contexts must already have been retired.
// Never clear live SPSC storage or manipulate another thread's cursor.
struct ScrubQueue {
    void operator()(worker::Queue* queue) const noexcept {
        queue->~Queue();
        volatile auto* bytes = reinterpret_cast<volatile unsigned char*>(queue);
        for (size_t i = 0; i < sizeof(worker::Queue); ++i) bytes[i] = 0;
#ifdef MIXYARD_NATIVE_HOST_TEST
        detail::queueScrubbed(queue, sizeof(worker::Queue));
#endif
        if constexpr (alignof(worker::Queue) > alignof(std::max_align_t))
            ::operator delete(queue, std::align_val_t{alignof(worker::Queue)});
        else ::operator delete(queue);
    }
};
using OwnedQueue = std::unique_ptr<worker::Queue, ScrubQueue>;
inline OwnedQueue ownQueue(worker::Format format, worker::Binding binding, uint64_t clock,
                           worker::Provenance provenance) {
    return OwnedQueue(new worker::Queue(std::move(format), binding, clock, std::move(provenance)));
}
} // namespace mixyard::macos
