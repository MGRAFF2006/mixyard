// SPDX-License-Identifier: MIT
#include "bridge.hpp"
#include "../inventory/protocol.hpp"
#include <cmath>
#include <cstring>
#include <limits>
#include <utility>

namespace mixyard_windows_host {
static_assert(std::atomic<bool>::is_always_lock_free);
namespace {
bool layout(myw_format f) {
    return (f.channels == 1 && f.channel_mask == 4) || (f.channels == 2 && f.channel_mask == 3);
}
uint32_t encoding(myw_format f) {
    if (!myw_frame_bytes(f) || !layout(f) || f.valid_bits != f.container_bits) return 0;
    if (f.floating) return f.container_bits == 32 ? 1u : f.container_bits == 64 ? 2u : 0u;
    return f.container_bits == 16 ? 3u : f.container_bits == 24 ? 4u : f.container_bits == 32 ? 5u : 0u;
}
const char* encoding_name(myw_format f) {
    switch (encoding(f)) {
    case 1: return "f32-le"; case 2: return "f64-le"; case 3: return "s16-le";
    case 4: return "s24-le"; case 5: return "s32-le";
    default: throw std::invalid_argument("unsupported exact Windows source format");
    }
}
std::string suffix(const Endpoint& e) {
    if (!layout(e.format) || !myw_frame_bytes(e.format)) throw std::invalid_argument("unsupported processed layout");
    const std::string labels = e.format.channels == 1 ? "[\"front-center\"]" : "[\"front-left\",\"front-right\"]";
    const std::string mapping = e.format.channels == 1 ? "[\"MONO\"]" : "[\"FL\",\"FR\"]";
    return ",\"sampleRateHz\":" + std::to_string(e.format.rate) + ",\"channels\":" + labels + ",\"mapping\":" + mapping + "}";
}
bool host_info(mx_host* host, size_t index, bool input, const std::string& expected) {
    size_t needed = 0;
    const auto get = input ? mx_host_source_info : mx_host_output_info;
    (void)get(host, index, nullptr, 0, &needed);
    if (!needed || needed > 64 * 1024) return false;
    std::vector<uint8_t> buffer(needed);
    if (get(host, index, buffer.data(), buffer.size(), &needed) != 0) return false;
    return needed == expected.size() && std::memcmp(buffer.data(), expected.data(), needed) == 0;
}
bool stamp_binding(mx_stamp s, const Timeline& t) {
    return s.generation == t.generation && s.revision == t.revision && s.clock_id == t.clock_id;
}
bool identity(const Endpoint& e) {
    using mixyard::inventory::identifier;
    try {
        identifier(e.observed_id); identifier(e.endpoint_id); identifier(e.provenance);
        identifier(e.consent); identifier(e.exclusions);
    } catch (...) { return false; }
    return e.injected;
}
// Encode only at this output edge. Never alter host storage or sibling routes.
bool encode(myw_format f, const double* samples, size_t count, uint8_t* bytes) {
    for (size_t i = 0; i < count; ++i) {
        if (!std::isfinite(samples[i])) return false;
        uint64_t word = 0;
        if (f.container_bits == 64) std::memcpy(&word, samples + i, sizeof(double));
        else {
            if (std::abs(samples[i]) > std::numeric_limits<float>::max()) return false;
            const float value = static_cast<float>(samples[i]);
            uint32_t narrow = 0; std::memcpy(&narrow, &value, sizeof(value)); word = narrow;
        }
        const size_t width = f.container_bits / 8;
        for (size_t j = 0; j < width; ++j) bytes[i * width + j] = static_cast<uint8_t>(word >> (j * 8));
    }
    return true;
}
}
std::string source_spec(const Endpoint& e) {
    return "{\"observedId\":" + mixyard::inventory::quoted(e.observed_id) + ",\"encoding\":\"" + encoding_name(e.format) + "\"" + suffix(e);
}
std::string output_spec(const Endpoint& e) {
    return "{\"observedId\":" + mixyard::inventory::quoted(e.observed_id) + suffix(e);
}
Bridge::Bridge(mx_host* host, Timeline timeline) : host_(host), timeline_(std::move(timeline)) {}
Bridge::~Bridge() { cancel(); if (!host_revoked_) mx_host_revoke(host_); }
myw_port* Bridge::source(size_t i) const { return i < sources_.size() ? sources_[i]->queue : nullptr; }
myw_port* Bridge::output(size_t i) const { return i < outputs_.size() ? outputs_[i]->queue : nullptr; }
void Bridge::cancel() {
    cancelled_.store(true, std::memory_order_release);
    for (const auto& p : sources_) myw_port_revoke(p->queue);
    for (const auto& p : outputs_) myw_port_revoke(p->queue);
}
Result Bridge::stop(Result result) {
    cancel();
    if (!host_revoked_) { mx_host_revoke(host_); host_revoked_ = true; }
    return result;
}
Result Bridge::create(mx_host* host, Timeline timeline, const std::vector<Endpoint>& sources,
                      const std::vector<Endpoint>& outputs, Authority authority, std::unique_ptr<Bridge>& out) {
    if (out || !host || !authority.admit) return Result::invalid;
    if (timeline.kind != ClockKind::injected_common_timeline || !timeline.generation ||
        !timeline.revision || !timeline.clock_id || timeline.evidence.empty()) return Result::clock;
    if (mx_host_source_count(host) != sources.size() || mx_host_output_count(host) != outputs.size()) return Result::binding;
    try {
        // Validate the WHOLE immutable observation/authority set before allocating
        // queues or exposing anything to an endpoint worker.
        uint32_t rate = 0;
        for (int direction : {MYW_CAPTURE, MYW_RENDER}) {
            const auto& endpoints = direction == MYW_CAPTURE ? sources : outputs;
            for (size_t i = 0; i < endpoints.size(); ++i) {
                const auto& e = endpoints[i];
                if (!identity(e) || !authority.admit(authority.context, e, direction, timeline) || !encoding(e.format)) return Result::binding;
                if (!rate) rate = e.format.rate;
                if (e.format.rate != rate) return Result::clock;
                if (direction == MYW_RENDER && (!e.owned_output || !e.format.floating ||
                    (e.format.container_bits == 32 && !e.allow_float32_destination))) return Result::binding;
                for (size_t j = 0; j < i; ++j)
                    if (endpoints[j].endpoint_id == e.endpoint_id || endpoints[j].observed_id == e.observed_id) return Result::binding;
                if (direction == MYW_CAPTURE) {
                    if (e.owned_output) return Result::binding;
                    for (const auto& output : outputs)
                        if (e.endpoint_id == output.endpoint_id || e.observed_id == output.observed_id) return Result::binding;
                }
                const auto spec = direction == MYW_CAPTURE ? source_spec(e) : output_spec(e);
                if (!host_info(host, i, direction == MYW_CAPTURE, spec)) return Result::binding;
            }
        }
        auto bridge = std::unique_ptr<Bridge>(new Bridge(host, std::move(timeline)));
        for (int direction : {MYW_CAPTURE, MYW_RENDER}) {
            const auto& endpoints = direction == MYW_CAPTURE ? sources : outputs;
            auto& ports = direction == MYW_CAPTURE ? bridge->sources_ : bridge->outputs_;
            for (const auto& e : endpoints) {
                auto p = std::make_unique<Port>(); p->endpoint = e;
                const myw_binding binding{bridge->timeline_.generation, bridge->timeline_.revision};
                if (myw_port_create(e.format, binding, bridge->timeline_.clock_id, &p->queue) != MYW_OK) return Result::exhausted;
                ports.push_back(std::move(p));
            }
        }
        bridge->inputs_.resize(sources.size());
        out = std::move(bridge); return Result::ok;
    } catch (const std::bad_alloc&) { return Result::exhausted; }
      catch (...) { return Result::binding; }
}
Result Bridge::process(uint64_t now_ns, mx_stamp engine, uint32_t frames, const MappedTime* times, size_t count) {
    if (cancelled_.load(std::memory_order_acquire)) return stop(Result::cancelled);
    if (!frames || frames > MYW_MAX_FRAMES || count != sources_.size() || (count && !times) ||
        !stamp_binding(engine, timeline_) || engine.sequence != sequence_ || engine.first_frame != first_frame_ ||
        sequence_ == UINT64_MAX || first_frame_ > UINT64_MAX - frames) return stop(Result::invalid);
    const myw_binding binding{timeline_.generation, timeline_.revision};
    for (size_t i = 0; i < sources_.size(); ++i) {
        auto& p = *sources_[i];
        inputs_[i] = {};
        inputs_[i].encoding = encoding(p.endpoint.format);
        inputs_[i].sample_rate_hz = p.endpoint.format.rate;
        inputs_[i].channels = p.endpoint.format.channels;
        inputs_[i].stamp = engine; // Fresh missing input, never historical bytes.
        myw_block block{};
        const auto result = myw_port_pop(p.queue, binding, p.bytes.data(), p.bytes.size(), &block);
        if (result != MYW_OK) continue; // Empty/lost/revoked source is isolated silence.
        const auto& mapped = times[i];
        if (block.frames != frames || !stamp_binding(mapped.engine, timeline_) ||
            mapped.engine.sequence != block.stamp.sequence || mapped.engine.first_frame != engine.first_frame ||
            block.stamp.first_frame != engine.first_frame || block.stamp.device_frame != mapped.device_frame ||
            block.stamp.qpc_100ns != mapped.qpc_100ns || block.stamp.packet_offset != mapped.packet_offset)
            return stop(Result::source_time);
        inputs_[i].bytes = p.bytes.data();
        inputs_[i].byte_count = frames * myw_frame_bytes(p.endpoint.format);
        inputs_[i].stamp = mapped.engine;
    }
    if (cancelled_.load(std::memory_order_acquire)) return stop(Result::cancelled);
    if (mx_host_process(host_, now_ns, engine, frames, inputs_.data(), inputs_.size()) != 0) return stop(Result::host);
    for (size_t i = 0; i < outputs_.size(); ++i) {
        auto& p = *outputs_[i];
        if (myw_port_state(p.queue) != MYW_OK) continue;
        const double* samples = nullptr; size_t sample_count = 0;
        if (mx_host_output(host_, i, &samples, &sample_count) != 0 || !samples ||
            sample_count != static_cast<size_t>(frames) * p.endpoint.format.channels) return stop(Result::host);
        if (!encode(p.endpoint.format, samples, sample_count, p.bytes.data())) {
            myw_port_revoke(p.queue); continue;
        }
        if (cancelled_.load(std::memory_order_acquire)) return stop(Result::cancelled);
        // This queue uses ENGINE timeline metadata. It is not a fabricated QPC
        // observation; native render timing is observed separately by the worker.
        myw_stamp stamp{binding, timeline_.clock_id, p.sequence, p.first_frame, 0, 0, 0, 0};
        if (myw_port_push(p.queue, p.endpoint.format, stamp, p.bytes.data(), frames,
                          frames * myw_frame_bytes(p.endpoint.format)) == MYW_OK) {
            ++p.sequence; p.first_frame += frames;
        }
    }
    ++sequence_; first_frame_ += frames;
    return cancelled_.load(std::memory_order_acquire) ? stop(Result::cancelled) : Result::ok;
}
} // namespace mixyard_windows_host
