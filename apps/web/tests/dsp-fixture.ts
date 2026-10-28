import type { Preview, PreviewRequest } from '../src/dsp.ts'
// Boundary fixture only: these numbers are not DSP output or performance evidence.
export const admission: Preview['admission'] = { memoryBytes: 1234, workUnitsPerBlock: 1000, algorithmicLatencyFrames: 0, explicitDelayFrames: 960, tail: { kind: 'finite', frames: 960 } }
export function previewFixture(request: PreviewRequest, options: { slow?: boolean; measured?: boolean } = {}): Preview {
  const measured = options.measured ?? true
  return {
    mode: 'offline-dsp-preview', precision: 'float64', frames: request.frames, sampleRateHz: request.config.format.sampleRateHz,
    channels: [...request.config.format.channels], output: Array(request.frames * request.config.format.channels.length).fill(2),
    report: { frames: request.frames, overFullScaleSamples: request.frames * request.config.format.channels.length, bypassed: request.bypassed },
    meters: request.config.format.channels.map(() => ({ peak: 2, rms: 2, overFullScaleSamples: request.frames })), admission,
    timing: { measured, iterations: request.iterations, meanMs: measured ? (options.slow ? 10 : 0.01) : null, maxMs: measured ? (options.slow ? 20 : 0.02) : null,
      blockBudgetMs: request.frames / request.config.format.sampleRateHz * 1000, deadlineMisses: options.slow ? request.iterations : 0, overBudget: !!options.slow,
      build: 'mock-release', os: 'mock-linux', arch: 'mock-x86_64' },
  }
}
