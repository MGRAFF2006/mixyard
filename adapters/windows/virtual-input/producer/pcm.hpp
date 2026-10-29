// SPDX-License-Identifier: MIT
#pragma once
#include "../../worker.h"
#include "../transport.hpp"
#include <array>
#include <cstring>

namespace mixyard_producer {
// This is the existing driver's exact destination constraint, not a source or
// engine format. Converting another output here must be an explicit host choice.
inline bool ingress_format(myw_format format) {
    const myw_format exact{48000, 3, 2, 32, 32, 1};
    return myw_format_equal(format, exact) != 0;
}
class PcmEncoder {
    myw_binding binding_;
    uint64_t clock_id_;
    mixyard_input::Transport state_{};
    bool active_ = false;
    uint64_t source_sequence_ = 0;
    uint32_t source_offset_ = 0;
    void put(uint8_t* p, uint64_t word, size_t width) const {
        for (size_t i = 0; i < width; ++i) p[i] = static_cast<uint8_t>(word >> (i * 8));
    }
public:
    using Packet = std::array<uint8_t, mixyard_input::max_packet_bytes>;
    PcmEncoder(myw_format format, myw_binding binding, uint64_t clock_id,
               uint64_t driver_generation, uint64_t driver_epoch)
        : binding_(binding), clock_id_(clock_id) {
        state_.initialize();
        active_ = ingress_format(format) && binding.generation && binding.revision && clock_id &&
            driver_generation && driver_epoch &&
            state_.begin(1, driver_epoch) == mixyard_input::Result::accepted;
        if (active_) state_.generation = driver_generation;
    }
    const mixyard_input::Transport& state() const { return state_; }
    bool active() const { return active_; }
    void revoke() { active_ = false; state_.cleanup(1); }
    // One IOCTL packet at a time, outside WASAPI servicing. A source block may
    // be split into <=128-frame portions without changing any sample word.
    bool encode(myw_format format, const myw_block& block, const void* bytes, size_t length,
                 uint32_t offset, uint32_t frames, Packet& packet, size_t& packet_bytes) {
        packet = {}; packet_bytes = 0;
        if (!active_) return false;
        if (!ingress_format(format) || !bytes || !block.frames || block.frames > MYW_MAX_FRAMES ||
            length != static_cast<size_t>(block.frames) * 8 || offset > block.frames ||
            !frames || frames > mixyard_input::max_frames || frames > block.frames - offset ||
            block.stamp.binding.generation != binding_.generation || block.stamp.binding.revision != binding_.revision ||
            block.stamp.clock_id != clock_id_ || (block.stamp.flags & ~2u) ||
            block.stamp.sequence != source_sequence_ || offset != source_offset_ ||
            source_sequence_ == UINT64_MAX ||
            block.stamp.first_frame > UINT64_MAX - offset ||
            block.stamp.first_frame + offset != state_.next_frame) { revoke(); return false; }
        using namespace mixyard_input;
        packet_bytes = header_bytes + frames * 8;
        auto* p = packet.data();
        put(p, magic, 4); put(p + 4, version, 2); put(p + 6, header_bytes, 2);
        put(p + 8, packet_bytes, 4); put(p + 12, frames, 4);
        put(p + 16, state_.generation, 8); put(p + 24, state_.epoch, 8);
        put(p + 32, binding_.revision, 8); put(p + 40, state_.next_frame, 8);
        put(p + 48, 48000, 4); put(p + 52, 2, 2); put(p + 54, 1, 2);
        put(p + 56, state_.next_sequence, 8);
        if (!(block.stamp.flags & 2u))
            std::memcpy(p + header_bytes, static_cast<const uint8_t*>(bytes) + offset * 8, frames * 8);
        const auto result = state_.submit(1, packet.data(), packet_bytes);
        if (result != Result::accepted && result != Result::accepted_overwrite) {
            packet = {}; packet_bytes = 0; revoke(); return false;
        }
        source_offset_ += frames;
        if (source_offset_ == block.frames) { source_offset_ = 0; ++source_sequence_; }
        return true;
    }
};
}
