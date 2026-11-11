import type { ModelAssociation } from './types';

export function modelAssociationConfigurationError(target:string,nativeLibrary:boolean,includeModel:boolean,mode:string){
 return target==='pads' && nativeLibrary && includeModel && mode==='native'
  ?'Classic 9.4 的 2D 原生库生成与现代 Reuse 写入暂不同时执行。请选择“生成 3D 关联包”，或关闭上方 2D 原生模式，使用 ASCII 输出配合现代 Reuse。'
  :'';
}

export function isModelMappingVerified(item:ModelAssociation){return item.status==='associated' && item.verified===true;}
export function isModelAssociationVerified(item:ModelAssociation){return isModelMappingVerified(item) && item.geometryVerified===true;}
export function modelAssociationLabel(item:ModelAssociation){
 if(isModelAssociationVerified(item))return '已关联并回读';
 if(item.status==='associated')return item.verified?'关联已回读 · 位置/尺寸待 EDA 验收':'关联待回读';
 if(item.status==='prepared')return '已准备，待关联回读';
 if(item.status==='unavailable')return '无可用源模型';
 if(item.status==='unsupported')return '当前条件不支持关联';
 return '关联状态待核对';
}
export function modelBBoxLabel(values?:number[]){
 if(values?.length!==6 || !values.every(Number.isFinite))return '';
 const n=(value:number)=>Number(value.toFixed(4)).toString();
 return `X ${n(values[0]!)} ~ ${n(values[3]!)} · Y ${n(values[1]!)} ~ ${n(values[4]!)} · Z ${n(values[2]!)} ~ ${n(values[5]!)}`;
}
export function modelAssociationSummary(items:ModelAssociation[]){
 if(!items.length)return '';
 const mapped=items.filter(isModelMappingVerified).length,geometry=items.filter(isModelAssociationVerified).length;
 if(geometry===items.length)return `${geometry} 项关联已回读，几何范围已核验`;
 const unavailable=items.filter(item=>item.status==='unavailable').length;
 if(unavailable===items.length)return '没有可关联的源模型';
 return `${mapped} / ${items.length} 项关联已回读 · ${geometry} / ${items.length} 项几何范围已核验`;
}
export function modelAssociationLog(item:ModelAssociation){
 const target=item.target==='pads'?'PADS':item.target==='cadence'?'Cadence':item.target;
 return `${target} · ${item.entry || item.partId} · ${modelAssociationLabel(item)}${item.message?'：'+item.message:'。'}`;
}
