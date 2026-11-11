export interface DecodedPartList {text:string;encoding:string}
export function decodePartList(bytes:Uint8Array):DecodedPartList {
 let encoding='utf-8';let content=bytes;
 if(bytes[0]===0xef && bytes[1]===0xbb && bytes[2]===0xbf)content=bytes.subarray(3);
 else if(bytes[0]===0xff && bytes[1]===0xfe){encoding='utf-16le';content=bytes.subarray(2);}
 else if(bytes[0]===0xfe && bytes[1]===0xff){encoding='utf-16be';content=bytes.subarray(2);}
 else if(bytes.length>=4){
  let even=0,odd=0;const length=Math.min(bytes.length,256);
  for(let i=0;i<length;i++)if(bytes[i]===0){if(i%2)odd++;else even++;}
  if(odd>length/5 && even<length/20)encoding='utf-16le';
  else if(even>length/5 && odd<length/20)encoding='utf-16be';
 }
 try{return {text:new TextDecoder(encoding,{fatal:true}).decode(content),encoding};}
 catch(error){
  if(encoding!=='utf-8')throw new Error(`无法按 ${encoding.toUpperCase()} 读取清单，请检查文件编码。`,{cause:error});
  try{return {text:new TextDecoder('gb18030',{fatal:true}).decode(content),encoding:'gb18030'};}
  catch(cause){throw new Error('无法读取清单编码，请另存为 UTF-8、UTF-16 或 GB18030。',{cause});}
 }
}
export function partListBatches(ids:string[],size=50){
 if(!Number.isInteger(size) || size<1 || size>50)throw new Error('每批最多 50 个器件。');
 const normalized=[...new Set(ids.map(id=>id.trim().toUpperCase()))];
 if(normalized.length>500)throw new Error('清单最多添加 500 个器件。');
 if(normalized.some(id=>!/^C\d{2,12}$/.test(id)))throw new Error('清单包含无效 C 编号，请重新解析。');
 const batches:string[][]=[];for(let i=0;i<normalized.length;i+=size)batches.push(normalized.slice(i,i+size));return batches;
}
