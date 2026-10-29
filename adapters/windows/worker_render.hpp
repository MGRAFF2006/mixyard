// SPDX-License-Identifier: MIT
#pragma once
#include "worker.h"
#include <algorithm>
#include <array>
#include <cstring>

namespace mixyard_worker {
// Sole render consumer, including partial packets across native buffer calls.
// Construct/touch before Start. Never reuse a cursor with a different port.
struct RenderCursor {
    std::array<uint8_t, MYW_MAX_FRAMES * MYW_MAX_FRAME_BYTES> bytes{};
    myw_block block{};
    uint32_t offset = 0;
    int fill(myw_port* port, myw_binding binding, size_t width, uint8_t* out,
             uint32_t frames, uint64_t& missing) {
        if (!out || !width || width > MYW_MAX_FRAME_BYTES || frames > MYW_MAX_FRAMES) return MYW_INVALID;
        const size_t capacity = frames * width;
        std::memset(out, 0, capacity);
        myw_format format{};
        myw_binding actual{};
        uint64_t clock = 0;
        if (myw_port_info(port, &format, &actual, &clock) != MYW_OK ||
            myw_frame_bytes(format) != width || actual.generation != binding.generation ||
            actual.revision != binding.revision) {
            myw_port_revoke(port); return MYW_STALE;
        }
        int result = myw_port_state(port);
        if (result != MYW_OK) return result;
        uint32_t filled = 0;
        while (filled < frames) { // <=256 one-frame blocks, then return.
            if (offset == block.frames) {
                result = myw_port_pop(port, binding, bytes.data(), MYW_MAX_FRAMES * width, &block);
                offset = 0;
                if (result == MYW_EMPTY) {
                    missing += frames - filled;
                    result = MYW_OK; break;
                }
                if (result != MYW_OK) break;
            }
            const auto count = std::min(frames - filled, block.frames - offset);
            std::memcpy(out + filled * width, bytes.data() + offset * width, count * width);
            offset += count; filled += count;
        }
        if (result == MYW_OK) result = myw_port_state(port);
        if (result != MYW_OK) std::memset(out, 0, capacity);
        return result;
    }
};
}
