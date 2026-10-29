// SPDX-License-Identifier: MIT
#pragma once
#include "capture.hpp"
#include "owned_queue.hpp"
#include "../output-only/output_only.hpp"

namespace mixyard::macos {
class NativeOutput {
    const std::string uid_;
    OwnedQueue queue_;
    std::unique_ptr<output::Worker> worker_;
    bool closed_ = false;
    NativeOutput(std::string uid, worker::Format, worker::Binding, Timeline);
public:
    // Prepare immutable storage off callback, then open() validates the actual
    // native UID/format/input-disable before start(). Preparation grants no I/O.
    static std::unique_ptr<NativeOutput> prepare(std::string uid, worker::Format,
                                                worker::Binding, Timeline);
    ~NativeOutput();
    worker::Queue& queue() noexcept { return *queue_; }
    void open();
    void start();
    // Stop/dispose native consumer and retire its private Renderer context.
    // false retains ownership for retry; the queue stays alive for HostLink.
    bool close() noexcept;
};
enum class FinishResult { Pending, Complete };
// Exclusive quiescent graph owner ONLY, after its final step and after stopping
// any engine scheduler. No callback may call this function or retain HostLink.
// Supply every native Capture/NativeOutput in this generation. All producers
// and readers of otherSourceQueues must already be quiescent; this function
// does not stop arbitrary external producer threads on the caller's behalf.
// Complete follows actual native close success, cursor-context retirement and
// quiescent queue scrubbing. Queue revoke/SourceLoss alone cannot acknowledge it.
// A failed close retains every port/queue for retry. Already submitted hardware
// buffers have a separate boundary and are not claimed scrubbed by this API.
FinishResult finishGeneration(std::unique_ptr<HostLink>&,
    std::vector<std::unique_ptr<Capture>>&,
    std::vector<std::unique_ptr<NativeOutput>>&,
    std::vector<OwnedQueue>& otherSourceQueues) noexcept;
} // namespace mixyard::macos
