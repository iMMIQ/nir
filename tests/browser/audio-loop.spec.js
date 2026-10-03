import { test, expect } from '@playwright/test';
import fs from 'node:fs/promises';

// These generated, neutral chirp fixtures exercise encoder delay and padding;
// the loop test checks that playback adds no samples beyond the decoded loop.
for (const rate of [44100, 48000]) {
  test(`gapless MP3 preserves alignment and repeats 100 times at ${rate} Hz`, async ({ page }) => {
    const fixture = new URL('../../apps/player-desktop/tests/fixtures/', import.meta.url);
    const wav = [...await fs.readFile(new URL('gapless-44100-mono.wav', fixture))];
    const mp3 = [...await fs.readFile(new URL('gapless-44100-mono.mp3', fixture))];
    const result = await page.evaluate(async ({ wav, mp3, rate }) => {
      const decode = new OfflineAudioContext(1, rate, rate);
      const reference = await decode.decodeAudioData(Uint8Array.from(wav).buffer);
      const buffer = await decode.decodeAudioData(Uint8Array.from(mp3).buffer);
      // Starting playback may detach views returned by getChannelData.
      const a = reference.getChannelData(0).slice(), b = buffer.getChannelData(0).slice();
      const skip = Math.round(rate * .05), count = Math.round(rate * .35);
      let best = { offset: null, rms: Infinity };
      for (let offset = -20; offset <= 20; offset++) {
        let sum = 0;
        for (let i = skip; i < skip + count; i++) sum += (a[i] - b[i + offset]) ** 2;
        const rms = Math.sqrt(sum / count);
        if (rms < best.rms) best = { offset, rms };
      }
      const context = new OfflineAudioContext(1, buffer.length * 100, rate);
      const source = context.createBufferSource();
      source.buffer = buffer; source.loop = true;
      source.connect(context.destination); source.start();
      const rendered = (await context.startRendering()).getChannelData(0);
      let maxLoopError = 0;
      for (let i = 0; i < rendered.length; i++) {
        maxLoopError = Math.max(maxLoopError, Math.abs(rendered[i] - b[i % b.length]));
      }
      return { referenceFrames: reference.length, frames: buffer.length,
        rate: buffer.sampleRate, channels: buffer.numberOfChannels, best, maxLoopError };
    }, { wav, mp3, rate });
    expect(result.rate).toBe(rate);
    expect(result.channels).toBe(1);
    expect(Math.abs(result.frames - result.referenceFrames)).toBeLessThanOrEqual(1);
    expect(result.referenceFrames).toBe(rate / 2);
    expect(result.best.offset).toBe(0);
    expect(result.best.rms).toBeLessThan(.05);
    expect(result.maxLoopError).toBeLessThan(.00001);
  });
}
