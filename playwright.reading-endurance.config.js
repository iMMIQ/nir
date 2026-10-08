import {defineConfig} from '@playwright/test';
export default defineConfig({
  testDir:'./tests/performance',testMatch:'reading-endurance.spec.js',workers:1,timeout:180000,
  expect:{timeout:5000},
  use:{headless:true,viewport:{width:1280,height:800},locale:'en-US',trace:'off',video:'off',screenshot:'only-on-failure',
    launchOptions:{executablePath:process.env.CHROMIUM||undefined,args:['--use-angle=swiftshader','--enable-unsafe-swiftshader']}},
});
