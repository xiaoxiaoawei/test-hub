<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import type { NativeConfig, NativeEnvironment, NativeInstallation, NativePathKind } from '../types';
import { api } from '../api';
import Icon from './Icon.vue';
const props=defineProps<{modelValue:NativeConfig;target:string;includeSymbol:boolean;includeFootprint:boolean;disabled:boolean}>();
const emit=defineEmits<{'update:modelValue':[NativeConfig]}>();
const environment=ref<NativeEnvironment>();const loading=ref(false);const error=ref('');const choosing=ref('');let generation=0;let alive=true;
const candidates=computed(()=>props.target==='pads'?environment.value?.pads || []:environment.value?.cadence || []);
const captures=computed(()=>candidates.value.filter(item=>item.product.toLowerCase().includes('capture')));
const allegros=computed(()=>candidates.value.filter(item=>item.product.toLowerCase().includes('allegro')));
const configured=computed(()=>props.target==='pads'?!!props.modelValue.padsRoot:(!props.includeSymbol || !!props.modelValue.captureExe) && (!props.includeFootprint || !!props.modelValue.allegroExe));
function set<K extends keyof NativeConfig>(key:K,value:NativeConfig[K]){emit('update:modelValue',{...props.modelValue,[key]:value});}
function inputValue(event:Event){return (event.target as HTMLInputElement).value;}
function pickCandidate(kind:'padsRoot'|'captureExe'|'allegroExe',event:Event){const id=inputValue(event);const item=candidates.value.find(value=>value.id===id);if(item)set(kind,kind==='padsRoot'?item.root:item.executable);}
function candidateLabel(item:NativeInstallation){return `${item.product} ${item.version || '版本未知'} · ${item.executable || item.root}${item.ready?'':' · 不可用'}`;}
async function refresh(){const current=++generation;loading.value=true;error.value='';try{const result=await api.nativeEnvironment();if(current===generation)environment.value=result;}catch(value){if(current===generation)error.value=value instanceof Error?value.message:String(value);}finally{if(current===generation)loading.value=false;}}
async function browse(kind:NativePathKind){if(choosing.value)return;choosing.value=kind;error.value='';try{const value=await api.chooseNativePath(kind);if(alive && value)set(kind,value);}catch(value){if(alive)error.value=value instanceof Error?value.message:String(value);}finally{if(alive)choosing.value='';}}
onMounted(()=>void refresh());onBeforeUnmount(()=>{alive=false;generation++;});
</script>
<template>
 <div class="native-export-options">
  <label class="checkbox-label native-enable"><input type="checkbox" :checked="modelValue.enabled" :disabled="disabled" @change="set('enabled',($event.target as HTMLInputElement).checked)"><span>由本机 EDA 生成原生库<br><small>待实际 EDA 验证</small></span></label>
  <p v-if="target==='pads'" class="field-hint">原生生成仅支持 PADS Classic 9.4 / 32 位 SDK（待实机验证）。</p>
  <p class="field-hint" v-if="!modelValue.enabled">当前输出{{target==='pads'?' PADS ASCII 导入包':' Cadence 脚本包'}}，可保留后手工执行。</p>
  <template v-if="modelValue.enabled">
   <div class="native-environment-heading"><strong>本机引擎</strong><button class="text-button" :disabled="loading || disabled" @click="refresh"><Icon name="refresh" :size="13"/>{{loading?'检测中':'重新检测'}}</button></div>
   <p class="native-availability" :class="{unavailable:!configured}">{{loading?'正在查找本机安装…':configured?'已配置引擎路径，尚未执行目标引擎。':candidates.some(item=>item.ready)?'请选择本次使用的安装，或手工指定路径。':'未发现可用安装；原生生成暂不可用。可指定路径，或关闭此项保留导入包。'}}</p>
   <p class="field-hint">安装发现只检查文件与版本线索，不代表许可可用或原生转换已通过。</p>
   <template v-if="target==='pads'">
    <label class="native-field">PADS 安装<select :disabled="disabled" value="" @change="pickCandidate('padsRoot',$event)"><option value="" disabled>选择已发现的安装</option><option v-for="item in candidates" :key="item.id" :value="item.id" :disabled="!item.ready">{{candidateLabel(item)}}</option></select></label>
    <label class="native-field">PADS 根目录<span class="native-path-row"><input :value="modelValue.padsRoot" :disabled="disabled" placeholder="包含 Logic / Layout 的安装目录" @input="set('padsRoot',inputValue($event))"><button class="secondary-button compact" :disabled="disabled || !!choosing" @click="browse('padsRoot')">浏览</button></span></label>
   </template>
   <template v-else>
    <label v-if="includeSymbol" class="native-field">Capture 安装<select :disabled="disabled" value="" @change="pickCandidate('captureExe',$event)"><option value="" disabled>选择已发现的 Capture</option><option v-for="item in captures" :key="item.id" :value="item.id" :disabled="!item.ready">{{candidateLabel(item)}}</option></select><span class="native-path-row"><input :value="modelValue.captureExe" :disabled="disabled" placeholder="capture.exe" @input="set('captureExe',inputValue($event))"><button class="secondary-button compact" :disabled="disabled || !!choosing" @click="browse('captureExe')">浏览</button></span></label>
    <template v-if="includeFootprint"><label class="native-field">Allegro 安装<select :disabled="disabled" value="" @change="pickCandidate('allegroExe',$event)"><option value="" disabled>选择已发现的 Allegro</option><option v-for="item in allegros" :key="item.id" :value="item.id" :disabled="!item.ready">{{candidateLabel(item)}}</option></select><span class="native-path-row"><input :value="modelValue.allegroExe" :disabled="disabled" placeholder="allegro.exe" @input="set('allegroExe',inputValue($event))"><button class="secondary-button compact" :disabled="disabled || !!choosing" @click="browse('allegroExe')">浏览</button></span></label><label class="native-field">Allegro 产品标识<input :value="modelValue.allegroProduct" :disabled="disabled" placeholder="按本机已配置的产品填写" @input="set('allegroProduct',inputValue($event))"></label></template>
    <label v-if="includeSymbol" class="native-field">Capture 单位 / inch<input type="number" min="1" step="1" :value="modelValue.captureUnitsPerInch" :disabled="disabled" @change="set('captureUnitsPerInch',Number(inputValue($event)))"><small>默认 100；需在目标版本核对网格与引脚长度。</small></label>
   </template>
   <div class="native-mode segmented"><button :class="{active:modelValue.libraryMode==='new'}" :disabled="disabled" @click="set('libraryMode','new')">新建独立库</button><button :class="{active:modelValue.libraryMode==='append'}" :disabled="disabled" @click="set('libraryMode','append')">追加已有库</button></div>
   <p v-if="modelValue.libraryMode==='new'" class="field-hint">按当前输出目录与库名建立本次库。原生结果须经引擎回读后才会发布。</p>
   <template v-else>
    <template v-if="target==='pads'"><label v-if="includeSymbol" class="native-field">现有 PADS Logic 库<span class="native-path-row"><input :value="modelValue.padsSymbolLibrary" :disabled="disabled" placeholder="现有库的 .pt9 文件" @input="set('padsSymbolLibrary',inputValue($event))"><button class="secondary-button compact" :disabled="disabled || !!choosing" @click="browse('padsSymbolLibrary')">选择</button></span></label><label v-if="includeFootprint" class="native-field">现有 PADS Layout 库<span class="native-path-row"><input :value="modelValue.padsFootprintLibrary" :disabled="disabled" placeholder="现有库的 .pt9 文件" @input="set('padsFootprintLibrary',inputValue($event))"><button class="secondary-button compact" :disabled="disabled || !!choosing" @click="browse('padsFootprintLibrary')">选择</button></span></label></template>
    <template v-else><label v-if="includeSymbol" class="native-field">现有 Capture 库<span class="native-path-row"><input :value="modelValue.captureLibrary" :disabled="disabled" placeholder="现有 .olb 文件" @input="set('captureLibrary',inputValue($event))"><button class="secondary-button compact" :disabled="disabled || !!choosing" @click="browse('captureLibrary')">选择</button></span></label><label v-if="includeFootprint" class="native-field">现有 Allegro 库目录<span class="native-path-row"><input :value="modelValue.allegroDirectory" :disabled="disabled" placeholder="包含封装与焊盘的现有库目录" @input="set('allegroDirectory',inputValue($event))"><button class="secondary-button compact" :disabled="disabled || !!choosing" @click="browse('allegroDirectory')">选择</button></span></label></template>
    <p class="field-hint">关闭占用目标库的 EDA 后追加，同名条目冲突会停止。</p>
   </template>
   <p class="field-hint">3D 如有提供仅作为附件交付，不代表已完成 Reuse 或原生模型关联。</p>
   <p v-for="warning in environment?.warnings || []" :key="warning" class="native-warning">{{warning}}</p>
  </template>
  <p v-if="error" class="native-error" role="alert">{{error}}</p>
 </div>
</template>
