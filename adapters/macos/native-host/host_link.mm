// SPDX-License-Identifier: MIT
#include "host_link.hpp"
#import <Foundation/Foundation.h>
#include <algorithm>

namespace mixyard::macos {
namespace w = worker;
namespace {
constexpr size_t portLimit = 8;
NSString* string(const std::string& s) {
    auto* value = [[NSString alloc] initWithBytes:s.data() length:s.size() encoding:NSUTF8StringEncoding];
    if (!value) throw std::invalid_argument("Port identity must be UTF-8");
    return [value autorelease];
}
NSArray* labels(const std::vector<std::string>& values) {
    auto* result = [NSMutableArray array];
    for (const auto& value : values) [result addObject:string(value)];
    return result;
}
NSDictionary* json(const uint8_t* data, size_t bytes) {
    auto* value = [NSJSONSerialization JSONObjectWithData:[NSData dataWithBytes:data length:bytes] options:0 error:nullptr];
    if (![value isKindOfClass:[NSDictionary class]]) throw std::invalid_argument("Host port info is not an object");
    return value;
}
using Getter = int (*)(const mx_host*, size_t, uint8_t*, size_t, size_t*);
void validatePort(const mx_host* host, size_t index, Getter getter, const Port& port, bool source) {
    size_t needed = 0;
    // The getter may report insufficient capacity while returning needed.
    getter(host, index, nullptr, 0, &needed);
    if (!needed || needed > 65536) throw std::invalid_argument("Unbounded host port info");
    std::vector<uint8_t> buffer(needed);
    if (getter(host, index, buffer.data(), buffer.size(), &needed) != 0 || needed > buffer.size())
        throw std::invalid_argument("Host port info unavailable");
    const auto end = std::find(buffer.begin(), buffer.begin() + needed, uint8_t{0});
    auto* info = json(buffer.data(), static_cast<size_t>(end - buffer.begin()));
    const auto& f = port.queue->format();
    if (![info[@"observedId"] isEqual:string(port.observedId)] ||
        ![info[@"sampleRateHz"] isEqual:@(f.sampleRateHz)] ||
        ![info[@"channels"] isEqual:labels(f.channels)] ||
        ![info[@"mapping"] isEqual:labels(port.mapping)])
        throw std::invalid_argument("Observed native port differs from compiled canonical slot");
    if (source && ![info[@"encoding"] isEqual:(f.precision == w::Precision::Float64 ? @"f64-le" : @"f32-le")])
        throw std::invalid_argument("Source precision differs from host binding");
}
mx_stamp stamp(const w::Stamp& s) noexcept {
    return {s.binding.generation, s.binding.revision, s.clockId, s.sequence, s.firstFrame, s.capturedNs, s.deadlineNs};
}
}
struct HostLink::Impl {
    mx_host* host = nullptr;
    const Timeline timeline;
    const std::vector<Port> sources, outputs;
    std::array<mx_input, portLimit> inputs{};
    std::array<bool, portLimit> borrowed{};
    alignas(8) std::array<uint8_t, w::maxBytes> converted{};
    std::atomic<uint32_t> cancelled{0};
    bool hostRevoked = false;
    explicit Impl(Timeline t, std::vector<Port> in, std::vector<Port> out)
        : timeline(t), sources(std::move(in)), outputs(std::move(out)) {}
    ~Impl() { if (host) { mx_host_revoke(host); mx_host_destroy(host); } }
    void cancel() noexcept {
        cancelled.store(1, std::memory_order_release);
        for (const auto& port : sources) port.queue->revoke();
        for (const auto& port : outputs) port.queue->revoke();
    }
    bool step(uint64_t now, mx_stamp engine, uint32_t frames) noexcept {
        if (cancelled.load(std::memory_order_acquire)) {
            if (!hostRevoked) { mx_host_revoke(host); hostRevoked = true; }
            return false;
        }
        if (!frames || frames > w::maxFrames || engine.clock_id != timeline.id()) { cancel(); return false; }
        for (size_t i = 0; i < sources.size(); ++i) {
            auto& q = *sources[i].queue;
            const auto& f = q.format();
            const auto* b = q.front(now);
            borrowed[i] = b != nullptr;
            inputs[i] = {nullptr, 0, f.precision == w::Precision::Float64 ? 2u : 1u,
                         f.sampleRateHz, static_cast<uint32_t>(f.channels.size()), engine};
            if (!b) continue;
            if (b->frames != frames || b->stamp.firstFrame != engine.first_frame ||
                b->stamp.clockId != engine.clock_id) { q.fail(w::Fault::Clock); continue; }
            if (q.fault() == w::Fault::None) {
                inputs[i].bytes = b->pcm.data();
                inputs[i].byte_count = frames * f.frameBytes();
                inputs[i].stamp = stamp(b->stamp);
            }
        }
        const auto result = mx_host_process(host, now, engine, frames, inputs.data(), sources.size());
        for (size_t i = 0; i < sources.size(); ++i) if (borrowed[i]) sources[i].queue->release();
        if (result != 0) { cancel(); return false; }
        // A source revoked while Rust borrowed its block cannot authorize the
        // just-computed outputs. Retain no PCM from that in-flight generation.
        for (size_t i = 0; i < sources.size(); ++i)
            if (inputs[i].bytes && sources[i].queue->fault() != w::Fault::None) { cancel(); return false; }
        if (cancelled.load(std::memory_order_acquire)) return false;
        for (size_t i = 0; i < outputs.size(); ++i) {
            auto& q = *outputs[i].queue;
            if (q.fault() != w::Fault::None) continue;
            const double* pcm = nullptr; size_t count = 0;
            const auto expected = size_t(frames) * q.format().channels.size();
            if (mx_host_output(host, i, &pcm, &count) != 0 || !pcm || count != expected ||
                expected > w::maxBytes / sizeof(double)) { q.fail(w::Fault::Shape); continue; }
            const void* bytes = pcm;
            if (q.format().precision == w::Precision::Float32) {
                for (size_t j = 0; j < count; ++j) {
                    // Conversion belongs only to this explicitly Float32 sink.
                    // Finite overflow is a terminal sink error, never clipping.
                    if (!std::isfinite(pcm[j]) || std::abs(pcm[j]) > std::numeric_limits<float>::max()) {
                        q.fail(w::Fault::Nonfinite); break;
                    }
                    const auto value = static_cast<float>(pcm[j]);
                    std::memcpy(converted.data() + j * sizeof(float), &value, sizeof(value));
                }
                bytes = converted.data();
            }
            const void* buffers[]{bytes}; const uint32_t sizes[]{frames * q.format().frameBytes()};
            const w::Stamp output{{engine.generation, engine.revision}, engine.clock_id, engine.sequence,
                                  engine.first_frame, engine.captured_ns, engine.deadline_ns};
            q.push(now, output, frames, buffers, sizes, 1, false);
        }
        return !cancelled.load(std::memory_order_acquire);
    }
};
HostLink::HostLink(std::unique_ptr<Impl> impl) : impl_(std::move(impl)) {}
HostLink::~HostLink() { impl_->cancel(); }
std::unique_ptr<HostLink> HostLink::create(const std::string& request, Timeline timeline,
                                         std::vector<Port> sources, std::vector<Port> outputs) {
    if (sources.empty() || outputs.empty() || sources.size() > portLimit || outputs.size() > portLimit)
        throw std::invalid_argument("Native host requires 1..8 sources and independent outputs");
    const auto* first = sources.front().queue;
    if (!first) throw std::invalid_argument("Missing source queue");
    const auto binding = first->binding(); const auto rate = first->format().sampleRateHz;
    std::vector<w::Queue*> queues;
    for (const auto* ports : {&sources, &outputs}) for (const auto& port : *ports) {
        const auto* q = port.queue;
        if (!q || q->fault() != w::Fault::None || !(q->binding() == binding) || q->clockId() != timeline.id() ||
            q->format().sampleRateHz != rate || q->format().channels.size() > 2 ||
            port.mapping.size() != q->format().channels.size() || port.observedId.empty() ||
            q->provenance().source != port.observedId || std::find(queues.begin(), queues.end(), q) != queues.end() ||
            (ports == &outputs && q->provenance().origin != w::Origin::AdaptedEngine))
            throw std::invalid_argument("Exact scoped mono/stereo native port binding required");
        queues.push_back(port.queue);
    }
    auto impl = std::make_unique<Impl>(timeline, std::move(sources), std::move(outputs));
    std::array<char, 1024> error{};
    if (mx_host_create(reinterpret_cast<const uint8_t*>(request.data()), request.size(), &impl->host,
                       error.data(), error.size()) != 0 || !impl->host)
        throw std::invalid_argument(std::string("Canonical host rejected request: ") + error.data());
    if (mx_host_source_count(impl->host) != impl->sources.size() || mx_host_output_count(impl->host) != impl->outputs.size())
        throw std::invalid_argument("Host plan port count differs");
    @autoreleasepool {
        for (size_t i = 0; i < impl->sources.size(); ++i)
            validatePort(impl->host, i, mx_host_source_info, impl->sources[i], true);
        for (size_t i = 0; i < impl->outputs.size(); ++i)
            validatePort(impl->host, i, mx_host_output_info, impl->outputs[i], false);
    }
    return std::unique_ptr<HostLink>(new HostLink(std::move(impl)));
}
bool HostLink::step(uint64_t now, mx_stamp s, uint32_t frames) noexcept { return impl_->step(now, s, frames); }
void HostLink::cancel() noexcept { impl_->cancel(); }
} // namespace mixyard::macos
