<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import type { PartSearchResult } from '../types';
import { reconcileSearchSelection, toggleSearchSelection } from '../search';
import Icon from './Icon.vue';
const props=defineProps<{query:string;result?:PartSearchResult;loading:boolean;error:string;busy:boolean;addedIds:string[];pendingIds:string[]}>();
const emit=defineEmits<{close:[];page:[number];retry:[];add:[string[]]}>();
const panel=ref<HTMLElement>();const selectedIds=ref<string[]>([]);
const items=computed(()=>props.result?.items || []);const pageIds=computed(()=>items.value.map(item=>item.lcscId));
const added=computed(()=>new Set(props.addedIds.map(id=>id.toUpperCase())));const pending=computed(()=>new Set(props.pendingIds.map(id=>id.toUpperCase())));
const selected=computed(()=>new Set(selectedIds.value));const selectable=computed(()=>pageIds.value.filter(id=>!added.value.has(id.toUpperCase())));
const allSelected=computed(()=>selectable.value.length>0 && selectable.value.every(id=>selected.value.has(id.toUpperCase())));
function toggle(id:string){if(!props.busy)selectedIds.value=toggleSearchSelection(selectedIds.value,id,pageIds.value,props.addedIds);}
function toggleAll(){selectedIds.value=allSelected.value?[]:reconcileSearchSelection(selectable.value,pageIds.value,props.addedIds);}
watch(()=>props.result,()=>{selectedIds.value=[];});
watch(()=>props.addedIds,()=>{selectedIds.value=reconcileSearchSelection(selectedIds.value,pageIds.value,props.addedIds);},{deep:true});
onMounted(()=>panel.value?.focus());
</script>
<template>
 <div class="modal-overlay search-results-overlay" @click.self="emit('close')"><section ref="panel" class="modal part-search-dialog" role="dialog" aria-modal="true" aria-label="料号搜索结果" tabindex="-1">
  <header><div><h2><Icon name="search" :size="19"/>料号搜索结果</h2><p class="search-query-label" :title="query">{{query}}</p></div><button class="icon-button" aria-label="关闭搜索结果" @click="emit('close')"><Icon name="close"/></button></header>
  <div class="search-results-summary"><span>{{loading?'正在查询公开器件目录…':result?`共 ${result.total.toLocaleString()} 项 · 第 ${result.page} 页`:'查询未完成'}}</span><span v-if="result?.source" class="search-source">{{result.source}}</span></div>
  <div v-if="loading" class="search-results-state" role="status"><span class="spinner"></span><strong>正在搜索“{{query}}”</strong><p>只按本次提交查询，不会自动逐字搜索。</p></div>
  <div v-else-if="error" class="search-results-state error" role="alert"><Icon name="warning" :size="29"/><strong>搜索暂未完成</strong><p>{{error}}</p><button class="secondary-button" @click="emit('retry')"><Icon name="refresh" :size="15"/>重新搜索</button></div>
  <template v-else-if="result">
   <div v-if="result.message" class="search-limit-message"><Icon name="info" :size="15"/><span>{{result.message}}</span></div>
   <div v-if="!items.length" class="search-results-state"><Icon name="search" :size="30"/><strong>没有找到匹配器件</strong><p>请关闭此窗口，输入更短的型号、厂家或明确的 C 编号后重试。</p></div>
   <div v-else class="search-results-table-wrap"><table class="search-results-table"><thead><tr><th class="search-check-column"><input type="checkbox" :checked="allSelected" :disabled="busy || !selectable.length" aria-label="选择本页可添加器件" @change="toggleAll"></th><th>型号 / 描述</th><th>C 编号</th><th>制造商</th><th>封装</th><th class="search-add-column"></th></tr></thead><tbody><tr v-for="item in items" :key="item.lcscId" :class="{selected:selected.has(item.lcscId.toUpperCase()),added:added.has(item.lcscId.toUpperCase())}"><td><input type="checkbox" :checked="selected.has(item.lcscId.toUpperCase())" :disabled="busy || added.has(item.lcscId.toUpperCase())" :aria-label="`选择 ${item.name} ${item.lcscId}`" @change="toggle(item.lcscId)"></td><td><strong :title="item.name">{{item.name || '未提供型号'}}</strong><p :title="item.description">{{item.description || '未提供描述'}}</p></td><td class="mono">{{item.lcscId}}</td><td :title="item.manufacturer">{{item.manufacturer || '未提供'}}</td><td :title="item.package">{{item.package || '未提供'}}</td><td><span v-if="added.has(item.lcscId.toUpperCase())" class="search-added"><Icon name="check" :size="14"/>已添加</span><button v-else class="secondary-button compact" :disabled="busy" @click="emit('add',[item.lcscId])"><span v-if="pending.has(item.lcscId.toUpperCase())" class="spinner tiny"></span><Icon v-else name="plus" :size="14"/>{{pending.has(item.lcscId.toUpperCase())?'获取中':'添加'}}</button></td></tr></tbody></table></div>
  </template>
  <footer><div class="search-selection-summary"><span>{{busy?'正在获取所选器件的符号与封装数据…':`已选择 ${selectedIds.length} 项`}}</span><div v-if="result" class="search-pagination"><button class="icon-button" :disabled="loading || busy || result.page<=1" aria-label="搜索结果上一页" @click="emit('page',result.page-1)"><Icon name="chevron" class="previous-page-icon" :size="16"/></button><span>第 {{result.page}} 页</span><button class="icon-button" :disabled="loading || busy || !result.hasMore" aria-label="搜索结果下一页" @click="emit('page',result.page+1)"><Icon name="chevron" :size="16"/></button></div></div><div class="search-result-actions"><button class="secondary-button" @click="emit('close')">返回</button><button class="primary-button" :disabled="busy || loading || !selectedIds.length" @click="emit('add',[...selectedIds])"><span v-if="busy" class="spinner"></span><Icon v-else name="plus" :size="16"/>添加所选（{{selectedIds.length}}）</button></div></footer>
 </section></div>
</template>
