// Fixture-only telemetry. No App boot, DOM/UI control, native call or permission.
export const resourceProbes = Object.freeze([
  { role: 'classic-probe', file: 'owned-classic-probe.js', type: '', source: 'window.__mixyardOwnedClassicProbeExecuted = true;\n' },
  { role: 'module-probe', file: 'owned-module-probe.js', type: 'module', source: 'window.__mixyardOwnedModuleProbeExecuted = true;\n' },
])
export function attachResourceProbes(html) {
  const matches = [...html.matchAll(/<script\b[^>]*\bdata-controller-entry=["']classic["'][^>]*>/g)]
  if (matches.length !== 1) throw Error('Expected exactly one actual classic App entry before adding owned resource telemetry.')
  const original = matches[0][0]
  if (/data-owned-role=/.test(original) && !/data-owned-role=["']app["']/.test(original)) throw Error('App entry role mismatch.')
  const app = /data-owned-role=/.test(original) ? original : original.replace('<script', '<script data-owned-role="app"')
  const probes = resourceProbes.map(value => `<script data-owned-role="${value.role}"${value.type ? ' type="module"' : ''} src="./${value.file}"></script>`).join('\n')
  return html.replace(original, probes + '\n' + app)
}
