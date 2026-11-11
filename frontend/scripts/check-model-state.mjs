import assert from 'node:assert/strict';
import { createModelLoadGuard, validateLocalGLB } from '../src/model-state.ts';
const guard=createModelLoadGuard();
const network=guard.begin(),local=guard.begin();
assert.equal(guard.isCurrent(network),false,'A later local load must supersede the pending network load.');
assert.equal(guard.isCurrent(local),true);
const geometry=guard.begin();assert.equal(guard.isCurrent(local),false);assert.equal(guard.isCurrent(geometry),true);
guard.invalidate();assert.equal(guard.isCurrent(geometry),false,'Unmount must invalidate pending completion/error updates.');
function glb(json){const raw=new TextEncoder().encode(JSON.stringify(json));const length=Math.ceil(raw.length/4)*4;const buffer=new ArrayBuffer(20+length);const view=new DataView(buffer);view.setUint32(0,0x46546c67,true);view.setUint32(4,2,true);view.setUint32(8,buffer.byteLength,true);view.setUint32(12,length,true);view.setUint32(16,0x4e4f534a,true);new Uint8Array(buffer,20).fill(0x20);new Uint8Array(buffer,20,raw.length).set(raw);return buffer;}
assert.doesNotThrow(()=>validateLocalGLB(glb({asset:{version:'2.0'},buffers:[{byteLength:12}],images:[{bufferView:0,mimeType:'image/png'}]})));
assert.doesNotThrow(()=>validateLocalGLB(glb({asset:{version:'2.0'},images:[{uri:'data:image/png;base64,AA=='}]})));
for(const uri of ['https://example.invalid/model.bin','file:///C:/private/model.bin','texture.png'])assert.throws(()=>validateLocalGLB(glb({buffers:[{uri}]})),/外部/);
for(const extension of ['KHR_draco_mesh_compression','EXT_meshopt_compression','KHR_texture_basisu'])assert.throws(()=>validateLocalGLB(glb({extensionsUsed:[extension]})),/解码器/);
const truncated=glb({asset:{version:'2.0'}}).slice(0,21);assert.throws(()=>validateLocalGLB(truncated),/完整/);
assert.throws(()=>validateLocalGLB(new ArrayBuffer(8)),/不完整/);
console.log('Model source supersession, disposal invalidation and embedded-only GLB boundaries passed.');
