// Synthetic contract fixture only. Never imported by the production app.
import type { CodecBinding, CodecConfig, CodecEvidence } from '../src/network-codecs.ts'
export const codecBinding: CodecBinding = { targetId: 'A', deviceId: 'A', authorityId: 'A-boot', revision: 7, epoch: 1 }
export const rawIntent: CodecConfig = { version: 1, streamId: '18446744073709551615', generation: '9007199254740993', encoding: { mode: 'raw' }, allowLossy: false }
export function codecFixture(binding = codecBinding, request = rawIntent): CodecEvidence {
  const originalSpec: CodecEvidence['originalSpec'] = { stream: request.streamId, name: 'Fixture exact F64 source', codec: 'raw-f64-le', sample_rate: 48000, channels: ['FL', 'FR'], frames_per_packet: 480, provenance: { sources: [{ peer: 'fixture-origin', source: 'fixture-source', groups: ['private-call'] }], visited_peers: ['fixture-origin', 'fixture-sender'] } }
  const lookahead = request.encoding.mode === 'opus' ? 312 : 0
  return {
    binding: { ...binding }, evidenceKind: 'fixture', direction: 'receive', originalSpec,
    timeline: { version: 1, request, sourceSpec: originalSpec, nextSourceSampleFrame: '18446744073709551000', bitPreserving: request.encoding.mode !== 'opus', destinationConversion: request.encoding.mode === 'opus' ? 'normalized-f32-clamp-v1' : null, codecLookaheadFrames: lookahead },
    capability: {
      version: 1, request, available: true, agreementState: 'receive-admitted', availabilityScope: 'local worker construction and exact grant on this authenticated session; no remote consumption or physical delivery proof', unavailableReason: null,
      constraints: { maxSourceChannels: 128, zstdLevels: [1, 2, 3], zstdMaxWindowBytes: 1048576, zstdDictionaries: false, opusRates: [8000, 12000, 16000, 24000, 48000], opusMonoPreservesSourceChannel: true, opusStereoOrderedChannels: ['FL', 'FR'], opusFrameMicros: [2500, 5000, 10000, 20000, 40000, 60000], opusMinBitrate: 6000, opusMaxBitrate: 512000, opusMaxComplexity: 10, opusFloatInput: 'finite normalized [-1,1]; S32/F64 conversion explicitly loses precision', automaticResampling: false, automaticChannelMapping: false },
      zstdVersion: 10507, opusVersion: 'fixture-libopus', maxRawBytes: 1048576, maxEncodedBytes: 1048576, maxEnvelopeBytes: 1050000, maxFragmentsPerBlock: 1024, reservedWindowBytes: 2100000, maxAggregateReassemblyBytes: 4200000, maxWorkWallNanos: 10000000, lookaheadFrames: lookahead, contextBytes: 64000,
      measurements: { blocks: 2, rawBytes: 15360, envelopeBytes: 8000, fragmentBytes: 8300, wallNanos: 120000, cpuNanos: null, cpuUnavailableReason: 'Fixture does not measure Linux thread CPU', lookaheadFrames: lookahead, contextBytes: 64000 }, measurementUnavailableReason: null, networkDelayNanos: null, physicalDelayNanos: null, delayUnavailableReason: 'Codec timing does not measure network, device or physical delay',
    },
  }
}
