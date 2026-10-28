<script setup lang="ts">
import { computed } from 'vue'
import { actionSummary, branches, conditionSummary } from './situations.ts'
import type { Evaluation, Graph } from './situations.ts'
const props = defineProps<{
  graph: Graph
  selected: string
  evaluation: Evaluation | null
}>()
const emit = defineEmits<{
  select: [
    id: string
  ]
}>()
const layout = computed(() => {
  const levels = new Map(props.graph.nodes.map(n => [n.id, 0]))
  // Bounded passes also render invalid cyclic drafts without recursive expansion.
  for (let i = 0; i < Math.min(props.graph.nodes.length, 32); i++) {
    let changed = false
    for (const n of props.graph.nodes)
      if (n.kind === 'decision')
        for (const [key] of branches)
          if (levels.has(n[key])) {
            const level = Math.min(31, Math.max(levels.get(n[key])!, levels.get(n.id)! + 1))
            if (levels.get(n[key]) !== level) {
              levels.set(n[key], level)
              changed = true
            }
          }
    if (!changed)
      break
  }
  const columns = new Map<number, number>(), points = new Map<string, {
    x: number
    y: number
  }>()
  for (const n of props.graph.nodes) {
    const level = levels.get(n.id)!, column = columns.get(level) ?? 0
    points.set(n.id, { x: 24 + column * 280, y: 24 + level * 185 })
    columns.set(level, column + 1)
  }
  const nodes = props.graph.nodes.map(n => ({ node: n, ...points.get(n.id)! }))
  const edges = props.graph.nodes.flatMap(n => n.kind === 'decision' ? branches.flatMap(([key, truth], i) => {
    const source = points.get(n.id)!, target = points.get(n[key])
    if (!target)
      return []
    const x = source.x + 40 + i * 70, y = source.y + 86, tx = target.x + 110, ty = target.y
    return [{ id: `${n.id}:${truth}`, source: n.id, next: n[key], truth, x, y, path: `M ${x} ${y} C ${x} ${y + 65}, ${tx} ${ty - 45}, ${tx} ${ty - 3}`, evaluated: props.evaluation?.trace.some(t => t.kind === 'decision' && t.node === n.id && t.result === truth && t.next === n[key]) }]
  }) : [])
  return { nodes, edges, width: Math.max(500, ...nodes.map(n => n.x + 245)), height: Math.max(155, ...nodes.map(n => n.y + 145)) }
})
function visited(id: string) {
  return props.evaluation?.trace.some(t => t.kind !== 'manual-override' && t.node === id) || props.evaluation?.terminal === id
}
</script>
<template>
  <div class="situation-diagram" role="region" aria-label="Decision graph diagram; scroll to explore. Node and labeled branch controls follow below." tabindex="0">
    <svg :viewBox="`0 0 ${layout.width} ${layout.height}`" :style="{ width: `${layout.width}px`, height: `${layout.height}px` }" role="img" aria-labelledby="situation-map-title" aria-describedby="situation-map-description">
      <title id="situation-map-title">{{ graph.name }} decision graph</title><desc id="situation-map-description">Root {{ graph.root }}. Each decision has true, false and unknown directed edges. Heavy dashed edges show the evaluated path. Selected nodes have a double outline. Use the node list to edit and follow branches with the keyboard.</desc>
      <defs><marker id="situation-arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M 0 0 L 8 4 L 0 8 z" fill="currentColor" /></marker></defs>
      <g v-for="edge in layout.edges" :key="edge.id" :class="['situation-edge', { evaluated: edge.evaluated }]" ><title>{{ edge.source }} — {{ edge.truth }} → {{ edge.next }}{{ edge.evaluated ? ' (evaluated path)' : '' }}</title><path :d="edge.path" marker-end="url(#situation-arrow)" /><text :x="edge.x" :y="edge.y + 18" text-anchor="middle">{{ edge.truth }}</text></g>
      <g v-for="item in layout.nodes" :key="item.node.id" :transform="`translate(${item.x}, ${item.y})`" :class="['situation-map-node', { selected: selected === item.node.id, visited: visited(item.node.id) }]">
        <rect width="220" height="86" :rx="item.node.kind === 'terminal' ? 20 : 4" /><rect v-if="selected === item.node.id" x="5" y="5" width="210" height="76" rx="3" class="situation-selection" />
        <text x="12" y="25">{{ item.node.id.length > 25 ? item.node.id.slice(0, 24) + '…' : item.node.id }}</text><text x="12" y="47" class="situation-map-label">{{ item.node.kind === 'decision' ? 'Decision' : 'Terminal' }}{{ graph.root === item.node.id ? ' · Root' : '' }}</text><text x="12" y="68" class="situation-map-label">{{ visited(item.node.id) ? 'Evaluated path' : selected === item.node.id ? 'Selected for editing' : 'Not evaluated' }}</text>
      </g>
    </svg>
  </div>
  <ul class="situation-node-list" aria-label="Graph nodes and directed branches">
    <li v-for="node in graph.nodes" :key="node.id" :class="{ selected: node.id === selected }">
      <button type="button" :aria-pressed="node.id === selected" @click="emit('select', node.id)">Edit {{ node.id }}</button><strong v-if="graph.root === node.id">Root</strong><span v-if="visited(node.id)" class="situation-path-label">Evaluated path</span>
      <p>{{ node.kind === 'decision' ? conditionSummary(node.condition) : actionSummary(node.action) }}</p>
      <div v-if="node.kind === 'decision'" class="situation-branches"><button v-for="[key, truth] in branches" :key="key" type="button" :aria-label="`${node.id}: ${truth} to ${node[key]}`" @click="emit('select', node[key])">{{ truth }} → {{ node[key] }}<span v-if="evaluation?.trace.some(t => t.kind === 'decision' && t.node === node.id && t.result === truth)"> (taken)</span></button></div>
    </li>
  </ul>
</template>
