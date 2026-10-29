// SPDX-License-Identifier: MIT
#pragma once
#include <cstdint>

namespace mixyard::inventory::macos {
// Storage consistency only. The caller separately validates the LPCM encoding,
// flags, supported bit depth, finite integer rate and representable channel map.
// A template lets the actual ASBD predicate run in portable injected tests too.
template<class Description>
bool pcm_storage_consistent(const Description& f, bool floating, bool packed,
    bool aligned_high, bool non_interleaved) {
    // Uncompressed PCM packets contain one frame. For planar PCM, the byte
    // fields describe one channel's buffer rather than all channel buffers.
    if (!f.mChannelsPerFrame || !f.mBitsPerChannel || f.mFramesPerPacket != 1 ||
        !f.mBytesPerFrame || f.mBytesPerPacket != f.mBytesPerFrame) return false;
    const std::uint64_t lanes = non_interleaved ? 1 : f.mChannelsPerFrame;
    if (f.mBytesPerFrame % lanes) return false;
    const auto bytes_per_sample = f.mBytesPerFrame / lanes;
    if (floating) {
        return packed && !aligned_high &&
            ((f.mBitsPerChannel == 32 && bytes_per_sample == 4) ||
             (f.mBitsPerChannel == 64 && bytes_per_sample == 8));
    }
    if (!bytes_per_sample || bytes_per_sample > 4) return false;
    const auto container_bits = bytes_per_sample * 8;
    // Packed integer PCM has no padding. Unpacked low/high-aligned samples
    // may use a wider supported container, e.g. 24 significant bits in 32.
    return packed ? !aligned_high && f.mBitsPerChannel == container_bits :
        f.mBitsPerChannel <= container_bits;
}
}
