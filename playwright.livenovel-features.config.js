import {defineConfig} from '@playwright/test';
export default defineConfig({
  testDir:'./tests/livenovel-features',workers:1,timeout:90000,
  use:{headless:true,launchOptions:{executablePath:process.env.CHROMIUM||undefined,
    args:['--no-proxy-server','--use-angle=swiftshader','--enable-unsafe-swiftshader']}},
  webServer:{command:'python3 scripts/serve_livenovel_features.py',url:'http://127.0.0.1:4268',reuseExistingServer:process.env.NIR_REUSE_SERVER==='1',timeout:300000},
});
