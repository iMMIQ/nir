import {test, expect} from '@playwright/test';
import {recoverAudioOutput} from './audio-output-helper.js';

async function activate(page, button) {
  await recoverAudioOutput(page);
  await expect(button).toBeVisible();
  await button.focus();
  await page.keyboard.press('Enter');
}
async function history(page) {
  await activate(page, page.getByRole('button', {name:/^History$|^回看$/}));
  await page.waitForFunction(() => __nir.state().screen === 'History' && !__nir.state().loading);
}
async function stored(page) {
  return page.evaluate(async () => {
    for (const {name} of await indexedDB.databases()) {
      const db=await new Promise((ok,no) => {const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      if (!db.objectStoreNames.contains('saves')) {db.close();continue;}
      const rows=await new Promise((ok,no) => {const tx=db.transaction('saves'),r=tx.objectStore('saves').getAll();tx.oncomplete=()=>ok(r.result);tx.onerror=()=>no(tx.error);});
      db.close();
      const found=rows.find(row => row.envelope?.slot === 1 && row.envelope.snapshot.history.some(h => h.choice));
      if (found) return found.envelope.snapshot;
    }
    return null;
  });
}
const frozen = page => page.evaluate(() => {
  const s=__nir.state();
  return {tick:s.tick_us,variables:s.variables,interaction:s.interaction,count:s.history_count,position:s.position};
});

for (const worker of ['required','main']) {
  for (const resolution of ['selected','timed_out','cancelled']) {
    test(`history records ${resolution} and restores without committing again, ${worker}`, async ({page}) => {
      const errors=[];page.on('pageerror',error=>errors.push(error.message));
      await page.setViewportSize({width:390,height:844});
      const port=resolution === 'timed_out' ? 4253 : 4223;
      await page.goto(`http://127.0.0.1:${port}/?test=1&worker=${worker}&backend=webgl2`);
      await page.waitForFunction(() => globalThis.__nir?.state().ready && !__nir.state().loading);
      await page.keyboard.press('Enter');
      await page.waitForFunction(() => __nir.state().choice && !__nir.state().loading);
      const offered=await page.evaluate(() => __nir.state().choice);
      if (resolution === 'selected') {
        await activate(page,page.getByRole('button',{name:offered.options.find(o=>o.id==='stay').label,exact:true}));
      } else if (resolution === 'cancelled') {
        await page.keyboard.press('Escape');
      }
      await page.waitForFunction(() => !__nir.state().choice && __nir.state().dialogue && !__nir.state().loading);
      await history(page);
      const before=await frozen(page);
      await expect(page.getByRole('button',{name:/^Replay voice$|^重播语音$/})).toHaveCount(0);
      await page.waitForTimeout(150);
      expect(await frozen(page)).toEqual(before);
      await page.evaluate(() => __nir.action({type:'save',slot:1}));
      await expect.poll(() => stored(page)).not.toBeNull();
      const saved=await stored(page), record=saved.history.find(h=>h.choice);
      expect(record.choice.id).toBe(offered.id);
      expect(record.interaction).toBe(offered.interaction);
      expect(record.locale).toBe(offered.locale);
      expect(record.font_plan_digest).toBe(offered.font_plan_digest);
      expect(record.choice.options.map(({id,label,enabled})=>({id,label,enabled}))).toEqual(offered.options);
      expect(record.choice.resolution).toEqual(resolution === 'cancelled' ? {type:resolution} : {type:resolution,option:'stay'});
      expect(record.voices || []).toEqual([]);
      expect(saved.variables.picked.value).toBe(resolution === 'cancelled' ? 0 : 2);
      expect(await frozen(page)).toEqual(before);
      await page.screenshot({path:`reports/experience-choice-${resolution}-${worker}.png`});
      await page.reload();
      await page.waitForFunction(() => globalThis.__nir?.state().ready && !__nir.state().loading);
      const session=await page.evaluate(()=>__nir.state().session);
      await page.evaluate(()=>__nir.action({type:'load',slot:1}));
      await page.waitForFunction(session=>__nir.state().session !== session && !__nir.state().loading && __nir.state().paused,session);
      expect(await page.evaluate(()=>__nir.state().variables)).toEqual(saved.variables);
      expect(await page.evaluate(()=>__nir.state().history_count)).toBe(saved.history.length);
      await page.evaluate(()=>__nir.action({type:'continue'}));
      await page.waitForFunction(()=>__nir.state().screen === 'Story');
      await recoverAudioOutput(page);
      await page.waitForFunction(()=>__nir.state().screen === 'Story' && !__nir.state().paused);
      await history(page);
      const restored=await frozen(page);
      await page.waitForTimeout(150);
      expect(await frozen(page)).toEqual(restored);
      await activate(page,page.getByRole('button',{name:/^Back to story$|^返回故事$/}));
      await page.waitForFunction(()=>__nir.state().screen === 'Story');
      await recoverAudioOutput(page);
      await page.waitForFunction(()=>__nir.state().screen === 'Story' && !__nir.state().paused);
      expect(await page.evaluate(()=>__nir.state().variables)).toEqual(saved.variables);
      expect(await page.evaluate(()=>__nir.state().history_count)).toBe(saved.history.length);
      expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
    });
  }
}
