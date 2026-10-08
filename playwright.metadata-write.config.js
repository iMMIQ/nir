import {defineConfig} from '@playwright/test';
export default defineConfig({testDir:'./tests/nir-next',workers:1,timeout:90000,
 testMatch:['metadata-write-timeout.spec.js','metadata-read-timeout.spec.js','metadata-retry.spec.js','storage-open.spec.js','startup-metadata.spec.js','persistence.spec.js','save-corruption.spec.js','save-history-corruption.spec.js','export.spec.js'],
 use:{headless:true,viewport:{width:1024,height:768},launchOptions:{executablePath:process.env.CHROMIUM,args:['--use-angle=swiftshader','--enable-unsafe-swiftshader']}},
 webServer:{command:'python3 scripts/serve_storage.py',url:'http://127.0.0.1:4259',reuseExistingServer:false,timeout:120000},
});
