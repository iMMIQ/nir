import { defineConfig } from '@playwright/test';
import base from './playwright.config.js';

export default defineConfig({
  ...base,
  testMatch: 'backends.spec.js',
  testIgnore: [],
  outputDir: 'reports/backend-artifacts',
  reporter: [['list'], ['json', { outputFile: 'reports/backend-results.json' }]],
  projects: [
    { name: 'chromium-webgpu', metadata: { backend: 'webgpu' }, grepInvert: /auto falls back|preserves WebGPU title/, use: { ...base.use, browserName: 'chromium' } },
    { name: 'chromium-webgl2', metadata: { backend: 'webgl2' }, use: { ...base.use, browserName: 'chromium' } },
    { name: 'firefox-webgl2', metadata: { backend: 'webgl2' }, grepInvert: /auto falls back|preserves WebGPU title/, use: {
      ...base.use, browserName: 'firefox',
      launchOptions: {
        ...(process.env.FIREFOX ? { executablePath: process.env.FIREFOX } : {}),
        firefoxUserPrefs: { 'webgl.force-enabled': true, 'webgl.disabled': false },
      },
    } },
  ],
});
