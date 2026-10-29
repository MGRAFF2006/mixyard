// SPDX-License-Identifier: MIT
#include "native_output.hpp"

namespace mixyard::macos {
NativeOutput::NativeOutput(std::string uid, worker::Format format, worker::Binding binding, Timeline timeline)
    : uid_(std::move(uid)), queue_(ownQueue(std::move(format), binding, timeline.id(),
        {worker::Origin::AdaptedEngine, uid_})) {}
std::unique_ptr<NativeOutput> NativeOutput::prepare(std::string uid, worker::Format format,
                                                  worker::Binding binding, Timeline timeline) {
    return std::unique_ptr<NativeOutput>(new NativeOutput(std::move(uid), std::move(format), binding, timeline));
}
NativeOutput::~NativeOutput() { if (!close()) std::terminate(); }
void NativeOutput::open() {
    if (closed_ || worker_ || queue_->fault() != worker::Fault::None)
        throw std::runtime_error("Native consumer is opened, revoked or closed");
    worker_ = output::Worker::open(uid_, *queue_);
}
void NativeOutput::start() {
    if (!worker_ || closed_) throw std::runtime_error("Native consumer is not admitted");
    worker_->start();
}
bool NativeOutput::close() noexcept {
    queue_->revoke();
    if (worker_) {
        if (!worker_->close()) return false;
        worker_.reset(); // Native callbacks stopped; retire partial cursor/scratch.
    }
    closed_ = true;
    return true;
}
FinishResult finishGeneration(std::unique_ptr<HostLink>& host,
    std::vector<std::unique_ptr<Capture>>& captures,
    std::vector<std::unique_ptr<NativeOutput>>& outputs,
    std::vector<OwnedQueue>& otherSources) noexcept {
    if (host) { host->cancel(); host.reset(); }
    bool closed = true;
    for (const auto& source : captures) if (!source->close()) closed = false;
    for (const auto& output : outputs) if (!output->close()) closed = false;
    if (!closed) return FinishResult::Pending;
    // No graph borrows/native callbacks/cursors remain. Only now scrub/free
    // private queue storage. A late listener has no owner after native close.
    outputs.clear(); captures.clear(); otherSources.clear();
    return FinishResult::Complete;
}
} // namespace mixyard::macos
