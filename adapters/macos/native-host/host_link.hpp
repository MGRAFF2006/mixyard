// SPDX-License-Identifier: MIT
#pragma once
#include "../worker_bridge.hpp"
#include <mixyard_host.h>
#include <memory>

namespace mixyard::macos {
// No physical clock mapping is currently qualified. The injected timeline is
// available only in SDK tests and never selectable in the production library.
class Timeline {
    uint64_t id_;
    explicit Timeline(uint64_t id) : id_(id) {}
public:
    static Timeline physical(uint64_t) {
        throw std::invalid_argument("Physical tap/AUHAL clocks are unqualified; equal nominal rates do not establish a common timeline");
    }
#ifdef MIXYARD_NATIVE_HOST_TEST
    static Timeline injected(uint64_t id) {
        if (!id) throw std::invalid_argument("Injected clock ID required");
        return Timeline(id);
    }
#endif
    uint64_t id() const noexcept { return id_; }
};

struct Port {
    worker::Queue* queue;
    std::string observedId;
    std::vector<std::string> mapping;
};

enum class StepResult { Processed, SourceLoss, Cancelled, HostFailure };
constexpr const char* stepResultName(StepResult result) noexcept {
    switch (result) {
    case StepResult::Processed: return "processed";
    case StepResult::SourceLoss: return "source_loss";
    case StepResult::Cancelled: return "cancelled";
    case StepResult::HostFailure: return "host_failure";
    }
    return "host_failure";
}

// Owns the real Rust host; never supplies an alternate mixer. All construction,
// immutable port metadata checks and destruction occur on a quiescent control owner. step()
// has one exclusive engine owner; source and destination callbacks use SPSC.
class HostLink {
    struct Impl;
    std::unique_ptr<Impl> impl_;
    explicit HostLink(std::unique_ptr<Impl>);
public:
    static std::unique_ptr<HostLink> create(const std::string& requestJson, Timeline,
        std::vector<Port> sources, std::vector<Port> outputs);
    ~HostLink();
    HostLink(const HostLink&) = delete;
    HostLink& operator=(const HostLink&) = delete;
    // No allocation, locks, JSON or I/O. Any absent/failed selected native source
    // returns source_loss and terminally revokes the entire generation. This is
    // a notification, NOT native-consumer quiescence/teardown acknowledgement.
    // Expected scheduled silence must arrive as fresh valid PCM, not absence.
    // Destination-only DSP/queue failures remain isolated to that consumer.
    StepResult step(uint64_t nowNs, mx_stamp engineStamp, uint32_t frames) noexcept;
    // Concurrent cancellation only revokes queues/sets an atomic flag. Rust
    // host revocation/destruction stays with its exclusive quiescent owner.
    void cancel() noexcept;
};
} // namespace mixyard::macos
