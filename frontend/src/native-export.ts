import type { ExportMetadata, NativeConfig } from './types';

export function defaultNativeConfig():NativeConfig {
 return {enabled:false,libraryMode:'new',padsRoot:'',padsSymbolLibrary:'',padsFootprintLibrary:'',captureExe:'',allegroExe:'',allegroProduct:'',captureLibrary:'',allegroDirectory:'',captureUnitsPerInch:100,modelAssociationMode:'prepare',padsModelExe:'',padsReuseDirectory:''};
}
export function normalizeNativeConfig(value?:Partial<NativeConfig>):NativeConfig {
 const defaults=defaultNativeConfig();const result={...defaults,...value};
 result.enabled=value?.enabled===true;result.libraryMode=value?.libraryMode==='append'?'append':'new';
 result.modelAssociationMode=value?.modelAssociationMode==='native'?'native':'prepare';
 for(const key of ['padsRoot','padsSymbolLibrary','padsFootprintLibrary','captureExe','allegroExe','allegroProduct','captureLibrary','allegroDirectory','padsModelExe','padsReuseDirectory'] as const)result[key]=typeof result[key]==='string'?result[key]:'';
 if(!Number.isFinite(result.captureUnitsPerInch) || result.captureUnitsPerInch<=0)result.captureUnitsPerInch=100;
 return result;
}
export function generatedLabel(target:string,meta?:ExportMetadata){
 if(meta?.nativeGenerated)return meta.runtimeVerified?'原生库已生成 · 引擎已回读':'原生库已生成 · 待 EDA 验证';
 if(meta?.artifactType?.includes('pending') || meta?.validationStatus==='pending_engine')return '待本机 EDA 生成';
 return target==='cadence'?'脚本包已生成':target==='pads'?'导入包已生成':'已完成';
}
export function generatedMessage(target:string,count:number,meta?:ExportMetadata){
 if(meta?.nativeGenerated)return `${generatedLabel(target,meta)}，共 ${count} 个文件。仍需在目标 EDA 中验收几何、引脚和模型。`;
 if(meta?.artifactType?.includes('pending') || meta?.validationStatus==='pending_engine')return `已生成 ${count} 个中间文件，等待本机 EDA 生成及回读原生库。`;
 return target==='cadence'?`Cadence 脚本包已生成，共 ${count} 个文件；需在目标 EDA 中执行。`:target==='pads'?`PADS ASCII 导入包已生成，共 ${count} 个文件；需在 PADS 中导入。`:`导出完成，共生成 ${count} 个文件。`;
}
export function validationLabel(meta:ExportMetadata){
 if(meta.nativeGenerated && meta.runtimeVerified)return '本次目标引擎回读完成，仍需人工验收。';
 if(meta.nativeGenerated)return '原生文件已生成，尚未完成目标引擎回读。';
 if(meta.validationStatus==='awaiting-eda' || meta.validationStatus==='pending_engine')return '等待目标 EDA 生成原生库并回读。';
 if(meta.validationStatus==='format-checked')return '文件格式检查完成，尚未由目标 EDA 回读。';
 return '请在目标 EDA 中核对实际结果。';
}
