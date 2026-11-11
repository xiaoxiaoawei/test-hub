<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { parseMaterialOBJ, wrapGLBForZUp } from '../model-materials';
import { createModelLoadGuard, validateLocalGLB } from '../model-state';
import type { Part } from '../types';
import { api } from '../api';
import Icon from './Icon.vue';
const props=defineProps<{part:Part;dark:boolean;visible:boolean}>();
const host=ref<HTMLElement>();const fileInput=ref<HTMLInputElement>();const error=ref('');const modelName=ref('尚未加载模型');const loaded=ref(false);const loading=ref(false);const caption=ref('');
const viewPreset=ref<'top'|'oblique'>('oblique');
let renderer:THREE.WebGLRenderer|undefined;let scene:THREE.Scene;let camera:THREE.PerspectiveCamera;let controls:OrbitControls;let group:THREE.Group|undefined;let observer:ResizeObserver;let frame=0;let alive=true;
let viewportWidth=1;let viewportHeight=1;let pendingFit=false;const loadGuard=createModelLoadGuard();
function disposeObject(object:THREE.Object3D){
 const geometries=new Set<THREE.BufferGeometry>(),materials=new Set<THREE.Material>(),textures=new Set<THREE.Texture>();
 object.traverse(o=>{if(o instanceof THREE.Mesh){geometries.add(o.geometry);for(const m of Array.isArray(o.material)?o.material:[o.material]){materials.add(m);for(const value of Object.values(m))if(value instanceof THREE.Texture)textures.add(value);}}});
 textures.forEach(value=>value.dispose());materials.forEach(value=>value.dispose());geometries.forEach(value=>value.dispose());
}
function stopRendering(){cancelAnimationFrame(frame);frame=0;}
function requestRender(){if(!alive || !props.visible || frame || !renderer || document.hidden)return;frame=requestAnimationFrame(()=>{frame=0;if(!alive || !props.visible || !renderer || document.hidden)return;const changed=controls.update();renderer.render(scene,camera);if(changed && controls.enableDamping)requestRender();});}
function fit(resetOrientation=true){
 if(!group || !renderer || !camera || !controls)return;
 if(!props.visible || viewportWidth<2 || viewportHeight<2){pendingFit=true;return;}pendingFit=false;
 const box=new THREE.Box3().setFromObject(group);if(box.isEmpty())return;
 const sphere=box.getBoundingSphere(new THREE.Sphere());const center=sphere.center;const radius=Math.max(sphere.radius,.001);if(!Number.isFinite(radius))return;
 const verticalHalfFOV=THREE.MathUtils.degToRad(camera.getEffectiveFOV())/2;
 const horizontalHalfFOV=Math.atan(Math.tan(verticalHalfFOV)*Math.max(.001,camera.aspect));
 // Leave room for the toolbar and caption. The horizontal FOV is the limiting
 // factor in a narrow split pane, even when its vertical space is generous.
 const horizontalSpace=Math.max(.15,(viewportWidth-36)/Math.max(1,viewportWidth));
 const overlaySpace=viewportWidth<250?150:110;
 const verticalSpace=Math.max(.15,(viewportHeight-overlaySpace)/Math.max(1,viewportHeight));
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
function makePart(){if(!renderer)return;loadGuard.begin();loading.value=false;const next=new THREE.Group();modelName.value='几何示意';caption.value='根据焊盘生成的几何示意；本体尺寸与高度仅供预览，不代表实物模型。';
 const pads=props.part.footprint.pads || [];const minX=Math.min(-1,...pads.map(p=>p.x-p.width/2)),maxX=Math.max(1,...pads.map(p=>p.x+p.width/2)),minY=Math.min(-1,...pads.map(p=>p.y-p.height/2)),maxY=Math.max(1,...pads.map(p=>p.y+p.height/2));const w=maxX-minX,h=maxY-minY;const cx=(minX+maxX)/2,cy=(minY+maxY)/2;
 const board=new THREE.Mesh(new THREE.BoxGeometry(w+3,h+3,.3),new THREE.MeshStandardMaterial({color:0x176f62,roughness:.62,metalness:.12}));board.position.set(cx,cy,-.22);next.add(board);
 for(const p of pads){const pad=new THREE.Mesh(new THREE.BoxGeometry(p.width,p.height,.08),new THREE.MeshStandardMaterial({color:0xd8b973,metalness:.72,roughness:.3}));pad.position.set(p.x,p.y,-.02);pad.rotation.z=(p.rotation||0)*Math.PI/180;next.add(pad);}
 const bodyW=Math.max(.5,w*.53),bodyH=Math.max(.5,h*.56),bodyHeight=Math.max(.6,Math.min(w,h)*.23);const body=new THREE.Mesh(new THREE.BoxGeometry(bodyW,bodyH,bodyHeight),new THREE.MeshStandardMaterial({color:0x252a30,roughness:.48}));body.position.set(cx,cy,bodyHeight/2+.07);next.add(body);
 const mark=new THREE.Mesh(new THREE.CylinderGeometry(.15,.15,.02,24),new THREE.MeshStandardMaterial({color:0x9da5ab}));mark.rotation.x=Math.PI/2;mark.position.set(cx-bodyW*.3,cy-bodyH*.3,bodyHeight+.08);next.add(mark);replaceGroup(next);error.value='';
}
function parseOBJ(raw:string,name:string,real:boolean,token:number){
 if(raw.length>64*1024*1024)throw new Error('OBJ 模型超过 64 MB，暂不载入。');
 const inlineEasyEDAMaterial=/(?:^|\n)endmtl\s*(?:\r?\n|$)/m.test(raw);
 const parsed=parseMaterialOBJ(raw,real || inlineEasyEDAMaterial);
 if(!parsed.meshes){disposeObject(parsed.object);throw new Error('OBJ 文件没有网格几何。');}
 if(!alive || !loadGuard.isCurrent(token)){disposeObject(parsed.object);return;}
 const next=new THREE.Group();next.add(parsed.object);replaceGroup(next);modelName.value=name;
 caption.value=real?'嘉立创 / EasyEDA 原始 OBJ 预览（非最终 EDA 装配对位）· 保留源材质与颜色 · Z 轴向上。':'本地 OBJ · 保留内嵌材质，未包含的外部贴图不加载。';
 if(!parsed.embedded)caption.value+=' 此文件未提供内嵌材质，采用文件默认颜色。';
 if(parsed.skippedTextures)caption.value+=' 外部纹理未包含在文件中。';
 error.value='';
}
async function loadReal(){
 const token=loadGuard.begin();loading.value=true;error.value='';
 try{const result=await api.loadModel(props.part.id);if(!alive || !loadGuard.isCurrent(token))return;parseOBJ(result.obj,'嘉立创原始模型',true,token);if(result.warnings?.length)caption.value+=' '+result.warnings.join('；');}
 catch(e){if(alive && loadGuard.isCurrent(token))error.value=e instanceof Error?e.message:'模型载入失败';}
 finally{if(alive && loadGuard.isCurrent(token))loading.value=false;}
}
async function parseGLB(buffer:ArrayBuffer,name:string,token:number){
 validateLocalGLB(buffer);const {GLTFLoader}=await import('three/examples/jsm/loaders/GLTFLoader.js');
 if(!alive || !loadGuard.isCurrent(token))return;
 const manager=new THREE.LoadingManager();manager.setURLModifier(url=>{if(url.startsWith('blob:') || url.startsWith('data:'))return url;throw new Error('GLB 外部资源已阻止。');});
 const parsed=await new GLTFLoader(manager).parseAsync(buffer,'');
 const object=parsed.scene;if(!object)throw new Error('GLB 文件没有场景。');
 if(!alive || !loadGuard.isCurrent(token)){disposeObject(object);return;}
 const next=wrapGLBForZUp(object);replaceGroup(next);modelName.value=name;caption.value='本地 GLB · 保留文件材质与内嵌纹理；Y-up 已转为 Z-up，未加载外部资源。';error.value='';
}
async function loadOBJ(event:Event){
 const input=event.target as HTMLInputElement;const file=input.files?.[0];if(!file)return;
 const token=loadGuard.begin();loading.value=true;error.value='';
 try{
  if(file.size>64*1024*1024)throw new Error('模型超过 64 MB，暂不载入。');
  if(/\.glb$/i.test(file.name)){const buffer=await file.arrayBuffer();if(alive && loadGuard.isCurrent(token))await parseGLB(buffer,file.name,token);}
  else{const raw=await file.text();if(alive && loadGuard.isCurrent(token))parseOBJ(raw,file.name,false,token);}
 }catch(e){if(alive && loadGuard.isCurrent(token))error.value=e instanceof Error?e.message:'模型读取失败';}
 finally{input.value='';if(alive && loadGuard.isCurrent(token))loading.value=false;}
}
watch(()=>props.dark,()=>{renderer?.setClearColor(props.dark?0x121d25:0xf5f8f8,1);requestRender();});
function pauseView(){
 stopRendering();if(!controls || !camera)return;
 // Flush damped input without moving the saved view while this pane is hidden.
 const position=camera.position.clone(),target=controls.target.clone(),quaternion=camera.quaternion.clone();
 const damping=controls.enableDamping;controls.enableDamping=false;controls.update();controls.enableDamping=damping;
 camera.position.copy(position);camera.quaternion.copy(quaternion);controls.target.copy(target);
}
function resizeViewport(w:number,h:number){if(w<1 || h<1 || !renderer || !alive)return;viewportWidth=w;viewportHeight=h;renderer.setSize(w,h);camera.aspect=w/h;camera.updateProjectionMatrix();if(group && pendingFit)fit();else requestRender();}
watch(()=>props.visible,visible=>{if(controls)controls.enabled=visible;if(!visible)pauseView();else void nextTick(()=>{if(alive && props.visible && host.value)resizeViewport(host.value.clientWidth,host.value.clientHeight);});});
function onVisibility(){if(document.hidden)pauseView();else requestRender();}
onMounted(()=>{try{scene=new THREE.Scene();camera=new THREE.PerspectiveCamera(38,1,.01,1000);camera.up.set(0,0,1);renderer=new THREE.WebGLRenderer({antialias:true,alpha:false,powerPreference:'low-power'});renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1,1.25));renderer.setClearColor(props.dark?0x121d25:0xf5f8f8,1);renderer.outputColorSpace=THREE.SRGBColorSpace;host.value!.appendChild(renderer.domElement);controls=new OrbitControls(camera,renderer.domElement);controls.enabled=props.visible;controls.enableDamping=true;controls.dampingFactor=.09;controls.addEventListener('change',requestRender);const ambient=new THREE.HemisphereLight(0xffffff,0xb8bec5,1.25);ambient.position.set(0,0,1);scene.add(ambient);const key=new THREE.DirectionalLight(0xffffff,2.1);key.position.set(8,-10,16);scene.add(key);const fill=new THREE.DirectionalLight(0xffffff,.8);fill.position.set(-9,5,8);scene.add(fill);observer=new ResizeObserver(([e])=>{if(!e || !renderer || !alive)return;resizeViewport(e.contentRect.width,e.contentRect.height);});observer.observe(host.value!);document.addEventListener('visibilitychange',onVisibility);requestRender();}catch(e){error.value=`3D 显示不可用：${e instanceof Error?e.message:'WebGL 初始化失败'}`;}});
onBeforeUnmount(()=>{alive=false;loadGuard.invalidate();stopRendering();observer?.disconnect();controls?.removeEventListener('change',requestRender);controls?.dispose();document.removeEventListener('visibilitychange',onVisibility);if(group){disposeObject(group);group=undefined;}if(renderer){const canvas=renderer.domElement;renderer.dispose();renderer.forceContextLoss();canvas.remove();renderer=undefined;}});
</script>
<template><div class="model-viewport" ref="host"><div v-if="!loaded" class="model-load"><Icon name="cube" :size="45" :stroke="1"/><h3>{{part.model?.uuid?'查看器件实际模型':'此器件未提供在线模型'}}</h3><p>{{part.model?.uuid?'模型仅在你需要时载入，保持工作台轻盈流畅。':'可以载入本地 OBJ / GLB，或查看清楚标注的封装几何示意。'}}</p><div class="model-load-actions"><button v-if="part.model?.uuid" class="primary-button" :disabled="loading" @click="loadReal"><span v-if="loading" class="spinner"></span><Icon v-else name="cube" :size="17"/>{{loading?'正在获取模型…':'加载实际模型'}}</button><button v-else class="secondary-button" @click="fileInput?.click()"><Icon name="import" :size="16"/>载入模型</button><button class="secondary-button" @click="makePart">几何示意</button></div></div><div v-if="loaded" class="model-toolbar"><span class="model-badge"><Icon name="cube" :size="14"/>{{modelName}}</span><button v-if="part.model?.uuid" class="icon-button surface" :disabled="loading" @click="loadReal" title="重新载入原始模型" aria-label="重新载入原始模型"><span v-if="loading" class="spinner tiny"></span><Icon v-else name="refresh" :size="15"/></button><button class="secondary-button compact" @click="fileInput?.click()" title="载入本地 OBJ / GLB 模型"><Icon name="import" :size="14"/>模型</button><div class="model-presets"><button :class="{active:viewPreset==='top'}" @click="setView('top')">俯视</button><button :class="{active:viewPreset==='oblique'}" @click="setView('oblique')">斜俯视</button></div><button class="icon-button surface" title="复位模型视角" @click="fit()"><Icon name="fit" :size="16"/></button></div><input ref="fileInput" type="file" accept=".obj,.glb" hidden @change="loadOBJ"><div v-if="error" class="model-error">{{error}}<button class="text-button" style="margin-left:10px" @click="error=''">关闭</button></div><div v-if="loaded" class="model-caption"><Icon name="info" :size="14"/>{{caption}}</div></div></template>



