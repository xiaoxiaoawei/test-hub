<script setup lang="ts">
import type { DividerKind } from '../layout';
defineProps<{kind:DividerKind;axis:'x'|'y';label:string;value?:number;min?:number;max?:number}>();
const emit=defineEmits<{start:[event:PointerEvent];key:[event:KeyboardEvent];reset:[]}>();
</script>

<template>
  <div class="pane-divider" :class="`pane-divider-${axis}`" :data-divider="kind"
    role="separator" :aria-orientation="axis==='x'?'vertical':'horizontal'" :aria-label="label"
    :aria-valuenow="value===undefined?undefined:Math.round(value)"
    :aria-valuemin="value===undefined?undefined:(min ?? 0)" :aria-valuemax="value===undefined?undefined:(max ?? (kind==='canvas'?100:10000))"
    :aria-valuetext="value===undefined?undefined:`${Math.round(value)}${kind==='canvas'?'%':' 像素'}`" tabindex="0"
    :title="`${label}：拖动或使用方向键调整，双击或 Home 复位`"
    @pointerdown="emit('start',$event)" @keydown="emit('key',$event)" @dblclick.prevent="emit('reset')">
    <span aria-hidden="true"></span>
  </div>
</template>

<style scoped>
.pane-divider{position:relative;display:flex;align-items:center;justify-content:center;touch-action:none;user-select:none;outline:none;z-index:3;min-width:0;min-height:0;border-radius:3px}
.pane-divider-x{width:6px;height:100%;cursor:col-resize}.pane-divider-y{height:6px;width:100%;cursor:row-resize}
.pane-divider>span{display:block;background:var(--border-strong);border-radius:2px;opacity:.55;transition:background .12s,opacity .12s}
.pane-divider-x>span{width:2px;height:32px}.pane-divider-y>span{height:2px;width:32px}
.pane-divider:hover,.pane-divider:focus-visible{background:var(--primary-soft)}.pane-divider:hover>span,.pane-divider:focus-visible>span{background:var(--primary);opacity:1}.pane-divider:focus-visible{box-shadow:inset 0 0 0 1px var(--primary)}
</style>
