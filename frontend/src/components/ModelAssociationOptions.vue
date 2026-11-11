<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue';
import type { NativeConfig, NativePathKind } from '../types';
import { api } from '../api';
const props=defineProps<{modelValue:NativeConfig;target:string;nativeEnabled:boolean;includeFootprint:boolean;disabled:boolean}>();
const emit=defineEmits<{'update:modelValue':[NativeConfig]}>();
const choosing=ref(''),error=ref('');let alive=true;
const nativeAvailable=computed(()=>props.target==='pads'?!props.nativeEnabled:props.nativeEnabled);
const mode=computed(()=>props.target==='pads' || nativeAvailable.value?props.modelValue.modelAssociationMode:'prepare');
function set<K extends keyof NativeConfig>(key:K,value:NativeConfig[K]){emit('update:modelValue',{...props.modelValue,[key]:value});}
function inputValue(event:Event){return (event.target as HTMLInputElement).value;}
async function browse(kind:NativePathKind){if(choosing.value)return;choosing.value=kind;error.value='';try{const path=await api.chooseNativePath(kind);if(alive && path)set(kind,path);}catch(value){if(alive)error.value=value instanceof Error?value.message:String(value);}finally{if(alive)choosing.value='';}}
onBeforeUnmount(()=>alive=false);
</script>
<template>
 <div class="model-association-options">
  <h4>3D 关联方式</h4>
  <label class="radio-label"><input type="radio" name="model-association-mode" :checked="mode==='prepare'" :disabled="disabled" @change="set('modelAssociationMode','prepare')"><span>生成 3D 关联包</span></label>
  <p class="field-hint">{{target==='pads'?'准备模型与关联配置；写入 Reuse 需要切换本机 Reuse 模式后重新导出。':'准备 STEP、坐标配置与关联脚本，供目标 EDA 执行。'}}关联回读与几何范围核验结果分别显示。</p>
  <label class="radio-label" :class="{disabled:!nativeAvailable}"><input type="radio" name="model-association-mode" :checked="mode==='native'" :disabled="disabled || !nativeAvailable" @change="set('modelAssociationMode','native')"><span>{{target==='pads'?'在本机写入 Reuse':'由本机 Allegro 关联'}}<small>待 EDA 验证</small></span></label>
  <p v-if="target==='cadence' && !nativeEnabled" class="field-hint">Cadence 本机关联需要先启用上方的本机 EDA 原生库生成；当前仍可生成关联包。</p>
  <p v-if="target==='pads' && nativeEnabled" class="native-warning">Classic 9.4 的 2D 原生生成与现代 Reuse 写入暂不同时执行。写入 Reuse 时，请关闭上方 2D 原生模式，使用 ASCII 输出。</p>
  <p v-if="!includeFootprint" class="native-warning">当前未选择 PCB 封装；有源模型时仍可交付资料，封装关联须以各项结果为准。</p>
  <template v-if="mode==='native' && target==='pads'">
   <label class="native-field">PADS 3D 模型引擎<span class="native-path-row"><input :value="modelValue.padsModelExe" :disabled="disabled" placeholder="现代 PADS Layout 的 powerpcb.exe" @input="set('padsModelExe',inputValue($event))"><button class="secondary-button compact" :disabled="disabled || !!choosing" @click="browse('padsModelExe')">浏览</button></span></label>
   <p class="field-hint">3D 模型引擎与 Classic 9.4 的 2D 原生库引擎分别选择。</p>
   <label class="native-field">Reuse 根目录<span class="native-path-row"><input :value="modelValue.padsReuseDirectory" :disabled="disabled" placeholder="包含 3DModels 的 Reuse 根目录" @input="set('padsReuseDirectory',inputValue($event))"><button class="secondary-button compact" :disabled="disabled || !!choosing" @click="browse('padsReuseDirectory')">选择</button></span></label>
   <p class="field-hint">请选择目标 PADS 实际使用的 Reuse 根目录，先退出使用该目录的 PADS。本工具不修改用户全局 Reuse 配置。无法确认的第三方 PartDB 不自动合并。</p>
   <p class="field-hint">每批最多选择 50 个器件写入 Reuse，受管库最多保留 500 项映射。写入与关联结果逐项报告，不能仅凭 STEP 或文件夹出现判断成功。</p>
  </template>
  <p v-if="error" class="native-error" role="alert">{{error}}</p>
 </div>
</template>
