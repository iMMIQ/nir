import {defineConfig} from '@playwright/test';
export default defineConfig({testDir:'./tests/reading-modes',testMatch:'*.spec.js',workers:1,timeout:90000,
 use:{headless:true,viewport:{width:1024,height:768},launchOptions:{executablePath:process.env.CHROMIUM,args:['--use-angle=swiftshader','--enable-unsafe-swiftshader']}},
 webServer:{command:'python3 scripts/serve_reading_modes.py',url:'http://127.0.0.1:4271',reuseExistingServer:false,timeout:120000},
});
