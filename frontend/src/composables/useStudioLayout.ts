import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { clampLayout, decodeLayoutStorage, encodeLayoutStorage, LAYOUT_STORAGE_KEY, moveDivider } from '../layout';
import type { DividerKind, LayoutGeometry, LayoutValues } from '../layout';

export function useStudioLayout() {
  const workspaceRef=ref<HTMLElement|null>(null),leftRef=ref<HTMLElement|null>(null);
  const centerRef=ref<HTMLElement|null>(null),previewRef=ref<HTMLElement|null>(null);
  const initialWidth=typeof window==='undefined'?1280:window.innerWidth;
  const initialHeight=typeof window==='undefined'?800:window.innerHeight;
  const geometry=ref<LayoutGeometry>({viewportWidth:initialWidth,viewportHeight:initialHeight,workspaceWidth:initialWidth-16,leftHeight:initialHeight-100,centerHeight:initialHeight-100,previewWidth:initialWidth-680});
  const preferences=ref<Partial<LayoutValues>>({});
  const dragging=ref<DividerKind|null>(null);
  const values=computed(()=>clampLayout(preferences.value,geometry.value,dragging.value ?? undefined));
  const styleVars=computed<Record<string,string>>(()=>({
    '--layout-left':`${values.value.left}px`,'--layout-right':`${values.value.right}px`,
    '--layout-queue':`${values.value.queue}px`,'--layout-tasks':`${values.value.tasks}px`,
    '--layout-canvas':`${values.value.canvas}%`,
  }));
  let observer:ResizeObserver|undefined;
  let drag:{kind:DividerKind;pointer:number;x:number;y:number;origin:LayoutValues;target:HTMLElement;cursor:string;selection:string}|undefined;

  function measure() {
    const numeric=(value:string)=>Number.parseFloat(value)||0;
    const read=(element:HTMLElement|null)=>{
      if(!element) return undefined;
      const style=getComputedStyle(element);
      return {width:Math.max(0,element.clientWidth-numeric(style.paddingLeft)-numeric(style.paddingRight)),height:Math.max(0,element.clientHeight-numeric(style.paddingTop)-numeric(style.paddingBottom)),columnGap:numeric(style.columnGap),rowGap:numeric(style.rowGap)};
    };
    const workspace=read(workspaceRef.value),left=read(leftRef.value),center=read(centerRef.value),preview=read(previewRef.value);
    geometry.value={...geometry.value,viewportWidth:window.innerWidth,viewportHeight:window.innerHeight,
      ...(workspace?{workspaceWidth:workspace.width,workspaceGap:workspace.columnGap}:{}),
      ...(left?{leftHeight:left.height,leftGap:left.rowGap}:{}),
      ...(center?{centerHeight:center.height,centerGap:center.rowGap}:{}),
      ...(preview?{previewWidth:preview.width,previewGap:preview.columnGap}:{}),
    };
  }
  function observe() {
    observer?.disconnect();
    for(const element of [workspaceRef.value,leftRef.value,centerRef.value,previewRef.value]) if(element) observer?.observe(element);
    measure();
  }
  function persist() {
    try { window.localStorage.setItem(LAYOUT_STORAGE_KEY,encodeLayoutStorage(preferences.value)); }
    catch { /* Layout changes still work when browser storage is unavailable. */ }
  }
  function stopDrag() {
    const current=drag;
    if(!current) return;
    drag=undefined;
    window.removeEventListener('pointermove',pointerMove);
    window.removeEventListener('pointerup',pointerEnd);
    window.removeEventListener('pointercancel',pointerEnd);
    window.removeEventListener('blur',stopDrag);
    current.target.removeEventListener('lostpointercapture',pointerEnd);
    document.documentElement.style.cursor=current.cursor;
    document.documentElement.style.userSelect=current.selection;
    try { if(current.target.hasPointerCapture(current.pointer)) current.target.releasePointerCapture(current.pointer); } catch { /* Detached element. */ }
    dragging.value=null;
    persist();
  }
  function pointerMove(event:PointerEvent) {
    if(!drag || event.pointerId!==drag.pointer) return;
    event.preventDefault();
    const delta=drag.kind==='queue' || drag.kind==='tasks'?event.clientY-drag.y:event.clientX-drag.x;
    const next=moveDivider(drag.origin,drag.kind,delta,geometry.value);
    preferences.value={...preferences.value,[drag.kind]:next[drag.kind]};
  }
  function pointerEnd(event:PointerEvent) { if(drag && event.pointerId===drag.pointer) stopDrag(); }
  function startDrag(kind:DividerKind,event:PointerEvent) {
    if(event.button!==0 || event.isPrimary===false || !(event.currentTarget instanceof HTMLElement)) return;
    stopDrag();measure();event.preventDefault();
    const target=event.currentTarget;
    drag={kind,pointer:event.pointerId,x:event.clientX,y:event.clientY,origin:{...values.value},target,cursor:document.documentElement.style.cursor,selection:document.documentElement.style.userSelect};
    dragging.value=kind;
    document.documentElement.style.cursor=kind==='queue' || kind==='tasks'?'row-resize':'col-resize';
    document.documentElement.style.userSelect='none';
    window.addEventListener('pointermove',pointerMove,{passive:false});
    window.addEventListener('pointerup',pointerEnd);
    window.addEventListener('pointercancel',pointerEnd);
    window.addEventListener('blur',stopDrag);
    target.addEventListener('lostpointercapture',pointerEnd);
    try { target.setPointerCapture(event.pointerId); } catch { /* Window listeners remain usable. */ }
  }
  function resetDivider(kind:DividerKind) {
    stopDrag();
    const next={...preferences.value};delete next[kind];preferences.value=next;persist();
  }
  function handleKey(kind:DividerKind,event:KeyboardEvent) {
    if(event.key==='Home' || event.key==='Enter') {event.preventDefault();resetDivider(kind);return;}
    const vertical=kind==='queue' || kind==='tasks';
    const minus=vertical?'ArrowUp':'ArrowLeft',plus=vertical?'ArrowDown':'ArrowRight';
    if(event.key!==minus && event.key!==plus) return;
    event.preventDefault();stopDrag();measure();
    const delta=(event.key===plus?1:-1)*(event.shiftKey?40:10);
    const next=moveDivider(values.value,kind,delta,geometry.value);
    preferences.value={...preferences.value,[kind]:next[kind]};persist();
  }
  onMounted(()=>{
    try { preferences.value=decodeLayoutStorage(window.localStorage.getItem(LAYOUT_STORAGE_KEY)); } catch { preferences.value={}; }
    observer=new ResizeObserver(measure);observe();window.addEventListener('resize',measure);
  });
  watch([workspaceRef,leftRef,centerRef,previewRef],()=>{if(observer) observe();},{flush:'post'});
  onBeforeUnmount(()=>{stopDrag();observer?.disconnect();window.removeEventListener('resize',measure);});
  return {workspaceRef,leftRef,centerRef,previewRef,styleVars,startDrag,handleKey,resetDivider,dragging,values};
}
