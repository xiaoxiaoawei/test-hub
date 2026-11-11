import assert from 'node:assert/strict';
import fs from 'node:fs';
import { parseMaterialOBJ } from '../src/model-materials.ts';
import { Box3, Mesh, SRGBColorSpace, Vector3 } from 'three';
const synthetic='newmtl black\nKd 0.25 0.25 0.25\nKs 0.08 0.08 0.08\nd 0.0\nendmtl\nnewmtl silver\nKd 0.77 0.77 0.77\nKs 0.62 0.62 0.62\nNs 80\nd 0.0\nendmtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nusemtl black\nf 1 2 3\nusemtl silver\nf 2 4 3\n';
function materials(parsed){const values=new Map();parsed.object.traverse(o=>{if(o instanceof Mesh)for(const m of Array.isArray(o.material)?o.material:[o.material])values.set(m.name,m);});return values;}
const sample=parseMaterialOBJ(synthetic,true);const sampleMaterials=materials(sample);assert.equal(sampleMaterials.size,2);assert.equal(sampleMaterials.get('black').opacity,1);assert.equal(sampleMaterials.get('silver').shininess,80);assert.ok(Math.abs(sampleMaterials.get('black').color.clone().convertLinearToSRGB().r-.25)<.001);assert.notEqual(sampleMaterials.get('black').color.getHex(),sampleMaterials.get('silver').color.getHex());
assert.equal(materials(parseMaterialOBJ(synthetic,false)).get('black').opacity,0,'Local standard MTL opacity must remain standard.');
if(process.argv[2]){const real=parseMaterialOBJ(fs.readFileSync(process.argv[2],'utf8'),true);const values=[...materials(real).values()];const bounds=new Box3().setFromObject(real.object);const extents=bounds.getSize(new Vector3());assert.ok(real.meshes>0);assert.ok(values.length>=2);assert.ok(values.every(m=>m.opacity===1));assert.ok(extents.z<Math.min(extents.x,extents.y),'Chip height must be on Z.');console.log(JSON.stringify({meshCount:real.meshes,extents:extents.toArray(),materials:values.map(m=>({name:m.name,color:`#${m.color.getHexString(SRGBColorSpace)}`,opacity:m.opacity,shininess:m.shininess}))},null,2));}
console.log('Material preservation and opacity tests passed.');
