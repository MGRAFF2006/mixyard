// SPDX-License-Identifier: MIT
#pragma once

#include <array>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <limits>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace mixyard::worker {
// Resource admission limits of this transport, not product format/mix limits.
constexpr uint32_t slots = 8, maxFrames = 4096, maxBytes = 65536, maxChannels = 32;
static_assert(std::atomic<uint32_t>::is_always_lock_free);
static_assert(sizeof(float) == 4 && sizeof(double) == 8 &&
              std::numeric_limits<float>::is_iec559 && std::numeric_limits<double>::is_iec559);

enum class Precision : uint32_t { Float32 = 4, Float64 = 8 };
enum class Origin { Synthetic, ProcessTap, AdaptedEngine };
struct Format {
    uint32_t sampleRateHz = 0;
    std::vector<std::string> channels;
    Precision precision = Precision::Float32;
    uint32_t sampleBytes() const noexcept { return static_cast<uint32_t>(precision); }
    uint32_t frameBytes() const noexcept { return sampleBytes() * static_cast<uint32_t>(channels.size()); }
};
struct Binding { uint64_t generation = 0, revision = 0; };
inline bool operator==(Binding a, Binding b) noexcept {
    return a.generation == b.generation && a.revision == b.revision;
}
struct Stamp {
    Binding binding;
    uint64_t clockId = 0, sequence = 0, firstFrame = 0, capturedNs = 0, deadlineNs = 0;
};
struct Provenance {
    Origin origin;
    // Configured/observed identity, not a credential or a caller-supplied proof.
    std::string source;
};
struct Block {
    Stamp stamp{};
    uint32_t frames = 0;
    alignas(8) std::array<unsigned char, maxBytes> pcm{};
};
enum class Fault : uint32_t {
    None, Revoked, Binding, Clock, Sequence, Shape, Nonfinite, Expired, Overflow, Underflow
};

// One producer and one consumer, each with exclusive ownership of its cursor.
// Control may revoke concurrently. Configuration/storage are constructed and
// touched before callbacks start; destruction requires native callback quiescence.
// A fault is terminal. Logical clearing never races a consumer by wiping slots.
// A new session/revision/clock needs a new object, never a reset of these cursors.
class Queue {
    const Format format_;
    const Binding binding_;
    const uint64_t clockId_;
    const Provenance provenance_;
    std::array<Block, slots> blocks_{};
    alignas(64) std::atomic<uint32_t> write_{0};
    alignas(64) std::atomic<uint32_t> read_{0};
    std::atomic<uint32_t> fault_{0};
    // Producer-only timeline state. No atomic uint64_t operations in callbacks.
    bool havePrevious_ = false;
    uint64_t nextSequence_ = 0, nextFrame_ = 0, lastCaptured_ = 0, lastProducerNow_ = 0;
    // Consumer-only monotonic observation, independent of producer state.
    bool haveConsumerNow_ = false;
    uint64_t lastConsumerNow_ = 0;

    bool finite(const unsigned char* bytes, uint32_t samples) const noexcept {
        for (uint32_t i = 0; i < samples; ++i) {
            if (format_.precision == Precision::Float32) {
                float value; std::memcpy(&value, bytes + i * 4, 4);
                if (!std::isfinite(value)) return false;
            } else {
                double value; std::memcpy(&value, bytes + i * 8, 8);
                if (!std::isfinite(value)) return false;
            }
        }
        return true;
    }
public:
    Queue(Format format, Binding binding, uint64_t clockId, Provenance provenance)
        : format_(std::move(format)), binding_(binding), clockId_(clockId), provenance_(std::move(provenance)) {
        if (!format_.sampleRateHz || format_.channels.empty() || format_.channels.size() > maxChannels ||
            (format_.precision != Precision::Float32 && format_.precision != Precision::Float64) ||
            !binding.generation || !binding.revision || !clockId || provenance_.source.empty() ||
            provenance_.source.size() > 4096)
            throw std::invalid_argument("Invalid exact PCM worker contract");
        for (size_t i = 0; i < format_.channels.size(); ++i) {
            if (format_.channels[i].empty() || format_.channels[i].size() > 64)
                throw std::invalid_argument("Channel labels must be nonempty and bounded");
            for (size_t j = 0; j < i; ++j)
                if (format_.channels[j] == format_.channels[i])
                    throw std::invalid_argument("Ordered channel labels must be unique");
        }
    }
    Queue(const Queue&) = delete;
    Queue& operator=(const Queue&) = delete;
    const Format& format() const noexcept { return format_; }
    Binding binding() const noexcept { return binding_; }
    uint64_t clockId() const noexcept { return clockId_; }
    const Provenance& provenance() const noexcept { return provenance_; }
    Fault fault() const noexcept { return static_cast<Fault>(fault_.load(std::memory_order_acquire)); }
    bool fail(Fault reason) noexcept {
        uint32_t expected = 0;
        fault_.compare_exchange_strong(expected, static_cast<uint32_t>(reason),
                                      std::memory_order_acq_rel, std::memory_order_acquire);
        return false;
    }
    void revoke() noexcept { fail(Fault::Revoked); }
    // Producer-only credit observation. Full publication is terminal; a worker
    // can inspect credit before doing bounded non-RT processing, never RT retry.
    bool writable() const noexcept {
        return fault() == Fault::None &&
            write_.load(std::memory_order_relaxed) - read_.load(std::memory_order_acquire) < slots;
    }
    // Canonical transport is interleaved bytes. Planar native ingress uses the
    // same routine with one buffer per ordered channel; no numeric conversion.
    bool push(uint64_t nowNs, Stamp stamp, uint32_t frames, const void* const* buffers,
              const uint32_t* byteSizes, uint32_t count, bool planar) noexcept {
        if (fault() != Fault::None) return false;
        if (!(stamp.binding == binding_)) return fail(Fault::Binding);
        if (stamp.clockId != clockId_) return fail(Fault::Clock);
        if (havePrevious_ && nowNs < lastProducerNow_) return fail(Fault::Clock);
        if (stamp.deadlineNs < stamp.capturedNs || nowNs < stamp.capturedNs || nowNs > stamp.deadlineNs)
            return fail(Fault::Expired);
        if (stamp.sequence == UINT64_MAX || stamp.firstFrame > UINT64_MAX - frames ||
            (havePrevious_ && (stamp.sequence != nextSequence_ || stamp.firstFrame != nextFrame_ ||
                               stamp.capturedNs < lastCaptured_))) return fail(Fault::Sequence);
        const auto channels = static_cast<uint32_t>(format_.channels.size());
        const auto bytes = uint64_t(frames) * format_.frameBytes();
        if (!frames || frames > maxFrames || bytes > maxBytes || !buffers || !byteSizes ||
            count != (planar ? channels : 1u)) return fail(Fault::Shape);
        const auto laneBytes = frames * format_.sampleBytes();
        for (uint32_t i = 0; i < count; ++i) {
            if (!buffers[i] || byteSizes[i] != (planar ? laneBytes : bytes)) return fail(Fault::Shape);
            if (!finite(static_cast<const unsigned char*>(buffers[i]), planar ? frames : frames * channels))
                return fail(Fault::Nonfinite);
        }
        const auto w = write_.load(std::memory_order_relaxed);
        if (w - read_.load(std::memory_order_acquire) >= slots) return fail(Fault::Overflow);
        auto& block = blocks_[w % slots];
        block.stamp = stamp; block.frames = frames;
        if (planar) {
            const auto width = format_.sampleBytes();
            for (uint32_t frame = 0; frame < frames; ++frame)
                for (uint32_t channel = 0; channel < channels; ++channel)
                    std::memcpy(block.pcm.data() + (frame * channels + channel) * width,
                                static_cast<const unsigned char*>(buffers[channel]) + frame * width, width);
        } else std::memcpy(block.pcm.data(), buffers[0], static_cast<size_t>(bytes));
        nextSequence_ = stamp.sequence + 1; nextFrame_ = stamp.firstFrame + frames;
        lastCaptured_ = stamp.capturedNs; lastProducerNow_ = nowNs; havePrevious_ = true;
        if (fault() != Fault::None) return false;
        write_.store(w + 1, std::memory_order_release);
        return fault() == Fault::None;
    }
    // Consumer only. A returned pointer remains owned until release(); check the
    // terminal fault again after copying and zero destination if revoked in flight.
    const Block* front(uint64_t nowNs) noexcept {
        if (fault() != Fault::None) return nullptr;
        if (haveConsumerNow_ && nowNs < lastConsumerNow_) { fail(Fault::Clock); return nullptr; }
        lastConsumerNow_ = nowNs; haveConsumerNow_ = true;
        const auto r = read_.load(std::memory_order_relaxed);
        if (r == write_.load(std::memory_order_acquire)) return nullptr;
        const auto& block = blocks_[r % slots];
        if (nowNs < block.stamp.capturedNs || nowNs > block.stamp.deadlineNs) {
            fail(Fault::Expired); return nullptr;
        }
        return &block;
    }
    void release() noexcept {
        read_.store(read_.load(std::memory_order_relaxed) + 1, std::memory_order_release);
    }
    bool pop(uint64_t nowNs, Binding binding, void* destination, uint32_t capacity, Stamp& stamp,
             uint32_t& frames) noexcept {
        frames = 0; stamp = {};
        if (!destination || capacity > maxBytes) return fail(Fault::Shape);
        std::memset(destination, 0, capacity);
        if (!(binding == binding_)) return fail(Fault::Binding);
        const auto* block = front(nowNs);
        if (!block) return false; // Empty is silence, not a replay or new authority.
        const auto bytes = block->frames * format_.frameBytes();
        if (capacity < bytes) return fail(Fault::Shape);
        std::memcpy(destination, block->pcm.data(), bytes);
        stamp = block->stamp; frames = block->frames;
        release();
        if (fault() != Fault::None) {
            std::memset(destination, 0, capacity); stamp = {}; frames = 0; return false;
        }
        return true;
    }
};

// Render-side cursor supports callback periods unlike producer block sizes.
// firstFrame is in this destination's explicitly bound timeline, not an inferred
// mapping from a source/hardware clock. Every failure zeros the whole callback.
class Renderer {
    Queue& queue_;
    uint32_t offset_ = 0;
public:
    explicit Renderer(Queue& queue) : queue_(queue) {}
    bool render(uint64_t nowNs, uint64_t firstFrame, uint32_t frames,
                void* destination, uint32_t bytes) noexcept {
        if (!destination || bytes > maxBytes) return queue_.fail(Fault::Shape);
        std::memset(destination, 0, bytes);
        const auto width = queue_.format().frameBytes();
        if (!frames || frames > maxFrames || uint64_t(frames) * width != bytes ||
            firstFrame > UINT64_MAX - frames) return queue_.fail(Fault::Shape);
        uint32_t copied = 0;
        // At most slots distinct blocks plus a partially consumed head.
        for (uint32_t i = 0; i <= slots && copied < frames; ++i) {
            const auto* block = queue_.front(nowNs);
            if (!block) { queue_.fail(Fault::Underflow); break; }
            if (block->stamp.firstFrame + offset_ != firstFrame + copied) {
                queue_.fail(Fault::Clock); break;
            }
            const auto available = block->frames - offset_;
            const auto take = available < frames - copied ? available : frames - copied;
            std::memcpy(static_cast<unsigned char*>(destination) + copied * width,
                        block->pcm.data() + offset_ * width, take * width);
            copied += take; offset_ += take;
            if (offset_ == block->frames) { queue_.release(); offset_ = 0; }
        }
        if (copied != frames || queue_.fault() != Fault::None) {
            std::memset(destination, 0, bytes); return false;
        }
        return true;
    }
};
} // namespace mixyard::worker
