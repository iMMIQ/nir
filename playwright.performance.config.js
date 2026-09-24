import base from './playwright.config.js';
export default {
  ...base,
  testDir: './tests/performance',
  timeout: 900000,
  expect: { ...base.expect, timeout: 30000 },
  use: {
    ...base.use, actionTimeout: 45000, navigationTimeout: 60000,
    launchOptions: {
      ...base.use.launchOptions,
      // Playwright adds this permission by default, even with native Vulkan.
      ignoreDefaultArgs: [
        ...(process.env.NIR_PERF_MODE === 'hardware' ? ['--enable-unsafe-swiftshader'] : []),
        ...(process.env.NIR_PERF_NATIVE_SHM === '1' ? ['--disable-dev-shm-usage'] : []),
      ],
    },
  },
  workers: 1,
  fullyParallel: false,
  reporter: [['list'], ['json', { outputFile: 'reports/performance-results.json' }]],
};
