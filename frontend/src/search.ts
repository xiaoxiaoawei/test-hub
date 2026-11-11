export type SearchInput={kind:'empty';query:'';ids:[]}|{kind:'ids';query:string;ids:string[]}|{kind:'keyword';query:string;ids:[]};

// Only entire C-number tokens enter the direct-fetch route. A model such as
// STM32F030C8T6 must never be reduced to its embedded C8 substring.
export function classifySearchInput(raw:string):SearchInput{
 const query=raw.trim();if(!query)return {kind:'empty',query:'',ids:[]};
 const tokens=query.split(/[\s,，;；、]+/u).filter(Boolean);
 if(tokens.length && tokens.every(token=>/^C\d{2,12}$/i.test(token)))return {kind:'ids',query,ids:[...new Set(tokens.map(token=>token.toUpperCase()))]};
 return {kind:'keyword',query,ids:[]};
}

export function reconcileSearchSelection(selected:string[],pageIds:string[],addedIds:string[]){
 const allowed=new Set(pageIds.map(value=>value.toUpperCase()));const added=new Set(addedIds.map(value=>value.toUpperCase()));
 return [...new Set(selected.map(value=>value.toUpperCase()))].filter(value=>allowed.has(value) && !added.has(value));
}
export function toggleSearchSelection(selected:string[],id:string,pageIds:string[],addedIds:string[]){
 const normalized=id.toUpperCase();const current=reconcileSearchSelection(selected,pageIds,addedIds);
 return reconcileSearchSelection(current.includes(normalized)?current.filter(value=>value!==normalized):[...current,normalized],pageIds,addedIds);
}

export interface LatestCallbacks<T>{start?:()=>void;success:(value:T)=>void;error:(error:unknown)=>void;settled?:()=>void}
export function createLatestRequestRunner<T>(){
 let generation=0;let controller:AbortController|undefined;
 function cancel(){generation++;controller?.abort();controller=undefined;}
 async function run(operation:(signal:AbortSignal)=>Promise<T>,callbacks:LatestCallbacks<T>){
  cancel();const current=generation;const request=new AbortController();controller=request;callbacks.start?.();
  try{const value=await operation(request.signal);if(current===generation && !request.signal.aborted)callbacks.success(value);}
  catch(error){if(current===generation && !request.signal.aborted)callbacks.error(error);}
  finally{if(current===generation){controller=undefined;callbacks.settled?.();}}
 }
 return {run,cancel};
}

// A search session uses the backend's single ranked snapshot. Cached pages are
// discarded only for a new explicit search or when its dialog is closed.
export function createSearchPageCache<T>(){
 let query='';const pages=new Map<number,T>();
 return {
  begin(value:string){query=value;pages.clear();},
  get(value:string,page:number){return value===query?pages.get(page):undefined;},
  put(value:string,page:number,result:T){if(value===query)pages.set(page,result);},
  clear(){query='';pages.clear();},
 };
}
