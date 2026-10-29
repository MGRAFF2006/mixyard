/* SPDX-License-Identifier: MIT */
#ifndef MIXYARD_HOST_H
#define MIXYARD_HOST_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct mx_host mx_host;
#define MX_HOST_ABI_VERSION 1u
uint32_t mx_host_abi_version(void);
typedef struct mx_stamp {
    uint64_t generation, revision, clock_id, sequence, first_frame, captured_ns, deadline_ns;
} mx_stamp;
enum mx_encoding { MX_F32LE = 1, MX_F64LE = 2, MX_S16LE = 3, MX_S24LE = 4, MX_S32LE = 5 };
enum mx_result {
    MX_OK = 0, MX_INVALID = 1, MX_POLICY = 2, MX_STALE = 3, MX_FORMAT = 4,
    MX_SAMPLES = 5, MX_EXPIRED = 6, MX_INACTIVE = 7, MX_BUFFER = 8, MX_PROCESS = 9,
    MX_UNAVAILABLE = 10
};
typedef struct mx_input {
    const uint8_t* bytes;
    size_t byte_count;
    uint32_t encoding, sample_rate_hz, channels;
    mx_stamp stamp;
} mx_input;
/* Control thread. UTF-8 strict request JSON, <=1 MiB. Error is NUL-terminated
 * when capacity is nonzero. No audio endpoints are activated by this library.
 * Caller owns authorization/native identity/format/clock proof; binding numbers
 * are freshness metadata, not authority. No status flags are promoted. */
int mx_host_create(const uint8_t* request_json, size_t length, mx_host** out,
                   char* error, size_t error_capacity);
/* Destroy only after all users have quiesced; never on an audio callback. */
void mx_host_destroy(mx_host* host);
size_t mx_host_source_count(const mx_host* host);
size_t mx_host_output_count(const mx_host* host);
/* Immutable source/output specs, serialized in the field order of the fixture:
 * source: observedId,encoding,sampleRateHz,channels,mapping;
 * output: observedId,sampleRateHz,channels,mapping.
 * needed is JSON byte length, with NO NUL terminator. A null/short buffer returns
 * MX_BUFFER and needed; a sufficient buffer receives exactly needed bytes.
 * Control thread only, do not parse or compare identities on the callback. */
int mx_host_source_info(const mx_host*, size_t index, uint8_t* json, size_t capacity, size_t* needed);
int mx_host_output_info(const mx_host*, size_t index, uint8_t* json, size_t capacity, size_t* needed);
/* Additive v1 metadata query, ABI1 source/output_info layouts remain unchanged.
 * Control owner only; same two-pass exact JSON/no-NUL semantics as info.
 * scope=compiled-plan-metadata conveys complete PLAN knowledge only, never
 * authenticated identity, native binding/provenance/clock, consent or grants.
 * The conservative per-output union includes muted/zero-gain/tail contributors.
 * Missing/partial/oversize metadata returns MX_UNAVAILABLE, never a partial list.
 * Invalid index returns MX_INVALID; revoked context returns MX_INACTIVE.
 * See output-attestation.md for the closed schema and independent lease gates. */
int mx_host_output_attestation_v1(const mx_host*, size_t index, uint8_t* json,
                                 size_t capacity, size_t* needed);
typedef struct mx_output_packet_v1 {
    const double* samples;
    size_t sample_count;
    mx_stamp stamp;
    uint64_t output_index;
    uint8_t attestation_sha256[32];
} mx_output_packet_v1;
/* Borrowed packet from exactly this host/output. Stamp is the exact admitted
 * engine stamp, not a new hardware-clock claim. Hash is SHA256 of queried JSON.
 * Output storage expires on next process/revoke/destroy. Same exclusive owner
 * requirements as output(); every error clears all packet fields. Missing
 * attestation is MX_UNAVAILABLE. Check every external lease again before commit.
 * ProcessId+instanceId is process-local freshness metadata, never authentication. */
int mx_host_output_packet_v1(const mx_host*, size_t index, mx_output_packet_v1*);
/* One exclusive graph owner, no concurrent calls/readers/revoke/destruction.
 * Exact interleaved little-endian native bytes. Missing source=NULL/0 is silence;
 * resuming after source loss requires fresh host admission (new generation).
 * All stamps must already use one explicitly qualified common timeline. This
 * API never guesses hardware offsets, resamples or promotes an Instant/QPC value
 * into capture time. No allocation/locks/I/O/deallocation on expected paths.
 * Invalid input or stale authority terminally revokes all retained host PCM.
 * A failed native consumer is isolated by its adapter's independent SPSC.
 * All non-null pointers must address valid caller-owned memory of stated size. */
int mx_host_process(mx_host*, uint64_t now_ns, mx_stamp engine_stamp, size_t frames,
                    const mx_input* inputs, size_t input_count);
/* Output is borrowed Float64 in the admitted canonical engine order; valid only
 * until next process/revoke/destroy. Never retain it across owners or callbacks.
 * Sink conversion must be explicit and local to that destination. */
int mx_host_output(const mx_host*, size_t index, const double** samples, size_t* sample_count);
void mx_host_revoke(mx_host*);
#ifdef __cplusplus
}
#endif
#endif
