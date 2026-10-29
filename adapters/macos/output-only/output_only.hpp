// SPDX-License-Identifier: MIT
#pragma once
#include "../worker_bridge.hpp"
#include <memory>

namespace mixyard::output {
// One queue, producer and output consumer per instance. Its destination clock,
// exact native positions CA:1..N and scheduling must already be explicitly bound
// by the host. This standalone library does not link the Rust engine.
class Worker {
    struct Impl;
    std::unique_ptr<Impl> impl_;
    explicit Worker(std::unique_ptr<Impl>);
public:
    static std::unique_ptr<Worker> open(std::string explicitUid, worker::Queue&);
    Worker(const Worker&) = delete;
    Worker& operator=(const Worker&) = delete;
    ~Worker();
    // Synchronous serialized control operations; never call from a render callback.
    // A failed start terminally revokes this instance. No implicit restart.
    void start();
    // Includes native stop, and is terminal. false retains storage for close retry.
    bool revoke() noexcept;
    // Idempotent; false requires retry while queue/worker storage remains alive.
    bool close() noexcept;
};
} // namespace mixyard::output
