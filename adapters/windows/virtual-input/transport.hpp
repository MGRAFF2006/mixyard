// SPDX-License-Identifier: MIT
// Original Mixyard ingress scaffold. All calls require external serialization.
#pragma once
#include <stddef.h>
#include <stdint.h>

namespace mixyard_input {
constexpr uint32_t magic = 0x4958594d; // little-endian "MYXI"
constexpr uint16_t version = 1;
constexpr size_t header_bytes = 64;
constexpr size_t max_frames = 128;
constexpr size_t retained_packets = 8; // proof storage budget, not endpoint capacity
constexpr size_t max_packet_bytes = header_bytes + max_frames * 8;

// Wire ABI, little endian, no implicit padding or native pointers:
// 0:u32 magic, 4:u16 version, 6:u16 headerBytes, 8:u32 packetBytes,
// 12:u32 frames, 16:u64 generation, 24:u64 epoch, 32:u64 revision,
// 40:u64 firstFrame, 48:u32 rate=48000, 52:u16 channels=2,
// 54:u16 format=1 (interleaved IEEE754 Float32 bits), 56:u64 sequence.
// Exactly frames*2 u32 sample words follow. No floating point in kernel code.
enum class Result : uint32_t {
    accepted, accepted_overwrite, malformed, unauthorized, stale,
    out_of_order, exhausted, unavailable
};

inline uint16_t load16(const uint8_t* p) {
    return static_cast<uint16_t>(p[0] | (static_cast<uint16_t>(p[1]) << 8));
}
inline uint32_t load32(const uint8_t* p) {
    return static_cast<uint32_t>(load16(p)) |
           (static_cast<uint32_t>(load16(p + 2)) << 16);
}
inline uint64_t load64(const uint8_t* p) {
    return static_cast<uint64_t>(load32(p)) |
           (static_cast<uint64_t>(load32(p + 4)) << 32);
}

struct QueuedPacket {
    uint64_t generation;
    uint64_t epoch;
    uint64_t revision;
    uint64_t first_frame;
    uint64_t sequence;
    uint32_t frames;
    uint32_t samples[max_frames * 2];
};

struct Transport {
    uintptr_t owner;
    uint64_t generation;
    uint64_t epoch;
    uint64_t next_frame;
    uint64_t next_sequence;
    uint64_t revision;
    uint64_t overwritten_packets;
    QueuedPacket packets[retained_packets];

    // Call once on newly allocated, exclusively owned storage before any use.
    void initialize() {
        owner = 0;
        generation = 0;
        clear();
    }
    void clear() {
        epoch = next_frame = next_sequence = revision = overwritten_packets = 0;
        for (size_t i = 0; i < retained_packets; ++i) {
            packets[i].generation = packets[i].epoch = packets[i].revision = 0;
            packets[i].first_frame = packets[i].sequence = 0;
            packets[i].frames = 0;
            for (size_t j = 0; j < max_frames * 2; ++j) packets[i].samples[j] = 0;
        }
    }
    Result begin(uintptr_t file, uint64_t new_epoch) {
        if (!file || !new_epoch) return Result::malformed;
        if (owner) return Result::unauthorized;
        if (generation == UINT64_MAX) return Result::exhausted;
        clear();
        ++generation;
        epoch = new_epoch;
        owner = file;
        return Result::accepted;
    }
    Result cleanup(uintptr_t file) {
        if (!file || owner != file) return Result::unauthorized;
        owner = 0;
        clear(); // Retain generation so stale packets cannot re-enter on reconnect.
        return Result::accepted;
    }
    Result submit(uintptr_t file, const void* buffer, size_t length) {
        if (!file || owner != file) return Result::unauthorized;
        if (!buffer || length < header_bytes || length > max_packet_bytes)
            return Result::malformed;
        const auto* b = static_cast<const uint8_t*>(buffer);
        const uint32_t frames = load32(b + 12);
        if (load32(b) != magic || load16(b + 4) != version ||
            load16(b + 6) != header_bytes || load32(b + 8) != length ||
            !frames || frames > max_frames || length != header_bytes + frames * 8 ||
            load32(b + 48) != 48000 || load16(b + 52) != 2 || load16(b + 54) != 1)
            return Result::malformed;
        if (load64(b + 16) != generation || load64(b + 24) != epoch)
            return Result::stale;
        const uint64_t rev = load64(b + 32);
        if (!rev || rev < revision || load64(b + 40) != next_frame ||
            load64(b + 56) != next_sequence) return Result::out_of_order;
        if (next_frame > UINT64_MAX - frames || next_sequence == UINT64_MAX)
            return Result::exhausted;
        for (size_t i = 0; i < frames * 2; ++i)
            if ((load32(b + header_bytes + i * 4) & 0x7f800000u) == 0x7f800000u)
                return Result::malformed; // Reject both infinities and every NaN.
        auto& p = packets[next_sequence % retained_packets];
        p.generation = generation;
        p.epoch = epoch;
        p.revision = rev;
        p.first_frame = next_frame;
        p.sequence = next_sequence;
        p.frames = frames;
        for (size_t i = 0; i < max_frames * 2; ++i)
            p.samples[i] = i < frames * 2 ? load32(b + header_bytes + i * 4) : 0;
        next_frame += frames;
        revision = rev;
        ++next_sequence;
        if (next_sequence > retained_packets) {
            ++overwritten_packets;
            return Result::accepted_overwrite;
        }
        return Result::accepted;
    }
    // Inspection of queued storage ONLY, never a sample-effective delivery ACK.
    // Each caller owns its cursor. Inspection never drains another reader.
    Result copy_queued(uint64_t gen, uint64_t sequence, QueuedPacket* out) const {
        if (!out) return Result::malformed;
        *out = {}; // Even stale/missing data yields initialized zeros.
        if (!owner || gen != generation) return Result::stale;
        if (sequence >= next_sequence || next_sequence - sequence > retained_packets)
            return Result::unavailable;
        *out = packets[sequence % retained_packets];
        return Result::accepted;
    }
};
} // namespace mixyard_input
