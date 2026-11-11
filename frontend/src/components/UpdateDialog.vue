<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue';
import type { UpdateDownloadResult, UpdateInfo } from '../types';
import { api } from '../api';
import Icon from './Icon.vue';
defineProps<{currentVersion:string}>();const emit=defineEmits<{close:[]}>();
const checking=ref(false);const downloading=ref(false);const installing=ref(false);const installLaunched=ref(false);const info=ref<UpdateInfo>();const result=ref<UpdateDownloadResult>();const error=ref('');let alive=true;
async function check(){if(checking.value || downloading.value || installing.value || installLaunched.value)return;checking.value=true;error.value='';try{const value=await api.checkUpdates();if(alive){info.value=value;if(result.value && value.latestVersion!==result.value.version)result.value=undefined;}}catch(e){if(alive)error.value=e instanceof Error?e.message:String(e);}finally{if(alive)checking.value=false;}}
async function download(){if(downloading.value || !info.value?.available)return;downloading.value=true;error.value='';try{const value=await api.downloadUpdate();if(alive)result.value=value;}catch(e){if(alive)error.value=e instanceof Error?e.message:String(e);}finally{if(alive)downloading.value=false;}}
async function install(){if(installing.value || !result.value?.verified)return;installing.value=true;error.value='';try{await api.installUpdate(result.value.path);if(alive)installLaunched.value=true;}catch(e){if(alive)error.value=e instanceof Error?e.message:String(e);}finally{if(alive)installing.value=false;}}
function close(){if(!downloading.value && !installing.value)emit('close');}
function date(value:string){const parsed=new Date(value);return Number.isNaN(parsed.valueOf())?value:parsed.toLocaleDateString('zh-CN');}
function size(value:number){return value>0?`${(value/1024/1024).toFixed(1)} MB`:'';}
function keyboard(event:KeyboardEvent){if(event.key==='Escape'){event.stopPropagation();close();}}
onMounted(()=>{void check();window.addEventListener('keydown',keyboard);});onBeforeUnmount(()=>{alive=false;window.removeEventListener('keydown',keyboard);});
</script>
<template>
 <div class="modal-overlay" @click.self="close"><section class="modal update-modal" role="dialog" aria-modal="true" aria-label="检查更新">
  <header><h2><Icon name="refresh" :size="20"/>检查更新</h2><button class="icon-button" :disabled="downloading || installing" aria-label="关闭更新窗口" @click="close"><Icon name="close"/></button></header>
  <div class="update-body">
   <div class="update-version"><Icon name="chip" :size="32"/><div><strong>封装工坊</strong><span>当前版本 {{info?.currentVersion || currentVersion}}</span></div></div>
   <div v-if="checking" class="update-status"><span class="spinner"></span><span>正在检查可用更新…</span></div>
   <div v-else-if="info" class="update-status" :class="{available:info.available}"><Icon :name="info.available?'import':'check'" :size="19"/><div><strong>{{info.available?`发现新版本 ${info.latestVersion}`:info.message || (info.latestVersion?'当前已是最新版本':'暂未发布可用更新')}}</strong><span v-if="info.publishedAt">发布于 {{date(info.publishedAt)}}<template v-if="info.size"> · {{size(info.size)}}</template></span></div></div>
   <div v-if="error" class="result-error update-error" role="alert"><Icon name="warning" :size="17"/><span>{{error}}</span></div>
   <section v-if="info?.releaseNotes" class="release-notes"><h3>更新内容</h3><p>{{info.releaseNotes}}</p></section>
   <div v-if="downloading" class="update-download-state"><span class="spinner"></span><p>正在下载并校验更新包，请保持窗口打开。</p></div>
   <div v-if="result" class="update-result"><strong><Icon :name="result.verified?'check':'warning'" :size="17"/>{{result.verified?'更新包已下载，校验通过':'已下载，尚未通过完整校验'}}</strong><p class="mono">{{result.path}}</p><p v-if="!installLaunched">下载完成不代表已安装。当前运行的版本仍为 {{currentVersion}}。</p><p v-else class="update-restart-state"><Icon name="refresh" :size="16"/>安装程序已启动，正在准备重启。请等待安装程序完成。</p></div>
   <div v-if="info?.sha256 && info.available" class="update-checksum"><span>SHA-256</span><code>{{info.sha256}}</code></div>
  </div>
  <footer><span>{{installLaunched?'正在准备重启…':installing?'正在启动安装程序…':downloading?'下载与校验进行中':'下载完成后，可选择安装并重启。'}}</span><div class="update-actions">
   <button class="secondary-button" :disabled="checking || downloading || installing || installLaunched" @click="check">重新检查</button>
   <button v-if="result?.verified" class="primary-button" :disabled="installing || installLaunched" @click="install"><span v-if="installing || installLaunched" class="spinner"></span><Icon v-else name="refresh" :size="16"/>{{installLaunched?'准备重启…':installing?'正在启动…':'安装并重启'}}</button>
   <button v-else-if="info?.available" class="primary-button" :disabled="checking || downloading" @click="download"><span v-if="downloading" class="spinner"></span><Icon v-else name="import" :size="16"/>{{downloading?'下载中…':'下载更新'}}</button>
   <button v-else class="primary-button" @click="close">关闭</button>
  </div></footer>
 </section></div>
</template>
