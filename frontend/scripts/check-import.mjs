import assert from 'node:assert/strict';
import { isModelAssociationVerified, isModelMappingVerified, modelAssociationLabel, modelAssociationSummary, modelBBoxLabel, modelAssociationConfigurationError } from '../src/model-association.ts';
import { decodePartList, partListBatches } from '../src/part-list.ts';
import { defaultNativeConfig, normalizeNativeConfig, generatedLabel, generatedMessage, validationLabel } from '../src/native-export.ts';
const source='编号,型号\r\nC23922,STM32F030C8T6';const utf8=new TextEncoder().encode(source);
assert.equal(decodePartList(utf8).text,source);
assert.equal(decodePartList(new Uint8Array([0xef,0xbb,0xbf,...utf8])).text,source);
function utf16(text,bigEndian,bom=true){const bytes=[];if(bom)bytes.push(...(bigEndian?[0xfe,0xff]:[0xff,0xfe]));for(let i=0;i<text.length;i++){const code=text.charCodeAt(i);bytes.push(...(bigEndian?[code>>8,code&255]:[code&255,code>>8]));}return new Uint8Array(bytes);}
for(const be of [true,false])for(const bom of [true,false])assert.equal(decodePartList(utf16(source,be,bom)).text,source);
const gb18030=new Uint8Array([0xb1,0xe0,0xba,0xc5,0x0a,...new TextEncoder().encode('C23922')]);assert.equal(decodePartList(gb18030).text,'编号\nC23922');assert.equal(decodePartList(gb18030).encoding,'gb18030');
assert.throws(()=>decodePartList(new Uint8Array([0xff,0xfe,0x41])),/编码/);
const ids=Array.from({length:500},(_,index)=>`C${index+100}`);const batches=partListBatches(ids);
assert.equal(batches.length,10);assert.ok(batches.every(batch=>batch.length===50));assert.deepEqual(batches.flat(),ids);
assert.deepEqual(partListBatches(['c23922','C23922']),[['C23922']]);
assert.throws(()=>partListBatches(['STM32F030C8T6']),/无效/);assert.throws(()=>partListBatches(['C8']),/无效/);
assert.throws(()=>partListBatches([...ids,'C99999']),/500/);assert.throws(()=>partListBatches(ids,51),/50/);
assert.equal(defaultNativeConfig().enabled,false);assert.equal(normalizeNativeConfig({libraryMode:'wrong',captureUnitsPerInch:NaN}).libraryMode,'new');
assert.equal(normalizeNativeConfig({captureUnitsPerInch:NaN}).captureUnitsPerInch,100);
assert.match(generatedLabel('cadence',{artifactType:'intermediate'}),/脚本/);
assert.match(generatedMessage('pads',3,{artifactType:'intermediate'}),/ASCII/);
assert.match(generatedLabel('pads',{nativeGenerated:true,runtimeVerified:false}),/待/);
assert.match(generatedLabel('pads',{nativeGenerated:true,runtimeVerified:true}),/回读/);
assert.doesNotMatch(validationLabel({validationStatus:'format-checked'}),/回读完成/);
assert.doesNotMatch(validationLabel({nativeGenerated:false,runtimeVerified:true}),/回读完成/,'Both native output and its readback must be present before reporting engine verification.');
assert.match(validationLabel({validationStatus:'awaiting-eda'}),/等待/);
console.log('List encoding, bounded serial batches and truthful native-result states passed.');

const association={partId:'C23922',entry:'LQFP48',target:'cadence',status:'prepared',verified:false,geometryVerified:false};
assert.equal(isModelAssociationVerified(association),false);assert.doesNotMatch(modelAssociationLabel(association),/^已关联/);
assert.equal(isModelAssociationVerified({...association,status:'associated',verified:false}),false);
assert.equal(isModelAssociationVerified({...association,status:'prepared',verified:true}),false);
assert.equal(isModelAssociationVerified({...association,status:'associated',verified:true,geometryVerified:true}),true);
for(const status of ['unavailable','unsupported'])assert.equal(isModelAssociationVerified({...association,status,verified:true}),false);
assert.equal(modelAssociationSummary([]),'');assert.match(modelAssociationSummary([{...association,status:'unavailable'}]),/没有/);
assert.match(modelAssociationSummary([association,{...association,status:'associated',verified:true,geometryVerified:true}]),/1 \/ 2/);
console.log('Model association states require actual association and verified readback together.');

assert.equal(isModelAssociationVerified({...association,status:'associated',verified:true,geometryVerified:false}),false,'A mapping without geometry readback is not verified.');
assert.equal(normalizeNativeConfig({}).modelAssociationMode,'prepare');

assert.equal(modelBBoxLabel([0,0,0,1,2,3]),'X 0 ~ 1 · Y 0 ~ 2 · Z 0 ~ 3');
for(const bbox of [undefined,[],[0,1,2],[0,0,0,1,2,Infinity],[0,0,0,1,2,NaN]])assert.equal(modelBBoxLabel(bbox),'');
assert.equal(normalizeNativeConfig({modelAssociationMode:'invalid'}).modelAssociationMode,'prepare');

assert.equal(isModelMappingVerified({...association,status:'associated',verified:true,geometryVerified:false}),true);
assert.match(modelAssociationLabel({...association,status:'associated',verified:true,geometryVerified:false}),/关联已回读.*位置\/尺寸待 EDA 验收/);
assert.match(modelAssociationSummary([{...association,status:'associated',verified:true,geometryVerified:false}]),/1 \/ 1 项关联已回读.*0 \/ 1 项几何范围已核验/);
assert.equal(modelAssociationSummary([{...association,status:'associated',verified:true,geometryVerified:true}]),'1 项关联已回读，几何范围已核验');
assert.doesNotMatch(modelAssociationSummary([{...association,status:'associated',verified:true,geometryVerified:true}]),/位置\/尺寸均已|完整几何/);

assert.match(modelAssociationConfigurationError('pads',true,true,'native'),/不同时执行/);
assert.equal(modelAssociationConfigurationError('pads',false,true,'native'),'');
assert.equal(modelAssociationConfigurationError('pads',true,true,'prepare'),'');
assert.equal(modelAssociationConfigurationError('pads',true,false,'native'),'');
assert.equal(modelAssociationConfigurationError('cadence',true,true,'native'),'');
