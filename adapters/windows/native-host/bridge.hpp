// SPDX-License-Identifier: MIT
#pragma once
#include "../worker.h"
#include "mixyard_host.h"
#include <array>
#include <atomic>
#include <memory>
#include <string>
#include <vector>

namespace mixyard_windows_host {
// Physical device clocks are deliberately NOT admitted. A shared nominal rate,
// WASAPI QPC100ns or a caller-supplied clock label does not qualify their mapping.
enum class ClockKind { physical_unqualified, injected_common_timeline };
struct Timeline {
    ClockKind kind = ClockKind::physical_unqualified;
    uint64_t generation = 0, revision = 0, clock_id = 0;
    std::string evidence;
};
struct Endpoint {
    std::string observed_id, endpoint_id, provenance, consent, exclusions;
    myw_format format{}; // Actual negotiated format, not inventory's nominal hint.
    bool injected = false;
    bool owned_output = false;
    bool allow_float32_destination = false;
};
// Control owner resolves authority and actual native observations before create.
// For an injected SDK endpoint this is its explicit private synthetic identity.
// No default endpoint selection, application identity inference or driver mapping.
struct Authority {
    void* context = nullptr;
    bool (*admit)(void*, const Endpoint&, int direction, const Timeline&) = nullptr;
};
struct MappedTime {
    mx_stamp engine{};
    uint64_t device_frame = 0, qpc_100ns = 0;
    uint32_t packet_offset = 0;
};
enum class Result { ok, cancelled, invalid, binding, clock, host, source_time, exhausted, source_loss };

class Bridge {
    struct Port {
        Endpoint endpoint;
        myw_port* queue = nullptr;
        std::array<uint8_t, MYW_MAX_FRAMES * 2 * sizeof(double)> bytes{};
        uint64_t sequence = 0, first_frame = 0;
        Port() = default;
        ~Port() { myw_port_destroy(queue); }
        Port(const Port&) = delete;
        Port& operator=(const Port&) = delete;
    };
    mx_host* host_; // Borrowed exclusively; never accessed by endpoint threads.
    Timeline timeline_;
    std::vector<std::unique_ptr<Port>> sources_, outputs_;
    std::vector<mx_input> inputs_;
    std::atomic<bool> cancelled_{false};
    bool host_revoked_ = false; // Graph-owner only.
    uint64_t sequence_ = 0, first_frame_ = 0;
    Bridge(mx_host*, Timeline);
    Result stop(Result);
public:
    // Control-thread allocation/info validation. host ownership stays with caller;
    // keep it alive and exclusive until this object is destroyed. On failure no
    // native stream is started. Source/output arrays MUST follow host plan order.
    static Result create(mx_host*, Timeline, const std::vector<Endpoint>& sources,
                         const std::vector<Endpoint>& outputs, Authority,
                         std::unique_ptr<Bridge>& out);
    ~Bridge(); // Quiescent only, after ALL endpoint and graph threads joined.
    Bridge(const Bridge&) = delete;
    Bridge& operator=(const Bridge&) = delete;
    myw_port* source(size_t index) const;
    myw_port* output(size_t index) const;
    // Safe control-thread cancellation. Does not touch the single-owner Rust host.
    void cancel();
    // Control-only after ALL graph/capture/consumer workers are stopped/joined.
    // Owned Lifecycle invokes this only after its actual worker joins/scrubs.
    // Returns invalid while admission is open. This is never a callback clear.
    Result scrub_quiescent();
    // Sole graph-owner, bounded/preallocated: no locks, allocation, I/O or native
    // calls. MappedTime binds EACH observed native packet to an explicit injected
    // engine timestamp; QPC is compared as provenance, never cast into engine ns.
    // ANY missing/failed selected source returns source_loss and terminally
    // revokes the WHOLE generation, including healthy sibling output queues.
    // Existing render cursors/pop return zero-filled output on that revoked
    // port, so retained PCM cannot be submitted after loss is acknowledged.
    // source_loss reports CLOSED ADMISSION, not completed quiescence/scrubbing.
    // Lifecycle::try_finish_stop provides completion only after actual joins.
    // Expected scheduled silence is a real zero-filled packet, NOT missing
    // capture input. Do not reuse this loss policy for network jitter/loss.
    // Restart requires quiescence and a fresh host/generation. A destination-only
    // failure stays isolated; it does not close healthy sources/outputs.
    Result process(uint64_t now_ns, mx_stamp engine, uint32_t frames,
                   const MappedTime* source_times, size_t count);
};

// Deterministic control-plane specs used in host create JSON and checked against
// immutable host info. No request parsing or alternate routing model lives here.
std::string source_spec(const Endpoint&);
std::string output_spec(const Endpoint&);
} // namespace mixyard_windows_host
