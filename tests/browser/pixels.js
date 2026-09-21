import { inflateSync } from 'node:zlib';
import { expect } from '@playwright/test';

// Inspect actual Chromium PNG pixels, not a DOM/screenshot size proxy.
export function expectPainted(png) {
  let width=0,height=0,channels=0;const chunks=[];
  for(let off=8;off<png.length;){
    const n=png.readUInt32BE(off),name=png.toString('ascii',off+4,off+8),data=png.subarray(off+8,off+8+n);
    if(name==='IHDR'){width=data.readUInt32BE(0);height=data.readUInt32BE(4);expect(data[8]).toBe(8);channels=data[9]===2?3:data[9]===6?4:0;}
    if(name==='IDAT')chunks.push(data);off+=12+n;
  }
  expect(channels).toBeGreaterThan(0);
  const raw=inflateSync(Buffer.concat(chunks)),stride=width*channels;let prev=Buffer.alloc(stride),off=0,min=255,max=0;const colors=new Set();
  for(let y=0;y<height;y++){
    const filter=raw[off++],row=Buffer.alloc(stride);
    for(let i=0;i<stride;i++){
      const a=i>=channels?row[i-channels]:0,b=prev[i],c=i>=channels?prev[i-channels]:0;
      const p=a+b-c,pa=Math.abs(p-a),pb=Math.abs(p-b),pc=Math.abs(p-c);
      const predictor=[0,a,b,Math.floor((a+b)/2),pa<=pb&&pa<=pc?a:pb<=pc?b:c][filter];
      row[i]=(raw[off++]+predictor)&255;
    }
    for(let x=0;x<width;x+=3){const i=x*channels;const light=(row[i]+row[i+1]+row[i+2])/3;min=Math.min(min,light);max=Math.max(max,light);colors.add(`${row[i]},${row[i+1]},${row[i+2]}`);}
    prev=row;
  }
  expect(max-min,'canvas luminance range').toBeGreaterThan(50);
  expect(colors.size,'canvas color diversity').toBeGreaterThan(128);
}
