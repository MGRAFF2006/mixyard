// SPDX-License-Identifier: MIT
#pragma once
#include "bridge.hpp"
#include <exception>
#include <system_error>
#include <thread>

namespace mixyard_windows_host {
// All asynchronous users of an owned bridge belong here; no detached/native
// users may outlive their run() or bypass this ownership boundary. Workers own
// their cursor/endpoint storage and release native interfaces on their run thread.
// Native callback servicing itself performs no allocation/locking/wait here.
class Worker {
public:
    virtual ~Worker() = default;
    virtual void run(Bridge&) = 0;
    virtual bool scrub_quiescent() = 0; // Only after ALL workers are joined.
};
enum class StopState { pending, complete };
enum class StopIssue { none, not_requested, workers_running, join_failed, worker_scrub_failed, bridge_scrub_failed };
class Lifecycle {
    struct Slot {
        std::unique_ptr<Worker> worker;
        std::thread thread;
        std::atomic<bool> finished{true};
    };
    std::unique_ptr<Bridge> bridge_;
    std::vector<std::unique_ptr<Slot>> workers_;
    std::atomic<bool> worker_failed_{false};
    bool started_ = false, stop_requested_ = false, complete_ = false;
    StopIssue stop_issue_ = StopIssue::not_requested;
    std::error_code join_error_;
public:
    explicit Lifecycle(std::unique_ptr<Bridge> bridge) : bridge_(std::move(bridge)) {}
    ~Lifecycle() {
        // Destruction is control-only and may join; never destroy this on one
        // of its own workers. Normal shutdown uses the nonblocking pending API
        // and retains this entire owner until it reports complete.
        request_stop();
        for (const auto& slot : workers_) if (slot->thread.joinable()) slot->thread.join();
        // A caller must retain this owner after a pending/failed scrub. Never
        // silently destroy storage while claiming a failed stop completed.
        if (try_finish_stop() != StopState::complete) std::terminate();
    }
    Lifecycle(const Lifecycle&) = delete;
    Lifecycle& operator=(const Lifecycle&) = delete;
    Bridge& bridge() { return *bridge_; }
    bool worker_failed() const { return worker_failed_.load(std::memory_order_acquire); }
    StopIssue stop_issue() const { return stop_issue_; } // Control owner only.
    std::error_code join_error() const { return join_error_; }
    bool add(std::unique_ptr<Worker> worker) { // Setup only.
        if (started_ || stop_requested_ || !worker) return false;
        auto slot = std::make_unique<Slot>(); slot->worker = std::move(worker);
        workers_.push_back(std::move(slot)); return true;
    }
    bool start() { // Control-only thread creation, never a PCM callback.
        if (started_ || stop_requested_ || !bridge_) return false;
        started_ = true;
        try {
            for (const auto& slot : workers_) {
                slot->finished.store(false, std::memory_order_release);
                try {
                    slot->thread = std::thread([this, state = slot.get()] {
                        try { state->worker->run(*bridge_); }
                        catch (...) { worker_failed_.store(true, std::memory_order_release); bridge_->cancel(); }
                        state->finished.store(true, std::memory_order_release);
                    });
                } catch (...) { slot->finished.store(true, std::memory_order_release); throw; }
            }
            return true;
        } catch (...) { worker_failed_.store(true, std::memory_order_release); request_stop(); return false; }
    }
    void request_stop() { // Control owner only. Worker/graph can still cancel Bridge directly.
        stop_requested_ = true;
        if (bridge_) bridge_->cancel();
    }
    StopState try_finish_stop() { // Control-only; no waiting for running workers.
        if (complete_) return StopState::complete;
        if (!stop_requested_) { stop_issue_ = StopIssue::not_requested; return StopState::pending; }
        for (const auto& slot : workers_)
            if (!slot->finished.load(std::memory_order_acquire)) { stop_issue_ = StopIssue::workers_running; return StopState::pending; }
        // finished is not itself a join. Join EVERY actual thread before ANY
        // queue/cursor scrub. Failure retains workers/bridge for the owner.
        try { for (const auto& slot : workers_) if (slot->thread.joinable()) slot->thread.join(); }
        catch (const std::system_error& error) { stop_issue_ = StopIssue::join_failed; join_error_ = error.code(); return StopState::pending; }
        join_error_.clear();
        for (const auto& slot : workers_)
            if (!slot->worker->scrub_quiescent()) { stop_issue_ = StopIssue::worker_scrub_failed; return StopState::pending; }
        if (!bridge_ || bridge_->scrub_quiescent() != Result::ok) { stop_issue_ = StopIssue::bridge_scrub_failed; return StopState::pending; }
        complete_ = true; stop_issue_ = StopIssue::none; return StopState::complete;
    }
};
} // namespace mixyard_windows_host
