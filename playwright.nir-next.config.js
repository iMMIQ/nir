import {defineConfig} from '@playwright/test';
export default defineConfig({
  testDir:'./tests/nir-next',workers:1,timeout:90000,
  use:{baseURL:'http://127.0.0.1:4198',headless:true,
    launchOptions:{args:['--use-angle=swiftshader','--enable-unsafe-swiftshader']}},
  webServer:{command:'python3 scripts/serve_nir_next.py',url:'http://127.0.0.1:4198',reuseExistingServer:false,timeout:120000},
});
