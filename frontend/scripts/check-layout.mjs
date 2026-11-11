import assert from 'node:assert/strict';
import { clampLayout, decodeLayoutStorage, defaultLayout, DIVIDER_SIZE, encodeLayoutStorage, moveDivider } from '../src/layout.ts';

const geometry={viewportWidth:1440,viewportHeight:900,workspaceWidth:1424,leftHeight:760,centerHeight:760,previewWidth:700,workspaceGap:0,leftGap:0,centerGap:0,previewGap:0};
const initial=clampLayout({},geometry);
assert.equal(initial.left,300);assert.equal(initial.right,326);assert.equal(initial.tasks,225);assert.equal(initial.canvas,55);
assert.equal(defaultLayout({...geometry,viewportWidth:1100,viewportHeight:700}).left,250);
assert.equal(defaultLayout({...geometry,viewportWidth:1200,viewportHeight:780}).right,298);
assert.equal(defaultLayout({...geometry,viewportHeight:700}).tasks,180);
assert.equal(defaultLayout({...geometry,viewportHeight:780}).tasks,199);

const moved=moveDivider(initial,'left',10000,geometry);
assert.equal(moved.right,initial.right);assert.ok(moved.left+360+moved.right+2*DIVIDER_SIZE<=geometry.workspaceWidth+.001);
assert.equal(moveDivider(initial,'right',20,geometry).right,initial.right-20);
assert.equal(moveDivider(initial,'queue',20,geometry).queue,initial.queue+20);
assert.equal(moveDivider(initial,'tasks',20,geometry).tasks,initial.tasks-20);
assert.ok(Math.abs(moveDivider(initial,'canvas',70,geometry).canvas-65)<.00001);
assert.deepEqual(moveDivider(initial,'canvas',NaN,geometry),initial);

for(const width of [0,12,100,500,900,1040,1600]) for(const height of [0,6,100,300,800]) {
  const g={...geometry,workspaceWidth:width,leftHeight:height,centerHeight:height,previewWidth:width,workspaceGap:8,leftGap:8,centerGap:8,previewGap:1};
  const v=clampLayout({left:9999,right:9999,queue:9999,tasks:9999,canvas:100},g);
  assert.ok(Object.values(v).every(Number.isFinite));
  assert.ok(Object.values(v).every(n=>n>=0));
  assert.ok(v.left+v.right<=Math.max(0,width-2*DIVIDER_SIZE-32)+.001);
  assert.ok(v.queue<=Math.max(0,height-DIVIDER_SIZE-16)+.001);
  assert.ok(v.tasks<=Math.max(0,height-DIVIDER_SIZE-16)+.001);
  if(width>0) assert.ok(v.canvas/100*width<=Math.max(0,width-DIVIDER_SIZE-2)+.001);
}
const shrunk=clampLayout({left:700,right:700,queue:700,tasks:700,canvas:95},{...geometry,workspaceWidth:1024,leftHeight:500,centerHeight:500,previewWidth:400});
assert.ok(shrunk.left>=240 && shrunk.right>=276 && shrunk.left+shrunk.right<=1024-12-360+.001);
assert.ok(shrunk.queue>=220 && shrunk.queue<=354);assert.ok(shrunk.tasks>=130 && shrunk.tasks<=274);
assert.ok(shrunk.canvas<=53.5);

for(const raw of [null,'','invalid','null','[]','42','{"version":2,"values":{"left":400}}','{"version":1,"values":[]}','x'.repeat(5000)]) assert.deepEqual(decodeLayoutStorage(raw),{});
assert.deepEqual(decodeLayoutStorage('{"version":1,"values":{"left":400,"right":"500","queue":-1,"tasks":1e99,"canvas":101,"unknown":17}}'),{left:400});
assert.deepEqual(decodeLayoutStorage(encodeLayoutStorage({left:400,canvas:60})),{left:400,canvas:60});
assert.deepEqual(clampLayout({left:NaN,right:Infinity},geometry),initial);
console.log('Layout constraints, resize, drag direction and persisted data checks passed.');
