// SPDX-License-Identifier: MIT
#ifndef MIXYARD_WINDOWS_WORKER_H
#define MIXYARD_WINDOWS_WORKER_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* Exact interleaved little-endian bytes, separate from the engine's f32/f64 queue.
 * Channel order is ascending set-bit order of the WAVE speaker mask. */
typedef struct myw_format {
    uint32_t rate;
    uint32_t channel_mask;
    uint16_t channels;
    uint16_t container_bits;
    uint16_t valid_bits;
    uint16_t floating;
} myw_format;
typedef struct myw_binding { uint64_t generation, revision; } myw_binding;
/* QPC is the WASAPI packet's ORIGINAL first-frame time, in 100ns units.
 * packet_offset locates this chunk within that packet. No engine-clock mapping
 * or latency estimate is implied. clock_id is the caller's source-clock label. */
typedef struct myw_stamp {
    myw_binding binding;
    uint64_t clock_id, sequence, first_frame, device_frame, qpc_100ns;
    uint32_t packet_offset, flags;
} myw_stamp;
typedef struct myw_block { myw_stamp stamp; uint32_t frames; } myw_block;
enum myw_result {
    MYW_OK, MYW_EMPTY, MYW_INVALID, MYW_STALE, MYW_FULL, MYW_ORDER,
    MYW_FORMAT, MYW_NATIVE, MYW_UNSUPPORTED, MYW_CAPTURE_FAULT, MYW_EXHAUSTED, MYW_UNDERFLOW
};
enum myw_direction { MYW_CAPTURE = 1, MYW_RENDER = 2 };
enum { MYW_MAX_FRAMES = 256, MYW_SLOTS = 16, MYW_MAX_FRAME_BYTES = 256 };
typedef struct myw_port myw_port;

/* Control-thread allocation/destruction. One immutable port per source/output.
 * generation is host-session scoped identity, NOT an authorization token. */
int myw_port_create(myw_format format, myw_binding binding, uint64_t clock_id, myw_port** out);
void myw_port_destroy(myw_port* port); /* Only after ALL users/workers are joined. */
void myw_port_revoke(myw_port* port);  /* Irreversible, safe from control thread. */
/* Control-only, after ALL producer/consumer workers are stopped and joined.
 * Requires an irreversibly closed port; an open port returns MYW_INVALID.
 * Zeroes retained PCM/packet metadata WITHOUT rearming/resetting the SPSC.
 * Closure alone is NOT quiescence. This API cannot prove caller thread joins;
 * the native host's owned Lifecycle enforces them before invoking it. */
int myw_port_scrub_quiescent(myw_port* port);
int myw_port_state(const myw_port* port);
int myw_port_info(const myw_port* port, myw_format* format, myw_binding* binding, uint64_t* clock_id);
/* One producer and one consumer, lock-free SPSC. No concurrent producers/consumers.
 * A malformed/stale/full/out-of-order push terminally closes only this port.
 * Pop zeroes the entire supplied byte capacity on any failure/empty result.
 * Capacity must fit a whole MYW_MAX_FRAMES packet. Neither operation allocates. */
int myw_port_push(myw_port* port, myw_format format, myw_stamp stamp,
                  const void* bytes, uint32_t frames, size_t byte_count);
int myw_port_pop(myw_port* port, myw_binding binding, void* bytes,
                 size_t capacity, myw_block* block);
size_t myw_frame_bytes(myw_format format);
int myw_format_equal(myw_format a, myw_format b);

/* Setup-only endpoint query; never selects a default, starts audio or reconnects.
 * Native functions are callable production boundaries, not inventory proof.
 * UTF-16 exact MMDevice IDs must be explicit, nonempty and <=4096 code units. */
int myw_endpoint_format(const uint16_t* endpoint, size_t units, int direction,
                         myw_format* format, int32_t* native_error);
typedef struct myw_report {
    myw_format negotiated;
    uint64_t frames, silent_frames, underflow_frames;
    /* Capture position is frames; render position uses clock_frequency units. */
    uint64_t last_device_position, last_qpc_100ns, clock_frequency;
    uint32_t last_flags, mmcss;
    int32_t native_error;
    int result;
} myw_report;
/* Blocking worker on a caller-owned, dedicated thread. Owns COM, event, audio
 * interfaces and their release on that SAME thread. duration_ms=1..600000.
 * Explicit invocation is the host's consent boundary. Revoke stops within the
 * 50ms event wait plus bounded work/API time; OS/driver calls are not timeouts.
 * Exact GetMixFormat match only; no AUTOCONVERTPCM/resampling/channel changes.
 * Capture produces to port; render consumes from port. No simultaneous other
 * producer for capture, or other consumer for render. Completion revokes port.
 * Report is written only by the worker; read it after joining.
 * Already submitted native frames cannot be retracted by a concurrent revoke. */
int myw_worker_run(myw_port* port, const uint16_t* endpoint, size_t units,
                   int direction, uint32_t duration_ms, myw_report* report);
#ifdef __cplusplus
}
#endif
#endif
