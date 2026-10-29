// SPDX-License-Identifier: MIT
#include "worker.h"
#include <array>
#include <atomic>
#include <cstring>
#include <limits>
#include <new>
#include <algorithm>

namespace {
bool binding_equal(myw_binding a, myw_binding b) {
    return a.generation == b.generation && a.revision == b.revision;
}
bool valid_format(myw_format f) {
    unsigned channels = 0;
    for (auto mask = f.channel_mask; mask; mask >>= 1) channels += mask & 1u;
    const auto width = static_cast<uint32_t>(f.channels) * (f.container_bits / 8u);
    return f.rate && f.channels && channels == f.channels && !(f.channel_mask & ~0x3ffffu) &&
        f.valid_bits && f.valid_bits <= f.container_bits && f.floating <= 1 &&
        (f.floating ? ((f.container_bits == 32 || f.container_bits == 64) && f.valid_bits == f.container_bits)
                    : (f.container_bits == 16 || f.container_bits == 24 || f.container_bits == 32)) &&
        width <= MYW_MAX_FRAME_BYTES && f.rate <= UINT32_MAX / width;
}
struct Packet {
    myw_block block{};
    std::array<uint8_t, MYW_MAX_FRAMES * MYW_MAX_FRAME_BYTES> bytes{};
};
}
struct myw_port {
    const myw_format format;
    const myw_binding binding;
    const uint64_t clock_id;
    std::array<Packet, MYW_SLOTS> packets{};
    std::atomic<uint64_t> written{0};
    std::atomic<uint64_t> read{0};
    std::atomic<int> state{MYW_OK};
    uint64_t next_frame = 0; // Producer-owned.
    myw_port(myw_format f, myw_binding b, uint64_t c) : format(f), binding(b), clock_id(c) {}
};
static_assert(std::atomic<uint64_t>::is_always_lock_free);
static_assert(std::atomic<int>::is_always_lock_free);
namespace {
int fail(myw_port* p, int reason) {
    int expected = MYW_OK;
    p->state.compare_exchange_strong(expected, reason, std::memory_order_acq_rel);
    return p->state.load(std::memory_order_acquire);
}
}
extern "C" {
int myw_format_equal(myw_format a, myw_format b) {
    return a.rate == b.rate && a.channel_mask == b.channel_mask && a.channels == b.channels &&
        a.container_bits == b.container_bits && a.valid_bits == b.valid_bits && a.floating == b.floating;
}
size_t myw_frame_bytes(myw_format f) {
    return valid_format(f) ? static_cast<size_t>(f.channels) * (f.container_bits / 8u) : 0;
}
int myw_port_create(myw_format format, myw_binding binding, uint64_t clock_id, myw_port** out) {
    if (!out) return MYW_INVALID;
    *out = nullptr;
    if (!valid_format(format)) return MYW_FORMAT;
    if (!binding.generation || !binding.revision || !clock_id) return MYW_INVALID;
    *out = new (std::nothrow) myw_port(format, binding, clock_id);
    return *out ? MYW_OK : MYW_EXHAUSTED;
}
void myw_port_destroy(myw_port* p) { delete p; }
void myw_port_revoke(myw_port* p) { if (p) (void)fail(p, MYW_STALE); }
int myw_port_state(const myw_port* p) { return p ? p->state.load(std::memory_order_acquire) : MYW_INVALID; }
int myw_port_info(const myw_port* p, myw_format* format, myw_binding* binding, uint64_t* clock_id) {
    if (!p || !format || !binding || !clock_id) return MYW_INVALID;
    *format = p->format; *binding = p->binding; *clock_id = p->clock_id;
    return MYW_OK;
}
int myw_port_push(myw_port* p, myw_format format, myw_stamp stamp,
                  const void* bytes, uint32_t frames, size_t byte_count) {
    if (!p) return MYW_INVALID;
    if (myw_port_state(p) != MYW_OK) return myw_port_state(p);
    if (!myw_format_equal(format, p->format)) return fail(p, MYW_FORMAT);
    if (!binding_equal(stamp.binding, p->binding) || stamp.clock_id != p->clock_id)
        return fail(p, MYW_STALE);
    if (!bytes || !frames || frames > MYW_MAX_FRAMES ||
        byte_count != frames * myw_frame_bytes(p->format)) return fail(p, MYW_INVALID);
    if (stamp.flags & ~2u) return fail(p, MYW_CAPTURE_FAULT); // WASAPI SILENT only; timestamps must be valid.
    const auto w = p->written.load(std::memory_order_relaxed);
    if (stamp.sequence != w || stamp.first_frame != p->next_frame) return fail(p, MYW_ORDER);
    if (w == UINT64_MAX || p->next_frame > UINT64_MAX - frames) return fail(p, MYW_EXHAUSTED);
    if (w - p->read.load(std::memory_order_acquire) == MYW_SLOTS) return fail(p, MYW_FULL);
    auto& packet = p->packets[w % MYW_SLOTS];
    packet.block = {stamp, frames};
    if (stamp.flags & 2u) std::memset(packet.bytes.data(), 0, byte_count);
    else std::memcpy(packet.bytes.data(), bytes, byte_count);
    if (myw_port_state(p) != MYW_OK) return myw_port_state(p);
    p->next_frame += frames;
    p->written.store(w + 1, std::memory_order_release);
    return myw_port_state(p); // Consumer rechecks revoke after copying, too.
}
int myw_port_pop(myw_port* p, myw_binding binding, void* bytes, size_t capacity, myw_block* block) {
    if (bytes) std::memset(bytes, 0, capacity);
    if (block) *block = {};
    if (!p || !bytes || !block) return MYW_INVALID;
    if (myw_port_state(p) != MYW_OK) return myw_port_state(p);
    if (!binding_equal(binding, p->binding)) return fail(p, MYW_STALE);
    if (capacity < MYW_MAX_FRAMES * myw_frame_bytes(p->format)) return fail(p, MYW_INVALID);
    const auto r = p->read.load(std::memory_order_relaxed);
    if (r == p->written.load(std::memory_order_acquire)) return MYW_EMPTY;
    const auto& packet = p->packets[r % MYW_SLOTS];
    std::memcpy(bytes, packet.bytes.data(), packet.block.frames * myw_frame_bytes(p->format));
    *block = packet.block;
    p->read.store(r + 1, std::memory_order_release);
    if (myw_port_state(p) != MYW_OK) {
        std::memset(bytes, 0, capacity); *block = {};
        return myw_port_state(p);
    }
    return MYW_OK;
}
}
