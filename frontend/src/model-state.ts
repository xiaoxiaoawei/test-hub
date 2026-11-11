// All model sources share this guard: a new user intent owns the preview.
export function createModelLoadGuard(){
 let generation=0;
 return {begin:()=>++generation,isCurrent:(token:number)=>token===generation,invalidate:()=>{generation++;}};
}

// GLB can carry its own textures and buffers. Never let a local file silently
// fetch resources or load optional external decoders from the network.
export function validateLocalGLB(buffer:ArrayBuffer){
 if(buffer.byteLength>64*1024*1024)throw new Error('GLB 模型超过 64 MB，暂不载入。');
 if(buffer.byteLength<20)throw new Error('GLB 文件不完整。');
 const view=new DataView(buffer);
 if(view.getUint32(0,true)!==0x46546c67 || view.getUint32(4,true)!==2 || view.getUint32(8,true)!==buffer.byteLength)throw new Error('只支持完整的 GLB 2.0 文件。');
 let json:unknown;let offset=12;
 while(offset+8<=buffer.byteLength){
  const size=view.getUint32(offset,true),type=view.getUint32(offset+4,true);offset+=8;
  if(size>buffer.byteLength-offset)throw new Error('GLB 数据块长度无效。');
  if(type===0x4e4f534a){if(json!==undefined)throw new Error('GLB 包含重复的 JSON 块。');json=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(new Uint8Array(buffer,offset,size)).replace(/\0+$/,''));}
  offset+=size;
 }
 if(offset!==buffer.byteLength || !json || typeof json!=='object')throw new Error('GLB 未提供有效场景。');
 const data=json as {extensionsRequired?:unknown;extensionsUsed?:unknown};
 const extensions=[...(Array.isArray(data.extensionsRequired)?data.extensionsRequired:[]),...(Array.isArray(data.extensionsUsed)?data.extensionsUsed:[])];
 if(extensions.some(value=>['KHR_draco_mesh_compression','EXT_meshopt_compression','KHR_texture_basisu'].includes(value)))throw new Error('此 GLB 需要额外压缩解码器，请导出未压缩 GLB 或 OBJ。');
 const inspect=(value:unknown)=>{
  if(!value || typeof value!=='object')return;
  for(const [key,item] of Object.entries(value)){
   if(key==='uri' && typeof item==='string' && !item.startsWith('data:'))throw new Error('只支持内嵌资源的 GLB，不加载外部纹理或缓冲区。');
   if(item && typeof item==='object')inspect(item);
  }
 };
 inspect(json);
}
