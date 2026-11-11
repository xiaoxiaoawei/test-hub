import { onBeforeUnmount, ref } from 'vue';
import { createLatestRequestRunner, createSearchPageCache } from '../search';
import type { PartSearchResult } from '../types';

export function usePartSearch(fetcher:(query:string,page:number,signal:AbortSignal)=>Promise<PartSearchResult>){
 const visible=ref(false),loading=ref(false),error=ref(''),query=ref(''),page=ref(1);const result=ref<PartSearchResult>();
 const runner=createLatestRequestRunner<PartSearchResult>();
 const cache=createSearchPageCache<PartSearchResult>();
 async function request(value:string,requestedPage:number,refresh:boolean){
  const text=value.trim();if(!text)return;
  const nextPage=Math.max(1,Math.floor(requestedPage));
  if(loading.value && query.value===text && page.value===nextPage)return;
  if(refresh || query.value!==text)cache.begin(text);
  query.value=text;page.value=nextPage;visible.value=true;
  const cached=refresh?undefined:cache.get(text,nextPage);
  if(cached){runner.cancel();loading.value=false;error.value='';result.value=cached;return;}
  await runner.run(signal=>fetcher(text,nextPage,signal),{
   start(){loading.value=true;error.value='';result.value=undefined;},
   success(value){const normalized={...value,items:Array.isArray(value.items)?value.items:[]};cache.put(text,nextPage,normalized);result.value=normalized;},
   error(value){error.value=value instanceof Error?value.message:String(value);},
   settled(){loading.value=false;},
  });
 }
 function run(value:string,requestedPage=1){return request(value,requestedPage,true);}
 function goPage(requestedPage:number){return request(query.value,requestedPage,false);}
 function close(){runner.cancel();cache.clear();loading.value=false;visible.value=false;error.value='';}
 onBeforeUnmount(close);
 return {visible,loading,error,query,page,result,run,goPage,close};
}
