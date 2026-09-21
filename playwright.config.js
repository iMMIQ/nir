import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './tests/browser', timeout: 180000, expect: { timeout: 30000 },
  workers: 1, fullyParallel: false,
  reporter: [['list'], ['json', { outputFile: 'reports/browser-results.json' }]],
  outputDir: 'reports/browser-artifacts',
  use: {
    baseURL: process.env.NIR_TEST_URL || 'http://127.0.0.1:4173',
    headless: false, viewport: { width: 1280, height: 800 }, locale: 'zh-CN',
    launchOptions: {
      executablePath: process.env.CHROMIUM || '/usr/bin/chromium',
      args: ['--enable-unsafe-webgpu', ...(process.env.NIR_CHROME_ARGS || '').split(' ').filter(Boolean)],
    },
    screenshot: 'only-on-failure', trace: 'retain-on-failure',
  },
  webServer: [{
    command: 'dist/novelc serve dist/rain-letters-web',
    url: 'http://127.0.0.1:4173', reuseExistingServer: true,
  },{
    command: 'dist/novelc serve dist --port 4174',
    url: 'http://127.0.0.1:4174/rain-letters-web/', reuseExistingServer: true,
  }],
});
