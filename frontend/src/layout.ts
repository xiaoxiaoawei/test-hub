export type DividerKind = 'left' | 'right' | 'queue' | 'tasks' | 'canvas';
export interface LayoutValues { left:number; right:number; queue:number; tasks:number; canvas:number }
export interface LayoutGeometry {
  viewportWidth:number; viewportHeight:number;
  workspaceWidth:number; leftHeight:number; centerHeight:number; previewWidth:number;
  workspaceGap?:number; leftGap?:number; centerGap?:number; previewGap?:number;
}
export const LAYOUT_STORAGE_KEY = 'package-studio.layout.v1';
export const DIVIDER_SIZE = 6;
const finite = (value:unknown):value is number => typeof value === 'number' && Number.isFinite(value);
const positive = (value:number, fallback=0) => finite(value) ? Math.max(0,value) : fallback;
const clamp = (value:number,min:number,max:number) => Math.min(Math.max(value,min),Math.max(min,max));
const kinds:DividerKind[] = ['left','right','queue','tasks','canvas'];

function verticalLimits(height:number,gap:number,first:number,second:number) {
  const available=Math.max(0,positive(height)-DIVIDER_SIZE-2*positive(gap));
  const scale=Math.min(1,available/(first+second));
  return {available,first:first*scale,second:second*scale};
}

export function defaultLayout(g:LayoutGeometry):LayoutValues {
  const width=positive(g.viewportWidth,1280),height=positive(g.viewportHeight,800);
  const available=Math.max(0,positive(g.leftHeight)-DIVIDER_SIZE-2*positive(g.leftGap ?? 0));
  return {
    left:width<=1120?250:width<=1270?270:300,
    right:width<=1120?286:width<=1270?298:326,
    queue:available*.574,
    tasks:height<=720?180:height<=800?199:225,
    canvas:55,
  };
}

// Jointly constrain both outer columns so neither can consume the preview.
// In a smaller-than-supported host, scale the minima together rather than
// returning negative tracks or pushing a separator outside its container.
export function clampLayout(input:Partial<LayoutValues>,g:LayoutGeometry,active?:DividerKind):LayoutValues {
  const fallback=defaultLayout(g);
  const v={...fallback};
  for(const kind of kinds) if(finite(input[kind])) v[kind]=input[kind];
  const width=Math.max(0,positive(g.workspaceWidth)-2*DIVIDER_SIZE-4*positive(g.workspaceGap ?? 0));
  const scale=Math.min(1,width/(240+360+276));
  const minLeft=240*scale,minRight=276*scale,budget=width-360*scale;
  v.left=clamp(v.left,minLeft,budget-minRight);
  v.right=clamp(v.right,minRight,budget-minLeft);
  if(active==='left') v.left=clamp(v.left,minLeft,budget-v.right);
  else if(active==='right') v.right=clamp(v.right,minRight,budget-v.left);
  else if(v.left+v.right>budget) {
    const extra=v.left+v.right-minLeft-minRight;
    const room=Math.max(0,budget-minLeft-minRight);
    v.left=minLeft+(extra>0?(v.left-minLeft)*room/extra:0);
    v.right=budget-v.left;
  }
  const left=verticalLimits(g.leftHeight,g.leftGap ?? 0,220,140);
  v.queue=clamp(v.queue,left.first,left.available-left.second);
  const center=verticalLimits(g.centerHeight,g.centerGap ?? 0,220,130);
  v.tasks=clamp(v.tasks,center.second,center.available-center.first);
  const preview=positive(g.previewWidth);
  if(preview>0) {
    const available=Math.max(0,preview-DIVIDER_SIZE-2*positive(g.previewGap ?? 0));
    const min=Math.min(180,available/2);
    v.canvas=clamp(v.canvas,min/preview*100,(available-min)/preview*100);
  } else v.canvas=55;
  return v;
}

export function moveDivider(origin:LayoutValues,kind:DividerKind,delta:number,g:LayoutGeometry):LayoutValues {
  if(!finite(delta)) return clampLayout(origin,g,kind);
  const next={...origin};
  if(kind==='right' || kind==='tasks') next[kind]-=delta;
  else if(kind==='canvas') next.canvas+=positive(g.previewWidth)>0?delta/g.previewWidth*100:0;
  else next[kind]+=delta;
  return clampLayout(next,g,kind);
}

export function decodeLayoutStorage(raw:string|null):Partial<LayoutValues> {
  if(!raw || raw.length>4096) return {};
  try {
    const data:unknown=JSON.parse(raw);
    if(!data || typeof data!=='object' || Array.isArray(data)) return {};
    const record=data as {version?:unknown;values?:unknown};
    if(record.version!==1 || !record.values || typeof record.values!=='object' || Array.isArray(record.values)) return {};
    const values=record.values as Record<string,unknown>,result:Partial<LayoutValues>={};
    for(const kind of kinds) {
      const value=values[kind];
      if(finite(value) && value>=0 && value<=(kind==='canvas'?100:10000)) result[kind]=value;
    }
    return result;
  } catch { return {}; }
}

export function encodeLayoutStorage(values:Partial<LayoutValues>):string {
  return JSON.stringify({version:1,values:decodeLayoutStorage(JSON.stringify({version:1,values}))});
}
