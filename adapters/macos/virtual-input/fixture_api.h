// SPDX-License-Identifier: MIT
#pragma once
#include <CoreAudio/AudioServerPlugIn.h>
#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif
// The only release export. Returns AudioServerPlugInDriverInterface **.
__attribute__((visibility("default"))) void *MixyardInput_Create(CFAllocatorRef, CFUUIDRef);

#ifdef MIXYARD_INPUT_TESTING
// Fixture-only, in-process API: no IPC, peer authentication, HAL loading or production apply.
// All these calls are non-RT. Strings must be NUL-terminated UTF-8; UUID is canonical lowercase.
enum { MY_INPUT_ENDPOINT_LIMIT = 2, MY_INPUT_CLIENT_LIMIT = 16,
       MY_INPUT_RING_FRAMES = 16384, MY_INPUT_PACKET_FRAMES = 4096 };
enum { MY_INPUT_OK = 0, MY_INPUT_INVALID = 1, MY_INPUT_STALE = 2,
       MY_INPUT_CAPACITY = 3, MY_INPUT_BUSY = 4, MY_INPUT_NOT_FOUND = 5,
       MY_INPUT_HOST_ERROR = 6 };
enum { MY_INPUT_PCM = 0, MY_INPUT_NO_PRODUCER = 1, MY_INPUT_UNDERRUN = 2,
       MY_INPUT_RETIRED = 3, MY_INPUT_STOPPED = 4, MY_INPUT_BAD_TIME = 5,
       MY_INPUT_TRANSITION = 6 };
#define MY_INPUT_INITIAL_UUID "c963dc90-9a0d-4c6c-8934-d8d5af579260"
#define MY_INPUT_UID_PREFIX "org.mixyard.input."
// One initialized A; publish B once. Two lifetime endpoint and 16 lifetime client slots
// per endpoint (retired IDs/client slots are never recycled; capacity is explicit).
int MixyardInputTest_PublishEndpoint(const char *uuid, const char *name, AudioObjectID *device);
int MixyardInputTest_Rename(AudioObjectID device, const char *name);
// Pending retirement gates PCM immediately; host must perform or abort the returned token.
// Request failure/abort leaves device published and silent; a fresh admission is required.
int MixyardInputTest_Retire(AudioObjectID device, uint64_t *action);
// Single fixture producer per endpoint. Nonzero, increasing generation prevents stale replay.
int MixyardInputTest_Admit(AudioObjectID device, uint64_t generation);
int MixyardInputTest_Disconnect(AudioObjectID device, uint64_t generation);
// Exact native Float32 stereo interleaved, 48k. Byte length must be frames*8.
// Absolute HAL input sample positions: contiguous packets per admission, monotonic revisions.
// First packet may start anywhere in current clock epoch; no destructive draining by readers.
int MixyardInputTest_Packet(AudioObjectID device, uint64_t generation, uint64_t clockSeed,
                           uint64_t revision, uint64_t firstFrame, uint32_t frames,
                           const float *pcm, size_t bytes);
// Snapshot is the most recent DoIO observation for this client, not consumer/OS evidence.
// effectiveRevision=0 unless entire read had PCM of one revision; zeros count explicitly.
typedef struct {
    uint64_t clockSeed, generation, firstFrame, nextFrame, effectiveRevision;
    uint64_t deliveredFrames, zeroFrames;
    uint32_t reason;
} MixyardInputTestEvidence;
int MixyardInputTest_Evidence(AudioObjectID device, uint32_t client, MixyardInputTestEvidence *out);
#endif
#ifdef __cplusplus
}
#endif
