<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { parseMaterialOBJ } from '../model-materials';
import type { Part } from '../types';
import { api } from '../api';
import Icon from './Icon.vue';
const props=defineProps<{part:Part;dark:boolean}>();
const host=ref<HTMLElement>();const fileInput=ref<HTMLInputElement>();const error=ref('');const modelName=ref('尚未加载模型');const loaded=ref(false);const loading=ref(false);const caption=ref('');
const viewPreset=ref<'top'|'oblique'>('oblique');
let renderer:THREE.WebGLRenderer|undefined;let scene:THREE.Scene;let camera:THREE.PerspectiveCamera;let controls:OrbitControls;let group:THREE.Group|undefined;let observer:ResizeObserver;let frame=0;let alive=true;
let viewportWidth=1;let viewportHeight=1;
function disposeObject(object:THREE.Object3D){object.traverse(o=>{if(o instanceof THREE.Mesh){o.geometry.dispose();const m=Array.isArray(o.material)?o.material:[o.material];m.forEach(v=>v.dispose());}});}
function requestRender(){if(!alive || frame || !renderer || document.hidden)return;frame=requestAnimationFrame(()=>{frame=0;if(!alive || !renderer)return;const changed=controls.update();renderer.render(scene,camera);if(changed && controls.enableDamping)requestRender();});}
function fit(resetOrientation=true){
 if(!group || !renderer || !camera || !controls)return;
 const box=new THREE.Box3().setFromObject(group);if(box.isEmpty())return;
 const sphere=box.getBoundingSphere(new THREE.Sphere());const center=sphere.center;const radius=Math.max(sphere.radius,.001);if(!Number.isFinite(radius))return;
 const verticalHalfFOV=THREE.MathUtils.degToRad(camera.getEffectiveFOV())/2;
 const horizontalHalfFOV=Math.atan(Math.tan(verticalHalfFOV)*Math.max(.001,camera.aspect));
 // Leave room for the toolbar and caption. The horizontal FOV is the limiting
 // factor in a narrow split pane, even when its vertical space is generous.
 const horizontalSpace=Math.max(.15,(viewportWidth-36)/Math.max(1,viewportWidth));
 const verticalSpace=Math.max(.15,(viewportHeight-110)/Math.max(1,viewportHeight));
 const usableHalfFOV=Math.min(Math.atan(Math.tan(verticalHalfFOV)*verticalSpace),Math.atan(Math.tan(horizontalHalfFOV)*horizontalSpace));
 const distance=radius/Math.sin(Math.max(.0001,usableHalfFOV))*1.08;
 const presetDirection=viewPreset.value==='top'?new THREE.Vector3(0,-.001,1):new THREE.Vector3(.3,-.6,1.8);
 const direction=resetOrientation?presetDirection:camera.position.clone().sub(controls.target);
 if(direction.lengthSq()<.000001)direction.copy(presetDirection);direction.normalize();
 camera.position.copy(center).addScaledVector(direction,distance);
 camera.near=Math.max(radius/1000,.000001);camera.far=Math.max(distance+radius*30,camera.near*100);
 camera.updateProjectionMatrix();controls.target.copy(center);controls.minDistance=Math.max(radius*.12,.0001);controls.maxDistance=Math.max(distance*20,radius*100);controls.update();requestRender();
}
defineExpose({fit});
function setView(value:'top'|'oblique'){viewPreset.value=value;fit();}
function replaceGroup(next:THREE.Group){if(group){scene.remove(group);disposeObject(group);}group=next;scene.add(group);loaded.value=true;fit();}
function makePart(){if(!renderer)return;const next=new THREE.Group();modelName.value='几何示意';caption.value='根据焊盘生成的几何示意；本体尺寸与高度仅供预览，不代表实物模型。';
 const pads=props.part.footprint.pads || [];const minX=Math.min(-1,...pads.map(p=>p.x-p.width/2)),maxX=Math.max(1,...pads.map(p=>p.x+p.width/2)),minY=Math.min(-1,...pads.map(p=>p.y-p.height/2)),maxY=Math.max(1,...pads.map(p=>p.y+p.height/2));const w=maxX-minX,h=maxY-minY;const cx=(minX+maxX)/2,cy=(minY+maxY)/2;
 const board=new THREE.Mesh(new THREE.BoxGeometry(w+3,h+3,.3),new THREE.MeshStandardMaterial({color:0x176f62,roughness:.62,metalness:.12}));board.position.set(cx,cy,-.22);next.add(board);
 for(const p of pads){const pad=new THREE.Mesh(new THREE.BoxGeometry(p.width,p.height,.08),new THREE.MeshStandardMaterial({color:0xd8b973,metalness:.72,roughness:.3}));pad.position.set(p.x,p.y,-.02);pad.rotation.z=(p.rotation||0)*Math.PI/180;next.add(pad);}
 const bodyW=Math.max(.5,w*.53),bodyH=Math.max(.5,h*.56),bodyHeight=Math.max(.6,Math.min(w,h)*.23);const body=new THREE.Mesh(new THREE.BoxGeometry(bodyW,bodyH,bodyHeight),new THREE.MeshStandardMaterial({color:0x252a30,roughness:.48}));body.position.set(cx,cy,bodyHeight/2+.07);next.add(body);
 const mark=new THREE.Mesh(new THREE.CylinderGeometry(.15,.15,.02,24),new THREE.MeshStandardMaterial({color:0x9da5ab}));mark.rotation.x=Math.PI/2;mark.position.set(cx-bodyW*.3,cy-bodyH*.3,bodyHeight+.08);next.add(mark);replaceGroup(next);error.value='';
}
function parseOBJ(raw:string,name:string,real:boolean){
 if(raw.length>64*1024*1024)throw new Error('OBJ 模型超过 64 MB，暂不载入。');
 const inlineEasyEDAMaterial=/(?:^|\n)endmtl\s*(?:\r?\n|$)/m.test(raw);
 const parsed=parseMaterialOBJ(raw,real || inlineEasyEDAMaterial);
 if(!parsed.meshes){disposeObject(parsed.object);throw new Error('OBJ 文件没有网格几何。');}
 const next=new THREE.Group();next.add(parsed.object);replaceGroup(next);modelName.value=name;
 caption.value=real?'嘉立创 / EasyEDA 原始 OBJ · 保留源材质与颜色 · Z 轴向上。':'本地 OBJ · 保留内嵌材质，未包含的外部贴图不加载。';
 if(!parsed.embedded)caption.value+=' 此文件未提供内嵌材质，采用文件默认颜色。';
 if(parsed.skippedTextures)caption.value+=' 外部纹理未包含在文件中。';
 error.value='';
}
async function loadReal(){if(loading.value)return;loading.value=true;error.value='';try{const result=await api.loadModel(props.part.id);if(!alive)return;parseOBJ(result.obj,'嘉立创原始模型',true);if(result.warnings?.length)caption.value+=' '+result.warnings.join('；');}catch(e){if(alive)error.value=e instanceof Error?e.message:'模型载入失败';}finally{if(alive)loading.value=false;}}
async function loadOBJ(event:Event){const file=(event.target as HTMLInputElement).files?.[0];if(!file)return;try{if(file.size>64*1024*1024)throw new Error('OBJ 模型超过 64 MB，暂不载入。');const raw=await file.text();if(!alive)return;parseOBJ(raw,file.name,false);}catch(e){if(alive)error.value=e instanceof Error?e.message:'模型读取失败';}finally{if(fileInput.value)fileInput.value.value='';}}
watch(()=>props.dark,()=>{renderer?.setClearColor(props.dark?0x121d25:0xf5f8f8,1);requestRender();});
function onVisibility(){requestRender();}
onMounted(()=>{try{scene=new THREE.Scene();camera=new THREE.PerspectiveCamera(38,1,.01,1000);camera.up.set(0,0,1);renderer=new THREE.WebGLRenderer({antialias:true,alpha:false,powerPreference:'low-power'});renderer.setPixelRatio(Math.min(window.devicePixelRatio,2));renderer.setClearColor(props.dark?0x121d25:0xf5f8f8,1);renderer.outputColorSpace=THREE.SRGBColorSpace;host.value!.appendChild(renderer.domElement);controls=new OrbitControls(camera,renderer.domElement);controls.enableDamping=true;controls.dampingFactor=.09;controls.addEventListener('change',requestRender);const ambient=new THREE.HemisphereLight(0xffffff,0xb8bec5,1.25);ambient.position.set(0,0,1);scene.add(ambient);const key=new THREE.DirectionalLight(0xffffff,2.1);key.position.set(8,-10,16);scene.add(key);const fill=new THREE.DirectionalLight(0xffffff,.8);fill.position.set(-9,5,8);scene.add(fill);observer=new ResizeObserver(([e])=>{if(!e || !renderer || !alive)return;const w=e.contentRect.width,h=e.contentRect.height;if(w<1 || h<1)return;viewportWidth=w;viewportHeight=h;renderer.setSize(w,h);camera.aspect=w/h;camera.updateProjectionMatrix();if(group)fit(false);else requestRender();});observer.observe(host.value!);document.addEventListener('visibilitychange',onVisibility);requestRender();}catch(e){error.value=`3D 显示不可用：${e instanceof Error?e.message:'WebGL 初始化失败'}`;}});
onBeforeUnmount(()=>{alive=false;cancelAnimationFrame(frame);observer?.disconnect();controls?.removeEventListener('change',requestRender);controls?.dispose();document.removeEventListener('visibilitychange',onVisibility);if(group){disposeObject(group);group=undefined;}if(renderer){const canvas=renderer.domElement;renderer.dispose();renderer.forceContextLoss();canvas.remove();renderer=undefined;}});
</script>
<template><div class="model-viewport" ref="host"><div v-if="!loaded" class="model-load"><Icon name="cube" :size="45" :stroke="1"/><h3>{{part.model?.uuid?'查看器件实际模型':'此器件未提供在线模型'}}</h3><p>{{part.model?.uuid?'模型仅在你需要时载入，保持工作台轻盈流畅。':'可以载入本地 OBJ，或查看清楚标注的封装几何示意。'}}</p><div class="model-load-actions"><button v-if="part.model?.uuid" class="primary-button" :disabled="loading" @click="loadReal"><span v-if="loading" class="spinner"></span><Icon v-else name="cube" :size="17"/>{{loading?'正在获取模型…':'加载实际模型'}}</button><button v-else class="secondary-button" @click="fileInput?.click()"><Icon name="import" :size="16"/>载入 OBJ</button><button class="secondary-button" @click="makePart">几何示意</button></div></div><div v-if="loaded" class="model-toolbar"><span class="model-badge"><Icon name="cube" :size="14"/>{{modelName}}</span><button v-if="part.model?.uuid" class="icon-button surface" :disabled="loading" @click="loadReal" title="重新载入原始模型" aria-label="重新载入原始模型"><span v-if="loading" class="spinner tiny"></span><Icon v-else name="refresh" :size="15"/></button><button class="secondary-button compact" @click="fileInput?.click()" title="载入本地 OBJ 模型"><Icon name="import" :size="14"/>OBJ</button><div class="model-presets"><button :class="{active:viewPreset==='top'}" @click="setView('top')">俯视</button><button :class="{active:viewPreset==='oblique'}" @click="setView('oblique')">斜俯视</button></div><button class="icon-button surface" title="复位模型视角" @click="fit()"><Icon name="fit" :size="16"/></button></div><input ref="fileInput" type="file" accept=".obj" hidden @change="loadOBJ"><div v-if="error" class="model-error">{{error}}<button class="text-button" style="margin-left:10px" @click="error=''">关闭</button></div><div v-if="loaded" class="model-caption"><Icon name="info" :size="14"/>{{caption}}</div></div></template>



