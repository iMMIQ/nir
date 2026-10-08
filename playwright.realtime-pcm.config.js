import {defineConfig} from '@playwright/test';
export default defineConfig({
  testDir:'./tests/browser',testMatch:'realtime-pcm.spec.js',workers:1,timeout:120000,
  expect:{timeout:15000},
  use:{headless:true,viewport:{width:390,height:844},locale:'en-US',trace:'off',video:'off',screenshot:'only-on-failure',
    launchOptions:{executablePath:process.env.CHROMIUM||undefined,args:['--use-angle=swiftshader','--enable-unsafe-swiftshader']}},
});
