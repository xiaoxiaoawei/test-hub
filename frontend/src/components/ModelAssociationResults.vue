<script setup lang="ts">
import type { ModelAssociation } from '../types';
import { isModelAssociationVerified, isModelMappingVerified, modelAssociationLabel, modelAssociationSummary, modelBBoxLabel } from '../model-association';
import Icon from './Icon.vue';
defineProps<{items:ModelAssociation[]}>();
function fileName(value:string){return value.split(/[\\/]/).pop() || value;}
</script>
<template>
 <section v-if="items.length" class="model-association-results" aria-label="3D 模型关联结果">
  <h3><Icon name="cube" :size="17"/>3D 模型关联</h3><p class="association-summary">{{modelAssociationSummary(items)}}</p>
  <div class="association-rows"><article v-for="(item,index) in items" :key="`${item.target}-${item.partId}-${index}`" class="association-row" :class="{verified:isModelAssociationVerified(item),unsupported:item.status==='unsupported','awaiting-geometry':isModelMappingVerified(item) && !isModelAssociationVerified(item)}"><div class="association-row-heading"><strong :title="item.entry || item.partId">{{item.entry || item.partId}}</strong><span><Icon :name="isModelAssociationVerified(item)?'check':item.status==='unsupported' || isModelMappingVerified(item)?'warning':'info'" :size="13"/>{{modelAssociationLabel(item)}}</span></div><p v-if="item.message">{{item.message}}</p><dl v-if="modelBBoxLabel(item.expectedBBoxMm) || modelBBoxLabel(item.actualBBoxMm)" class="association-geometry"><template v-if="modelBBoxLabel(item.expectedBBoxMm)"><dt>预期范围 (mm)</dt><dd>{{modelBBoxLabel(item.expectedBBoxMm)}}</dd></template><template v-if="modelBBoxLabel(item.actualBBoxMm)"><dt>回读范围 (mm)</dt><dd>{{modelBBoxLabel(item.actualBBoxMm)}}</dd></template></dl><p v-if="item.modelName" class="association-model-name">模型：{{item.modelName}}</p><div v-if="item.modelFile" class="association-file mono" :title="item.modelFile"><Icon name="file" :size="13"/>{{fileName(item.modelFile)}}</div></article></div>
 </section>
</template>
