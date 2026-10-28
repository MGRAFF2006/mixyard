import { shallowRef, watch } from 'vue'

// The model is cloned on edits, so object identity cannot identify an editor row.
// Keep private keys alongside rows; only the removed row loses its DOM/draft.
export function useSituationRows(length: () => number) {
  const keys = shallowRef<symbol[]>([])
  watch(length, count => {
    if (count < keys.value.length) keys.value = keys.value.slice(0, count)
    else if (count > keys.value.length)
      keys.value = [...keys.value, ...Array.from({ length: count - keys.value.length }, () => Symbol('situation-row'))]
  }, { immediate: true, flush: 'sync' })
  function remove(index: number) {
    keys.value = keys.value.filter((_, i) => i !== index)
  }
  return { keys, remove }
}
