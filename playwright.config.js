import { defineConfig } from '@playwright/test';

const novelc = process.platform === 'win32' ? 'dist\\novelc.exe' : 'dist/novelc';
const suite = process.env.NIR_PW_SUITE || 'browser';
const chromiumExecutable =
  process.env.CHROMIUM || (process.platform === 'win32' ? undefined : '/usr/bin/chromium');
const swiftshaderArgs = ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'];

const webServersBySuite = {
  browser: [
    {
      command: `${novelc} serve dist/rain-letters-web`,
      url: 'http://127.0.0.1:4173',
      reuseExistingServer: true,
    },
    {
      command: `${novelc} serve dist --port 4174`,
      url: 'http://127.0.0.1:4174/rain-letters-web/',
      reuseExistingServer: true,
    },
  ],
  backends: [
    {
      command: `${novelc} serve dist/rain-letters-web`,
      url: 'http://127.0.0.1:4173',
      reuseExistingServer: true,
    },
    {
      command: `${novelc} serve dist --port 4174`,
      url: 'http://127.0.0.1:4174/rain-letters-web/',
      reuseExistingServer: true,
    },
  ],
  'nir-next': {
    command: 'python3 scripts/serve_nir_next.py',
    url: 'http://127.0.0.1:4198',
    reuseExistingServer: false,
    timeout: 300000,
  },
  storage: {
    command: 'python3 scripts/serve_storage.py',
    url: 'http://127.0.0.1:4259',
    reuseExistingServer: false,
    timeout: 120000,
  },
  'reading-modes': {
    command: 'python3 scripts/serve_reading_modes.py',
    url: 'http://127.0.0.1:4271',
    reuseExistingServer: false,
    timeout: 120000,
  },
  'livenovel-features': {
    command: 'python3 scripts/serve_livenovel_features.py',
    url: 'http://127.0.0.1:4268',
    reuseExistingServer: process.env.NIR_REUSE_SERVER === '1',
    timeout: 300000,
  },
};

const baseUse = {
  baseURL: process.env.NIR_TEST_URL || 'http://127.0.0.1:4173',
  headless: false,
  viewport: { width: 1280, height: 800 },
  locale: 'zh-CN',
  launchOptions: {
    executablePath: chromiumExecutable,
    args: ['--enable-unsafe-webgpu', ...(process.env.NIR_CHROME_ARGS || '').split(' ').filter(Boolean)],
  },
  screenshot: 'only-on-failure',
  trace: 'retain-on-failure',
};

const headlessChromium = {
  headless: true,
  launchOptions: {
    executablePath: process.env.CHROMIUM || undefined,
    args: swiftshaderArgs,
  },
};

const projectsBySuite = {
  browser: [
    {
      name: 'browser',
      testDir: './tests/browser',
      testIgnore: ['backends.spec.js', 'audio-replacement.spec.js', 'realtime-pcm.spec.js'],
      use: baseUse,
    },
  ],
  backends: [
    {
      name: 'chromium-webgpu',
      testDir: './tests/browser',
      testMatch: 'backends.spec.js',
      metadata: { backend: 'webgpu' },
      grepInvert: /auto falls back|preserves WebGPU title/,
      use: { ...baseUse, browserName: 'chromium' },
    },
    {
      name: 'chromium-webgl2',
      testDir: './tests/browser',
      testMatch: 'backends.spec.js',
      metadata: { backend: 'webgl2' },
      use: { ...baseUse, browserName: 'chromium' },
    },
    {
      name: 'firefox-webgl2',
      testDir: './tests/browser',
      testMatch: 'backends.spec.js',
      metadata: { backend: 'webgl2' },
      grepInvert: /auto falls back|preserves WebGPU title/,
      use: {
        ...baseUse,
        browserName: 'firefox',
        launchOptions: {
          ...(process.env.FIREFOX ? { executablePath: process.env.FIREFOX } : {}),
          firefoxUserPrefs: { 'webgl.force-enabled': true, 'webgl.disabled': false },
        },
      },
    },
  ],
  'nir-next': [
    {
      name: 'nir-next',
      testDir: './tests/nir-next',
      timeout: 90000,
      use: { ...headlessChromium, baseURL: 'http://127.0.0.1:4198' },
    },
  ],
  storage: [
    {
      name: 'storage',
      testDir: './tests/nir-next',
      testMatch: [
        'metadata-write-timeout.spec.js',
        'metadata-read-timeout.spec.js',
        'metadata-retry.spec.js',
        'storage-open.spec.js',
        'startup-metadata.spec.js',
        'persistence.spec.js',
        'save-corruption.spec.js',
        'save-history-corruption.spec.js',
        'export.spec.js',
        'save-read-timeout.spec.js',
        'save-write-timeout.spec.js',
        'save-warning.spec.js',
        'history-check.spec.js',
      ],
      timeout: 90000,
      use: { ...headlessChromium, viewport: { width: 1024, height: 768 } },
    },
  ],
  'reading-modes': [
    {
      name: 'reading-modes',
      testDir: './tests/reading-modes',
      testMatch: '*.spec.js',
      timeout: 90000,
      use: { ...headlessChromium, viewport: { width: 1024, height: 768 } },
    },
  ],
  'reading-endurance': [
    {
      name: 'reading-endurance',
      testDir: './tests/performance',
      testMatch: 'reading-endurance.spec.js',
      timeout: 180000,
      use: {
        ...headlessChromium,
        viewport: { width: 1280, height: 800 },
        locale: 'en-US',
        trace: 'off',
        video: 'off',
        screenshot: 'only-on-failure',
      },
    },
  ],
  'audio-replacement': [
    {
      name: 'audio-replacement',
      testDir: './tests/browser',
      testMatch: 'audio-replacement.spec.js',
      timeout: 180000,
      use: {
        ...headlessChromium,
        viewport: { width: 390, height: 844 },
        deviceScaleFactor: 1,
        locale: 'en-US',
        trace: 'off',
        video: 'off',
        screenshot: 'only-on-failure',
      },
    },
  ],
  'realtime-pcm': [
    {
      name: 'realtime-pcm',
      testDir: './tests/browser',
      testMatch: 'realtime-pcm.spec.js',
      timeout: 120000,
      use: {
        ...headlessChromium,
        viewport: { width: 390, height: 844 },
        locale: 'en-US',
        trace: 'off',
        video: 'off',
        screenshot: 'only-on-failure',
      },
    },
  ],
  'livenovel-features': [
    {
      name: 'livenovel-features',
      testDir: './tests/livenovel-features',
      timeout: 90000,
      use: {
        headless: true,
        launchOptions: {
          executablePath: process.env.CHROMIUM || undefined,
          args: ['--no-proxy-server', ...swiftshaderArgs],
        },
      },
    },
  ],
};

const isBackends = suite === 'backends';
export default defineConfig({
  testDir: './tests/browser',
  timeout: 180000,
  expect: { timeout: suite === 'reading-endurance' ? 5000 : 30000 },
  workers: 1,
  fullyParallel: false,
  reporter: [
    ['list'],
    ['json', { outputFile: isBackends ? 'reports/backend-results.json' : 'reports/browser-results.json' }],
  ],
  outputDir: isBackends ? 'reports/backend-artifacts' : 'reports/browser-artifacts',
  use: baseUse,
  projects: projectsBySuite[suite] || projectsBySuite.browser,
  ...(webServersBySuite[suite] ? { webServer: webServersBySuite[suite] } : {}),
});
