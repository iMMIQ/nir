import {defineConfig} from '@playwright/test';
export default defineConfig({
  testDir:'./tests/browser',testMatch:'audio-replacement.spec.js',workers:1,timeout:180000,
  expect:{timeout:15000},
  use:{headless:true,viewport:{width:390,height:844},deviceScaleFactor:1,locale:'en-US',trace:'off',video:'off',screenshot:'only-on-failure',
    launchOptions:{executablePath:process.env.CHROMIUM||undefined,args:['--use-angle=swiftshader','--enable-unsafe-swiftshader']}},
});
