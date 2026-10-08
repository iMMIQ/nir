import {expect} from '@playwright/test';

// Real Chrome may request a new gesture after a paused route is released.
// Follow the displayed recovery UI before sending the next gameplay input.
export async function recoverAudioOutput(page) {
  await page.waitForFunction(() => {
    const state = globalThis.__nir?.state();
    return document.querySelector('#nir-audio-resume')?.hidden === false ||
      state && (state.screen !== 'Story' || !state.paused);
  }, null, {timeout:15000});
  const prompt = page.locator('#nir-audio-resume');
  if(await prompt.isVisible()&&await prompt.isDisabled()) {
    // A gesture already authorized the outstanding native request. The
    // disabled waiting control cannot issue another resume: observe either
    // actual recovery or a newly actionable permission/failure state.
    await page.waitForFunction(()=>{
      const button=document.querySelector('#nir-audio-resume');
      return button?.hidden===true||button?.disabled===false;
    },null,{timeout:15000});
  }
  if (await prompt.isVisible()) {
    const before = await page.evaluate(() => ({session:__nir.state().session, interaction:__nir.state().interaction}));
    try {
      await prompt.click({timeout:5000});
    } catch (error) {
      // Output can recover itself between the visibility observation and
      // Playwright's trusted click. Do not wait for a vanished prompt to
      // return; an unchanged, unpaused Story must prove recovery instead.
      if (error.name !== 'TimeoutError' || await prompt.isVisible() ||
          !await page.evaluate(() => __nir.state().screen === 'Story' && !__nir.state().paused)) throw error;
    }
    try {
      await expect(prompt).toBeHidden();
    } catch (error) {
      const proof = await page.evaluate(() => ({state:__nir.state(),
        audio:__nir.diagnostics().host_work.audio_domains,
        sources:(globalThis.authoredAudio || globalThis.voicePrefAudit || []).map(r => ({
          loop:r.source.loop,stops:r.stops,ended:r.ended,state:r.context.state,time:r.context.currentTime,
        })), hidden:document.hidden}));
      throw new Error(`Recovery prompt remained after its gesture: ${JSON.stringify(proof)}; ${error}`);
    }
    expect(await page.evaluate(() => ({session:__nir.state().session, interaction:__nir.state().interaction}))).toEqual(before);
  }
  await page.waitForFunction(() => {
    const state = __nir.state();
    return state.screen !== 'Story' || !state.paused;
  }, null, {timeout:15000});
}
