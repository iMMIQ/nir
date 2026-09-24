import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {spawn,execFile} from 'node:child_process';
import {promisify} from 'node:util';

const run=promisify(execFile),cli=path.resolve('dist/novelc'),source=path.resolve('examples/rain-letters');
const action=(page,value)=>page.evaluate(value=>window.__nir.action(value),value);

async function digest(directory){
  return JSON.parse(await fs.readFile(path.join(directory,'channels/stable.json'),'utf8')).release;
}
async function buildCopy(root,version){
  const project=path.join(root,`project-${version}`),out=path.join(root,`build-${version}`);
  await fs.cp(source,project,{recursive:true,filter:file=>{
    const relative=path.relative(source,file);
    return !relative.split(path.sep).some(part=>['dist','reports','.nir','game.lock'].includes(part));
  }});
  const manifest=path.join(project,'game.toml'),original=await fs.readFile(manifest,'utf8');
  await fs.writeFile(manifest,original.replace(/^version = "[^"]+"/m,`version = "${version}"`));
  await run(cli,['-p',project,'resolve']);
  await run(cli,['-p',project,'build','--locked','--out',out]);
  return {out,digest:await digest(out)};
}
async function ready(page,url){
  await page.goto(url);
  await page.waitForFunction(()=>window.__nir?.state().ready && !window.__nir.state().loading);
  await expect(page.locator('#shell')).toBeHidden();
}
async function saveSlot(page){
  await page.keyboard.press('Space');
  await page.waitForFunction(()=>window.__nir.state().dialogue?.ready&&!window.__nir.state().loading);
  await action(page,{type:'saves'});
  await action(page,{type:'save',slot:0});
  await page.waitForFunction(()=>/已保存|Saved/.test(window.__nir.state().status));
}

test('staged A and B preserve same-slot saves across promotion and rollback',async({page})=>{
  test.setTimeout(300000);
  await fs.mkdir(path.resolve('target/tmp'),{recursive:true});
  const temp=await fs.mkdtemp(path.resolve('target/tmp/nir-release-lifecycle-'));
  const live=path.join(temp,'live'),port=4195,origin=`http://127.0.0.1:${port}`;
  let server;
  try{
    const a=await buildCopy(temp,'0.1.0'),b=await buildCopy(temp,'0.2.0');
    expect(a.digest).not.toBe(b.digest);
    for(const build of [a,b]){
      await run(cli,['release','stage','--source',build.out,'--directory',live]);
      await run(cli,['release','verify','--directory',live,'--release',build.digest]);
    }
    await run(cli,['release','promote','--directory',live,'--release',a.digest,'--expect','none']);
    server=spawn(cli,['serve',live,'--port',String(port)],{stdio:'ignore'});
    await expect.poll(async()=>{try{return (await fetch(`${origin}/channels/stable.json`)).ok;}catch{return false;}},{timeout:15000}).toBe(true);
    await run(cli,['release','verify','--url',origin,'--release',b.digest]);

    await ready(page,`${origin}/?test=1`);
    expect(new URL(page.url()).pathname).toBe(`/releases/${a.digest}/index.html`);
    await saveSlot(page);
    await run(cli,['release','promote','--directory',live,'--release',b.digest,'--expect',a.digest]);
    expect(new URL(page.url()).pathname).toBe(`/releases/${a.digest}/index.html`);
    expect((await page.evaluate(()=>window.__nir.state())).error).toBeNull();

    await ready(page,`${origin}/?test=1`);
    expect(new URL(page.url()).pathname).toBe(`/releases/${b.digest}/index.html`);
    await saveSlot(page);
    await run(cli,['release','rollback','--directory',live,'--to',a.digest,'--expect',b.digest]);
    await ready(page,`${origin}/?test=1`);
    expect(new URL(page.url()).pathname).toBe(`/releases/${a.digest}/index.html`);
    await action(page,{type:'saves'});
    await action(page,{type:'load',slot:0});
    await page.waitForFunction(()=>window.__nir.state().screen==='Story'&&!window.__nir.state().loading);

    await page.locator('#nir-history-button').click();
    const panel=page.locator('#nir-history-panel');
    await expect(panel.locator('tbody tr')).toHaveCount(2);
    const aRow=panel.locator('tbody tr').filter({hasText:a.digest});
    const bRow=panel.locator('tbody tr').filter({hasText:b.digest});
    await expect(aRow).toContainText('0.1.0');
    await expect(bRow).toContainText('0.2.0');
    await expect(bRow.getByRole('button',{name:'Open release'})).toBeEnabled();
    await bRow.getByRole('button',{name:'Open release'}).click();
    await page.waitForURL(url=>new URL(url).pathname===`/releases/${b.digest}/index.html`);
    await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
    await action(page,{type:'saves'});
    await action(page,{type:'load',slot:0});
    await page.waitForFunction(()=>window.__nir.state().screen==='Story'&&!window.__nir.state().loading);
  }finally{
    if(server?.exitCode===null&&server.signalCode===null){const exited=new Promise(resolve=>server.once('exit',resolve));server.kill();await exited;}
    await fs.rm(temp,{recursive:true,force:true});
  }
});
