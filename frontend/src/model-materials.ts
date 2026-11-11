import * as THREE from 'three';
import { MTLLoader } from 'three/examples/jsm/loaders/MTLLoader.js';
import { OBJLoader } from 'three/examples/jsm/loaders/OBJLoader.js';

// glTF uses Y-up. Keep the file's own scene transforms intact and rotate only
// a wrapper so our Z-up camera's top view still looks down onto its top face.
export function wrapGLBForZUp(object:THREE.Object3D){
 const wrapper=new THREE.Group();wrapper.rotation.x=Math.PI/2;wrapper.add(object);return wrapper;
}

// EasyEDA places colour-only MTL records inside its OBJ stream, between
// newmtl/endmtl. Extract these records before giving geometry to OBJLoader.
// Inline EasyEDA colour records describe opaque solids; their d/Tr fields do
// not carry standard MTL dissolve semantics. Local standard MTL keeps them.
export function parseMaterialOBJ(raw:string,easyedaSource:boolean){
 const geometry:string[]=[];const material:string[]=[];let inMaterial=false;let embedded=false;let skippedTextures=false;
 for(const line of raw.split(/\r?\n/)){
  const text=line.trim();
  if(/^newmtl\s/.test(text)){inMaterial=true;embedded=true;material.push(text);continue;}
  if(text==='endmtl'){inMaterial=false;continue;}
  if(inMaterial){
   if(/^(map_|bump\b|disp\b|decal\b|refl\b)/i.test(text)){skippedTextures=true;continue;}
   if(easyedaSource && /^d\s+/i.test(text))material.push('d 1');
   else if(easyedaSource && /^Tr\s+/i.test(text))material.push('Tr 0');
   else material.push(text);
  }else if(!/^mtllib\s/.test(text))geometry.push(line);
 }
 const loader=new OBJLoader();
 let creator:MTLLoader.MaterialCreator|undefined;
 if(embedded){const mtlLoader=new MTLLoader();mtlLoader.setMaterialOptions({side:THREE.DoubleSide});creator=mtlLoader.parse(material.join('\n'),'');creator.preload();loader.setMaterials(creator);}
 const object=loader.parse(geometry.join('\n'));
 let meshes=0;const names=new Set<string>();
 object.traverse(child=>{
  if(!(child instanceof THREE.Mesh))return;
  meshes++;
  const materials:THREE.Material[]=Array.isArray(child.material)?child.material:[child.material];
  for(const item of materials){item.side=THREE.DoubleSide;names.add(item.name || '未命名材质');}
 });
 // MTL records may define unused materials; release these too.
 const used=new Set<THREE.Material>();object.traverse(child=>{if(child instanceof THREE.Mesh){for(const item of Array.isArray(child.material)?child.material:[child.material])used.add(item);}});
 if(creator)for(const item of Object.values(creator.materials))if(!used.has(item))item.dispose();
 return {object,meshes,materialNames:[...names],embedded,skippedTextures};
}
