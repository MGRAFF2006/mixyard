// SPDX-License-Identifier: MIT
#pragma once
#include <cstddef>
#include <cstdint>

namespace mixyard {
// Existing fixture wire contract, never a generic engine format negotiation.
constexpr uint64_t maxPosition = (uint64_t{1} << 53) - 1;
constexpr uint32_t packetFrames = 4096, creditFrames = 4096;
struct Header {
    uint32_t version = 1, sampleRate = 48000, channels = 2, format = 1;
    uint64_t generation = 0, epoch = 0, revision = 0, position = 0;
    uint32_t frames = 0, bytes = 0;
};
static_assert(sizeof(Header) == 56);
// The same admission predicate runs at the authenticated native broker boundary
// and in portable hostile-packet tests. No copy or ring mutation precedes it.
inline bool packetShape(const Header& h, size_t messageBytes) noexcept {
    return h.version == 1 && h.sampleRate == 48000 && h.channels == 2 && h.format == 1 &&
        h.frames && h.frames <= packetFrames && h.bytes == h.frames * 8 &&
        messageBytes == sizeof(Header) + h.bytes && h.generation && h.epoch && h.revision &&
        h.position <= maxPosition - h.frames;
}
enum class PacketAdmission { Accepted, Stale, Backpressure };
inline PacketAdmission packetAdmission(const Header& h, uint64_t generation, uint64_t epoch,
                                      uint64_t nextPosition, uint64_t revision, uint32_t credits) noexcept {
    if (!generation || h.generation != generation || !epoch || h.epoch != epoch ||
        h.position != nextPosition || h.revision < revision) return PacketAdmission::Stale;
    return h.frames > credits ? PacketAdmission::Backpressure : PacketAdmission::Accepted;
}
} // namespace mixyard
