import assert from 'node:assert/strict';
import fs from 'node:fs';
import { parseMaterialOBJ, wrapGLBForZUp } from '../src/model-materials.ts';
import { Box3, BoxGeometry, Mesh, MeshBasicMaterial, SRGBColorSpace, Vector3 } from 'three';
const synthetic='newmtl black\nKd 0.25 0.25 0.25\nKs 0.08 0.08 0.08\nd 0.0\nendmtl\nnewmtl silver\nKd 0.77 0.77 0.77\nKs 0.62 0.62 0.62\nNs 80\nd 0.0\nendmtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nusemtl black\nf 1 2 3\nusemtl silver\nf 2 4 3\n';
function materials(parsed){const values=new Map();parsed.object.traverse(o=>{if(o instanceof Mesh)for(const m of Array.isArray(o.material)?o.material:[o.material])values.set(m.name,m);});return values;}
const sample=parseMaterialOBJ(synthetic,true);const sampleMaterials=materials(sample);assert.equal(sampleMaterials.size,2);assert.equal(sampleMaterials.get('black').opacity,1);assert.equal(sampleMaterials.get('silver').shininess,80);assert.ok(Math.abs(sampleMaterials.get('black').color.clone().convertLinearToSRGB().r-.25)<.001);assert.notEqual(sampleMaterials.get('black').color.getHex(),sampleMaterials.get('silver').color.getHex());
assert.equal(materials(parseMaterialOBJ(synthetic,false)).get('black').opacity,0,'Local standard MTL opacity must remain standard.');
if(process.argv[2]){const real=parseMaterialOBJ(fs.readFileSync(process.argv[2],'utf8'),true);const values=[...materials(real).values()];const bounds=new Box3().setFromObject(real.object);const extents=bounds.getSize(new Vector3());assert.ok(real.meshes>0);assert.ok(values.length>=2);assert.ok(values.every(m=>m.opacity===1));assert.ok(extents.z<Math.min(extents.x,extents.y),'Chip height must be on Z.');console.log(JSON.stringify({meshCount:real.meshes,extents:extents.toArray(),materials:values.map(m=>({name:m.name,color:`#${m.color.getHexString(SRGBColorSpace)}`,opacity:m.opacity,shininess:m.shininess}))},null,2));}
console.log('Material preservation and opacity tests passed.');

for(const [record,expected] of [['d 0.5',.5],['Tr 1',0],['d 0.5\nTr 1',0]]){
 const text=synthetic.replaceAll('d 0.0',record);
 assert.ok([...materials(parseMaterialOBJ(text,true)).values()].every(value=>value.opacity===1),'EasyEDA solids stay opaque for '+record);
 assert.ok([...materials(parseMaterialOBJ(text,false)).values()].every(value=>value.opacity===expected),'Local MTL keeps standard opacity for '+record);
}
console.log('EasyEDA d/Tr normalization and local MTL transparency passed.');

const glbMesh=new Mesh(new BoxGeometry(2,6,3),new MeshBasicMaterial());glbMesh.position.y=4;
const oriented=wrapGLBForZUp(glbMesh);oriented.updateMatrixWorld(true);const glbBounds=new Box3().setFromObject(oriented);const glbSize=glbBounds.getSize(new Vector3());
assert.ok(Math.abs(glbSize.x-2)<1e-9 && Math.abs(glbSize.y-3)<1e-9 && Math.abs(glbSize.z-6)<1e-9,'GLB Y height must become Z height for our top view.');
assert.ok(Math.abs(glbBounds.getCenter(new Vector3()).z-4)<1e-9);assert.equal(glbMesh.position.y,4,'Keep the original GLB scene transform unchanged.');glbMesh.geometry.dispose();glbMesh.material.dispose();
console.log('Local GLB Y-up to Z-up orientation passed.');
