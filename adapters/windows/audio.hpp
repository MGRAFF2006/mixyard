#pragma once

#include <algorithm>
#include <array>
#include <atomic>
#include <cstdint>
#include <cstring>
#include <ostream>
#include <stdexcept>

namespace mixyard {
constexpr std::size_t packet_frames = 1024;
constexpr std::size_t packet_bytes = packet_frames * 2 * 4;
constexpr std::size_t queue_slots = 64;

struct Packet {
    std::array<std::uint8_t, packet_bytes> data{};
    std::uint64_t first_frame = 0;
    std::uint32_t frames = 0;
};

struct PacketFault {
    std::uint64_t first_frame = 0, device_position = 0, qpc_100ns = 0;
    std::uint32_t flags = 0;
};

// One capture producer, one disk consumer. No allocation or locks after setup.
template<class T>
class SpscQueue {
    std::array<T, queue_slots> slots_{};
    alignas(64) std::atomic<std::uint64_t> write_{0};
    alignas(64) std::atomic<std::uint64_t> read_{0};
public:
    static_assert(std::atomic<std::uint64_t>::is_always_lock_free);
    T* reserve() noexcept {
        const auto w = write_.load(std::memory_order_relaxed);
        if (w - read_.load(std::memory_order_acquire) == queue_slots) return nullptr;
        return &slots_[w % queue_slots];
    }
    void publish() noexcept { write_.fetch_add(1, std::memory_order_release); }
    const T* front() noexcept {
        const auto r = read_.load(std::memory_order_relaxed);
        if (r == write_.load(std::memory_order_acquire)) return nullptr;
        return &slots_[r % queue_slots];
    }
    void consume() noexcept { read_.fetch_add(1, std::memory_order_release); }
};

using Queue = SpscQueue<Packet>;
using FaultQueue = SpscQueue<PacketFault>;

inline bool enqueue_chunk(Queue& queue, std::uint64_t first_frame, const std::uint8_t* data,
                          std::uint32_t frames, std::uint16_t block_align, bool silent) noexcept {
    auto packet = queue.reserve();
    if (!packet) return false;
    packet->first_frame = first_frame; packet->frames = frames;
    const auto bytes = frames * block_align;
    if (silent) std::memset(packet->data.data(), 0, bytes);
    else std::memcpy(packet->data.data(), data, bytes);
    queue.publish(); return true;
}

// Packet boundary evidence is independent of whether any PCM chunk survives.
inline bool record_fault(FaultQueue& queue, PacketFault fault) noexcept {
    auto slot = queue.reserve();
    if (!slot) return false;
    *slot = fault; queue.publish(); return true;
}

inline void write_fault(std::ostream& out, const PacketFault& fault) {
    out << "{\"code\":\"wasapi-packet-flags\",\"firstFrame\":" << fault.first_frame
        << ",\"flags\":" << fault.flags << ",\"devicePosition\":" << fault.device_position
        << ",\"qpc100ns\":" << fault.qpc_100ns << ",\"missingFrames\":null}\n";
}

inline void write_fault_overflow(std::ostream& out, std::uint64_t records) {
    out << "{\"code\":\"fault-metadata-overflow\",\"records\":" << records
        << ",\"firstFrame\":null,\"missingFrames\":null}\n";
}

struct Format {
    std::uint32_t rate;
    std::uint16_t channels;
    std::uint16_t bits;
    bool floating;
    std::uint16_t valid_bits = 0;
    std::uint16_t block_align() const { return channels * (bits / 8); }
    void validate() const {
        if (rate < 8000 || rate > 192000 || (channels != 1 && channels != 2) ||
            (floating ? bits != 32 : (bits != 16 && bits != 24 && bits != 32)) || valid_bits > bits ||
            (floating && valid_bits && valid_bits != bits))
            throw std::runtime_error("unsupported-format: require mono/stereo, 8-192 kHz, float32 or PCM16/24/32");
    }
};

inline void le(std::ostream& out, std::uint32_t value, int bytes) {
    for (int i = 0; i < bytes; ++i) out.put(static_cast<char>((value >> (8 * i)) & 255));
}

class Wav {
    std::ostream& out_;
    Format format_;
    std::uint64_t frames_ = 0;
    void header() {
        const auto bytes = static_cast<std::uint32_t>(frames_ * format_.block_align());
        out_.write("RIFF", 4); le(out_, bytes + (bytes & 1) + (format_.floating ? 72 : 60), 4);
        out_.write("WAVEfmt ", 8);
        le(out_, 40, 4); le(out_, 0xfffe, 2); le(out_, format_.channels, 2);
        le(out_, format_.rate, 4); le(out_, format_.rate * format_.block_align(), 4);
        le(out_, format_.block_align(), 2); le(out_, format_.bits, 2); le(out_, 22, 2);
        le(out_, format_.valid_bits ? format_.valid_bits : format_.bits, 2);
        le(out_, format_.channels == 1 ? 4 : 3, 4); // Front center / front L+R.
        le(out_, format_.floating ? 3 : 1, 4); le(out_, 0, 2); le(out_, 0x10, 2);
        const char guid_tail[]{static_cast<char>(0x80), 0, 0, static_cast<char>(0xaa), 0, 0x38, static_cast<char>(0x9b), 0x71};
        out_.write(guid_tail, 8);
        if (format_.floating) { out_.write("fact", 4); le(out_, 4, 4); le(out_, static_cast<std::uint32_t>(frames_), 4); }
        out_.write("data", 4); le(out_, bytes, 4);
    }
public:
    Wav(std::ostream& out, Format format) : out_(out), format_(format) {
        format_.validate(); header();
    }
    std::uint64_t frames() const { return frames_; }
    void append(const std::uint8_t* data, std::uint64_t frames) {
        if ((frames_ + frames) * format_.block_align() > UINT32_MAX - 73)
            throw std::runtime_error("wav-size-limit: RIFF would exceed 4 GiB");
        const std::array<char, packet_bytes> silence{};
        while (frames) {
            const auto count = std::min<std::uint64_t>(frames, packet_frames);
            const auto bytes = count * format_.block_align();
            out_.write(data ? reinterpret_cast<const char*>(data) : silence.data(), static_cast<std::streamsize>(bytes));
            if (!out_) throw std::runtime_error("disk-write-failed: WAV output stopped; check free space and permissions");
            frames_ += count;
            frames -= count;
            if (data) data += count * format_.block_align();
        }
    }
    // Called only by disk writer; capture failure can still leave a valid partial WAV.
    void finish() {
        if ((frames_ * format_.block_align()) & 1) out_.put(0);
        out_.seekp(0); header(); out_.flush();
        if (!out_) throw std::runtime_error("disk-write-failed: could not finalize WAV header");
    }
};

// Gap offsets count received frames, not an inferred hardware clock timeline.
inline std::uint64_t pad_gap(Wav& wav, std::uint64_t next_frame) {
    if (next_frame < wav.frames()) throw std::runtime_error("queue-order-error");
    const auto gap = next_frame - wav.frames();
    wav.append(nullptr, gap);
    return gap;
}
} // namespace mixyard
