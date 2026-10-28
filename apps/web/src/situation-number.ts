import type { ObjectDirective } from 'vue'
import { metadataIntegerLiteral } from './situations.ts'

type Draft = { text: string; message: string }
const drafts = new WeakMap<HTMLInputElement, Draft>()
function validity(input: HTMLInputElement) {
  input.setCustomValidity(input.validity.badInput || (input.value !== '' && !Number.isFinite(input.valueAsNumber))
    ? 'Enter a finite number.'
    : input.value !== '' && input.step === '1' && !metadataIntegerLiteral(input.value)
      ? 'Enter an exact nonnegative safe integer.'
      : '')
}
// Vue can reuse an input for a new operand or row. Retain an invalid draft only
// for the same owner and unchanged model value; otherwise recompute validity.
// Required/min/max/step remain native constraints. Never round integer drafts.
export const vSituationNumber: ObjectDirective<HTMLInputElement> = {
  mounted(input) {
    validity(input)
    input.addEventListener('input', () => {
      validity(input)
      if (input.validity.valid) drafts.delete(input)
      else drafts.set(input, { text: input.value, message: input.validationMessage })
    }, true)
  },
  updated(input, binding, node, previous) {
    const draft = drafts.get(input)
    if (draft && Object.is(node.props?.value, previous?.props?.value) && binding.value === binding.oldValue) {
      if (input.value !== draft.text) input.value = draft.text
      input.setCustomValidity(draft.message)
    }
    else {
      drafts.delete(input)
      validity(input)
    }
  },
}
