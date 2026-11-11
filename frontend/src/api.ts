import type { Bootstrap, ExportHistoryEntry, ExportRequest, ExportResult, Part, Settings, UpdateDownloadResult, UpdateInfo } from './types';
declare global { interface Window { go?:{ main?:{ App?:Record<string,(...args:unknown[])=>Promise<unknown>> } } } }
async function call<T>(name:string, path:string, method='GET', body?:unknown):Promise<T> {
  const app=window.go?.main?.App;
  if(app?.[name]) return await app[name](...(body === undefined ? [] : name === 'FetchPart' ? [(body as {lcscID:string}).lcscID] : name === 'ImportPartJSON' ? [(body as {raw:string}).raw] : (name === 'LoadModel' || name === 'RemovePart') ? [(body as {partId:string}).partId] : (name === 'OpenOutputDirectory' || name === 'RevealGeneratedFile' || name === 'InstallUpdate') ? [(body as {path:string}).path] : [body])) as T;
  const response=await fetch(path,{method,headers:{'Content-Type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)})});
  const data=await response.json().catch(()=>({error:`服务返回异常（${response.status}）`}));
  if(!response.ok || data?.error) throw new Error(data?.error || `请求失败（${response.status}）`);
  return data as T;
}
export const api={
 bootstrap:()=>call<Bootstrap>('GetBootstrap','/api/bootstrap'),
 fetchPart:(lcscID:string)=>call<Part>('FetchPart','/api/parts/fetch','POST',{lcscID}),
 importPart:(raw:string)=>call<Part>('ImportPartJSON','/api/parts/import','POST',{raw}),
 loadModel:(partId:string)=>call<{partId:string;uuid:string;obj:string;warnings:string[]}>('LoadModel','/api/model','POST',{partId}),
 removePart:(partId:string)=>call<void>('RemovePart','/api/parts/remove','POST',{partId}),
 history:()=>call<ExportHistoryEntry[]>('GetExportHistory','/api/history'),
 clearHistory:()=>call<void>('ClearExportHistory','/api/history/clear','POST'),
 revealFile:(path:string)=>call<void>('RevealGeneratedFile','/api/reveal-file','POST',{path}),
 exportParts:(request:ExportRequest)=>call<ExportResult>('ExportParts','/api/export','POST',request),
 saveSettings:(settings:Settings)=>call<Settings>('SaveSettings','/api/settings','PUT',settings),
 chooseDirectory:()=>call<string>('ChooseOutputDirectory','/api/dialog/directory','POST'),
 openDirectory:(path:string)=>call<void>('OpenOutputDirectory','/api/open-directory','POST',{path}),
 checkUpdates:()=>call<UpdateInfo>('CheckForUpdates','/api/update/check'),
 downloadUpdate:()=>call<UpdateDownloadResult>('DownloadUpdate','/api/update/download','POST'),
 installUpdate:(path:string)=>call<void>('InstallUpdate','/api/update/install','POST',{path}),
};


