<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useId, watch } from 'vue';
import type { Part, Pin, Point } from '../types';
import Icon from './Icon.vue';
const props=defineProps<{part:Part;mode:'footprint'|'symbol';grid:boolean;showLabels:boolean;showSilk:boolean;showCopper:boolean}>();
const host=ref<HTMLElement>(); const svg=ref<SVGSVGElement>(); const width=ref(600); const height=ref(480); const zoom=ref(1); const pan=ref({x:0,y:0}); const dragging=ref(false);
let observer:ResizeObserver; let last={x:0,y:0};
const viewportID=useId().replace(/[^a-zA-Z0-9_-]/g,'');
const smallGridID=`grid-small-${viewportID}`;const largeGridID=`grid-large-${viewportID}`;
const compact=computed(()=>width.value<500);
const isSymbol=computed(()=>props.mode==='symbol');
const unit=ref(1);
const visibleUnit=(v:{unit?:number})=>!v.unit || v.unit===unit.value;
const activeSymbol=computed(()=>({
 pins:(props.part.symbol.pins || []).filter(visibleUnit),
 lines:(props.part.symbol.lines || []).filter(visibleUnit),
 rects:(props.part.symbol.rects || []).filter(visibleUnit),
 circles:(props.part.symbol.circles || []).filter(visibleUnit),
 polygons:(props.part.symbol.polygons || []).filter(visibleUnit),
 texts:props.part.symbol.texts || [],
}));
const bounds=computed(()=>{
 const points:Point[]=[]; const f=props.part.footprint; const s=activeSymbol.value;
 if(isSymbol.value){
  for(const p of s.pins || []){points.push(p,pinEnd(p));}
  for(const l of s.lines || [])points.push({x:l.x1,y:l.y1},{x:l.x2,y:l.y2});
  for(const r of s.rects || [])points.push(r,{x:r.x+r.width,y:r.y+r.height});
  for(const c of s.circles || [])points.push({x:c.x-c.radius,y:c.y-c.radius},{x:c.x+c.radius,y:c.y+c.radius});
  for(const p of s.polygons || [])points.push(...p.points);
 } else {
  for(const p of f.pads || []){
   if(p.shape==='custom' && p.points?.length){points.push(...p.points);continue;}
   const a=(p.rotation || 0)*Math.PI/180;const rx=(Math.abs(Math.cos(a))*p.width+Math.abs(Math.sin(a))*p.height)/2;const ry=(Math.abs(Math.sin(a))*p.width+Math.abs(Math.cos(a))*p.height)/2;
   points.push({x:p.x-rx,y:p.y-ry},{x:p.x+rx,y:p.y+ry});
  }
  for(const l of f.lines || [])points.push({x:l.x1,y:l.y1},{x:l.x2,y:l.y2});
  for(const c of f.circles || [])points.push({x:c.x-c.radius,y:c.y-c.radius},{x:c.x+c.radius,y:c.y+c.radius});
  for(const p of f.polygons || [])points.push(...p.points);
 }
 if(!points.length)return {x:-5,y:-5,w:10,h:10};
 let x=Infinity,y=Infinity,right=-Infinity,bottom=-Infinity;
 for(const p of points){if(!Number.isFinite(p.x) || !Number.isFinite(p.y))continue;x=Math.min(x,p.x);y=Math.min(y,p.y);right=Math.max(right,p.x);bottom=Math.max(bottom,p.y);}
 if(!Number.isFinite(x))return {x:-5,y:-5,w:10,h:10};
 return {x,y,w:Math.max(.1,right-x),h:Math.max(.1,bottom-y)};
});
// Reserve screen space for controls, rather than adding a fixed number of mm.
// The limiting dimension of the drawing occupies about 75–80% of its viewport.
const fitArea=computed(()=>{
 const w=Math.max(1,width.value),h=Math.max(1,height.value);
 const side=Math.min(52,Math.max(12,w*.1));const top=Math.min(42,Math.max(18,h*.08));const bottom=Math.min(compact.value?78:58,Math.max(24,h*(compact.value?.18:.13)));
 return {width:Math.max(1,w-2*side),height:Math.max(1,h-top-bottom),centerX:w/2,centerY:(h+top-bottom)/2};
});
const scale=computed(()=>Math.max(.0001,Math.min(fitArea.value.width/bounds.value.w,fitArea.value.height/bounds.value.h)*.96)*zoom.value);
const cx=computed(()=>bounds.value.x+bounds.value.w/2);const cy=computed(()=>bounds.value.y+bounds.value.h/2);
const transform=computed(()=>`translate(${fitArea.value.centerX+pan.value.x} ${fitArea.value.centerY+pan.value.y}) scale(${scale.value}) translate(${-cx.value} ${-cy.value})`);
const gridStep=computed(()=>{const wanted=18/scale.value;const power=10**Math.floor(Math.log10(wanted));return ([1,2,5,10].find(v=>v*power>=wanted) || 10)*power;});
const gridLabel=computed(()=>`${Number(gridStep.value.toPrecision(3))} mm`);
const worldView=computed(()=>({x:cx.value-(fitArea.value.centerX+pan.value.x)/scale.value,y:cy.value-(fitArea.value.centerY+pan.value.y)/scale.value,width:Math.max(1,width.value)/scale.value,height:Math.max(1,height.value)/scale.value}));
const pinOne=computed(()=>{
 if(isSymbol.value || !props.showCopper)return null;
 const p=props.part.footprint.pads.find(v=>v.number==='1' || v.number==='A1');if(!p)return null;
 const dx=p.x-cx.value,dy=p.y-cy.value;const distance=Math.hypot(dx,dy);const ux=distance>.001?dx/distance:0,uy=distance>.001?dy/distance:-1;const offset=Math.hypot(p.width,p.height)/2+8/scale.value;
 return {x:p.x+ux*offset,y:p.y+uy*offset,number:p.number};
});
const padLabelLayout=computed(()=>{
 const pads=props.part.footprint.pads.filter(p=>p.number?.trim());
 if(isSymbol.value || !props.showLabels || !pads.length)return {hidden:false,size:10};
 const smallest=Math.min(...pads.map(p=>Math.min(p.width,p.height)));
 const size=Math.min(14,Math.max(10,smallest*.48*scale.value));
 const candidates=pads.map(p=>{
  const a=(p.shape==='custom'?0:p.rotation || 0)*Math.PI/180;
  const padWidth=(Math.abs(Math.cos(a))*p.width+Math.abs(Math.sin(a))*p.height)*scale.value;
  const padHeight=(Math.abs(Math.sin(a))*p.width+Math.abs(Math.cos(a))*p.height)*scale.value;
  return {x:p.x*scale.value,y:p.y*scale.value,width:p.number.length*size*.62+2,height:size*1.1+1,padWidth,padHeight};
 });
 // At fit scale a fine-pitch package can have less space between pads than a
 // two-digit glyph needs. Keep its copper outline clear and reveal numbers
 // after zooming, rather than shrinking them into unreadable white blocks.
 if(candidates.some(p=>p.width>p.padWidth || p.height>p.padHeight))return {hidden:true,size};
 const cell=Math.max(16,...candidates.map(p=>Math.max(p.width,p.height)));
 const buckets=new Map<string,typeof candidates>();
 for(const p of candidates){
  const bx=Math.floor(p.x/cell),by=Math.floor(p.y/cell);
  for(let dx=-1;dx<=1;dx++)for(let dy=-1;dy<=1;dy++){
   for(const other of buckets.get(`${bx+dx}:${by+dy}`) || []){
    if(Math.abs(p.x-other.x)<(p.width+other.width)/2 && Math.abs(p.y-other.y)<(p.height+other.height)/2)return {hidden:true,size};
   }
  }
  const key=`${bx}:${by}`;const bucket=buckets.get(key) || [];bucket.push(p);buckets.set(key,bucket);
 }
 return {hidden:false,size};
});
const copperLayer=(layer?:string)=>!layer || /cu|copper|^1$|^2$|pad/i.test(layer);
const lineVisible=(layer?:string)=>copperLayer(layer)?props.showCopper:props.showSilk;
function pinEnd(p:Pin){const a=(p.rotation||0)*Math.PI/180;return {x:p.x+Math.cos(a)*(p.length??2.54),y:p.y+Math.sin(a)*(p.length??2.54)};}
function pinName(p:Pin){const e=pinEnd(p);const a=(p.rotation||0)*Math.PI/180;return {x:e.x+Math.cos(a)*.6,y:e.y+Math.sin(a)*.6,anchor:Math.cos(a)<-.1?'end':Math.cos(a)>.1?'start':'middle'};}
function labelSize(nominal:number){return Math.min(16/scale.value,Math.max(11/scale.value,nominal));}
function fit(){zoom.value=1;pan.value={x:0,y:0};dragging.value=false;}
defineExpose({fit});
function changeZoom(multiplier:number){zoom.value=Math.min(8,Math.max(.18,zoom.value*multiplier));}
function wheel(e:WheelEvent){e.preventDefault();if(!svg.value)return;const old=zoom.value;const next=Math.min(8,Math.max(.18,old*Math.exp(-e.deltaY*.0012)));const rect=svg.value.getBoundingClientRect();const x=e.clientX-rect.left-fitArea.value.centerX;const y=e.clientY-rect.top-fitArea.value.centerY;pan.value={x:x-(x-pan.value.x)*next/old,y:y-(y-pan.value.y)*next/old};zoom.value=next;}
function down(e:PointerEvent){if(e.button!==0 && e.button!==1)return;e.preventDefault();dragging.value=true;last={x:e.clientX,y:e.clientY};svg.value?.setPointerCapture(e.pointerId);}
function move(e:PointerEvent){if(!dragging.value)return;pan.value={x:pan.value.x+e.clientX-last.x,y:pan.value.y+e.clientY-last.y};last={x:e.clientX,y:e.clientY};}
function up(e?:PointerEvent){dragging.value=false;if(e && svg.value?.hasPointerCapture(e.pointerId))svg.value.releasePointerCapture(e.pointerId);}
watch(()=>props.part.id,()=>{unit.value=1;fit();});
watch(()=>[props.part,props.mode,unit.value],fit);
onMounted(()=>{observer=new ResizeObserver(([e])=>{if(e){width.value=Math.max(1,e.contentRect.width);height.value=Math.max(1,e.contentRect.height);}});if(host.value)observer.observe(host.value);});
onBeforeUnmount(()=>observer?.disconnect());
</script>
<template>
 <div class="geometry-viewport" :class="{compact}" ref="host">
  <svg ref="svg" :viewBox="`0 0 ${width} ${height}`" :class="{dragging}" role="img" :aria-label="`${part.name} ${isSymbol?'原理图符号':'PCB 封装'}预览`" @wheel="wheel" @pointerdown="down" @pointermove="move" @pointerup="up" @pointercancel="up" @lostpointercapture="up" @dblclick="fit">
   <defs><pattern :id="smallGridID" :width="gridStep" :height="gridStep" patternUnits="userSpaceOnUse"><path :d="`M${gridStep} 0H0V${gridStep}`" fill="none" stroke="var(--grid)" :stroke-width=".6/scale"/></pattern><pattern :id="largeGridID" :width="gridStep*5" :height="gridStep*5" patternUnits="userSpaceOnUse"><rect :width="gridStep*5" :height="gridStep*5" :fill="`url(#${smallGridID})`"/><path :d="`M${gridStep*5} 0H0V${gridStep*5}`" fill="none" stroke="var(--grid-major)" :stroke-width=".9/scale"/></pattern></defs>
   <g :transform="transform">
    <rect v-if="grid" :x="worldView.x" :y="worldView.y" :width="worldView.width" :height="worldView.height" :fill="`url(#${largeGridID})`"/>
    <g class="origin-mark" :stroke-width="1/scale" stroke="var(--origin)"><path d="M-1 0H1M0-1V1"/><circle r=".18" fill="none"/></g>
    <g v-if="isSymbol" class="symbol-drawing">
     <rect v-for="(r,i) in activeSymbol.rects" :key="`r${i}`" :x="r.x" :y="r.y" :width="Math.max(.01,r.width)" :height="Math.max(.01,r.height)" :fill="r.fill?'var(--symbol-fill)':'none'" stroke="var(--symbol-line)" stroke-width=".22"/>
     <line v-for="(l,i) in activeSymbol.lines" :key="`l${i}`" :x1="l.x1" :y1="l.y1" :x2="l.x2" :y2="l.y2" stroke="var(--symbol-line)" :stroke-width="l.width || .2"/>
     <circle v-for="(c,i) in activeSymbol.circles" :key="`c${i}`" :cx="c.x" :cy="c.y" :r="c.radius" fill="none" stroke="var(--symbol-line)" :stroke-width="c.width || .2"/>
     <polygon v-for="(p,i) in activeSymbol.polygons" :key="`g${i}`" :points="p.points.map(v=>`${v.x},${v.y}`).join(' ')" :fill="p.filled||p.fill?'var(--symbol-fill)':'none'" stroke="var(--symbol-line)" :stroke-width="p.width || .2"/>
     <g v-for="(p,i) in activeSymbol.pins" :key="`p${i}`" :opacity="p.hidden?.45:1">
      <line :x1="p.x" :y1="p.y" :x2="pinEnd(p).x" :y2="pinEnd(p).y" stroke="var(--pin)" stroke-width=".18"/>
      <circle :cx="p.x" :cy="p.y" r=".17" fill="var(--viewport)" stroke="var(--pin)" stroke-width=".13"/>
      <text v-if="showLabels" :x="(p.x+pinEnd(p).x)/2" :y="(p.y+pinEnd(p).y)/2-.38" text-anchor="middle" :font-size="labelSize(.8)" fill="var(--pin)">{{p.number}}</text>
      <text v-if="showLabels" :x="pinName(p).x" :y="pinName(p).y+.27" :text-anchor="pinName(p).anchor" :font-size="labelSize(.85)" fill="var(--symbol-line)">{{p.name}}</text>
     </g>
     <text v-for="(t,i) in activeSymbol.texts" :key="`t${i}`" :x="t.x" :y="t.y" :font-size="t.size || 1" fill="var(--symbol-line)">{{t.text}}</text>
    </g>
    <g v-else>
     <g>
      <polygon v-for="(p,i) in part.footprint.polygons || []" v-show="lineVisible(p.layer)" :key="`pg${i}`" :points="p.points.map(v=>`${v.x},${v.y}`).join(' ')" :fill="p.filled||p.fill?'var(--silk)':'none'" stroke="var(--silk)" :stroke-width="p.width || .14"/>
      <line v-for="(l,i) in part.footprint.lines || []" v-show="lineVisible(l.layer)" :key="`l${i}`" :x1="l.x1" :y1="l.y1" :x2="l.x2" :y2="l.y2" :stroke="copperLayer(l.layer)?'var(--copper)':'var(--silk)'" :stroke-width="l.width || .15" stroke-linecap="round"/>
      <circle v-for="(c,i) in part.footprint.circles || []" v-show="lineVisible(c.layer)" :key="`c${i}`" :cx="c.x" :cy="c.y" :r="c.radius" fill="none" stroke="var(--silk)" :stroke-width="c.width || .15"/>
     </g>
     <g v-if="showCopper">
      <g v-for="(p,i) in part.footprint.pads || []" :key="`p${i}`" :transform="`translate(${p.x} ${p.y}) rotate(${p.shape==='custom'?0:p.rotation || 0})`">
       <title>焊盘 {{p.number || '安装孔'}} · {{p.width.toFixed(3)}} × {{p.height.toFixed(3)}} mm</title>
       <ellipse v-if="p.shape==='circle' || p.shape==='ellipse'" :rx="p.width/2" :ry="p.height/2" fill="var(--copper)" stroke="var(--copper-edge)" stroke-width=".045"/>
       <polygon v-else-if="p.shape==='custom' && p.points?.length" :points="p.points.map(v=>`${v.x-p.x},${v.y-p.y}`).join(' ')" fill="var(--copper)"/>
       <rect v-else :x="-p.width/2" :y="-p.height/2" :width="p.width" :height="p.height" :rx="p.shape==='oval'?Math.min(p.width,p.height)/2:p.shape==='roundrect'?Math.min(p.width,p.height)*.18:.03" fill="var(--copper)" stroke="var(--copper-edge)" stroke-width=".045"/>
       <ellipse v-if="p.drill" :rx="(p.drillWidth || p.drill)/2" :ry="p.drill/2" fill="var(--viewport)" stroke="var(--drill)" stroke-width=".06"/>
       <text v-if="showLabels && !padLabelLayout.hidden" x="0" dominant-baseline="central" text-anchor="middle" :font-size="padLabelLayout.size/scale" font-weight="600" fill="var(--pad-label)" :transform="`rotate(${p.shape==='custom'?0:-1*(p.rotation || 0)})`">{{p.number}}</text>
      </g>
     </g>
     <text v-for="(t,i) in part.footprint.texts || []" v-show="showLabels && showSilk" :key="`t${i}`" :x="t.x" :y="t.y" :font-size="t.size || 1" text-anchor="middle" fill="var(--silk)" :transform="`rotate(${t.rotation || 0} ${t.x} ${t.y})`">{{t.text}}</text>
     <circle v-if="pinOne" :cx="pinOne.x" :cy="pinOne.y" :r="3.5/scale" fill="var(--primary)" stroke="var(--viewport)" :stroke-width="1.5/scale"><title>{{pinOne.number}} 号焊盘参考位置</title></circle>
    </g>
   </g>
  </svg>
  <div v-if="isSymbol && (part.symbol.units || 1)>1" class="symbol-unit-select"><label :for="`symbol-unit-${viewportID}`">原理图单元</label><select :id="`symbol-unit-${viewportID}`" v-model="unit"><option v-for="i in part.symbol.units" :key="i" :value="i">Unit {{i}}</option></select></div>
  <span v-if="padLabelLayout.hidden" class="viewport-label-hint"><Icon name="zoomIn" :size="13"/>放大查看焊盘编号</span>
  <div class="viewport-coordinate"><span class="axis-line x"></span>X<span class="axis-line y"></span>Y<span class="mono">{{bounds.w.toFixed(2)}} × {{bounds.h.toFixed(2)}} mm</span><span v-if="grid" class="grid-size">网格 {{gridLabel}}</span></div>
  <div class="viewport-controls"><button class="icon-button" title="缩小" aria-label="缩小" @click="changeZoom(.8)"><Icon name="zoomOut" :size="17"/></button><span class="mono">{{Math.round(zoom*100)}}%</span><button class="icon-button" title="放大" aria-label="放大" @click="changeZoom(1.25)"><Icon name="zoomIn" :size="17"/></button><i></i><button class="icon-button" title="适应视图（双击）" aria-label="适应视图" @click="fit"><Icon name="fit" :size="17"/></button></div>
  <span class="viewport-hint">拖动平移 · 滚轮缩放 · 双击复位</span>
 </div>
</template>

<style scoped>
.geometry-viewport .viewport-controls{bottom:12px;right:12px;padding:3px;gap:4px}
.geometry-viewport .viewport-controls .icon-button{width:32px;height:32px}
.geometry-viewport .viewport-controls .mono{font-size:12px;width:44px}
.geometry-viewport .viewport-coordinate{left:14px;bottom:23px;font-size:11px;gap:5px;color:var(--secondary)}
.geometry-viewport .viewport-coordinate>.mono{font-size:11px;margin-left:7px}
.geometry-viewport .grid-size{font-size:11px;margin-left:9px;color:var(--muted)}
.geometry-viewport .viewport-hint{font-size:11px;right:14px;top:14px;line-height:1.5}
.geometry-viewport .viewport-label-hint{position:absolute;left:12px;top:12px;display:flex;align-items:center;gap:5px;font-size:11px;color:var(--secondary);background:var(--surface-raised);border:1px solid var(--border);border-radius:5px;padding:6px 8px;pointer-events:none}
.geometry-viewport .symbol-unit-select{font-size:12px;left:12px;top:10px;gap:8px;padding:5px 9px}
.geometry-viewport .symbol-unit-select select{font-size:12px;min-height:25px;padding:2px 5px}
.geometry-viewport.compact .viewport-controls{bottom:40px}
.geometry-viewport.compact .viewport-coordinate{bottom:13px;left:12px;right:12px;gap:4px;white-space:nowrap}
.geometry-viewport.compact .grid-size{margin-left:auto;font-size:10px}
.geometry-viewport.compact .viewport-hint{display:none}
.geometry-viewport text{paint-order:stroke fill;stroke-width:0;letter-spacing:0}
</style>
