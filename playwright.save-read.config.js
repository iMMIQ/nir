import base from './playwright.metadata-write.config.js';
import {defineConfig} from '@playwright/test';
export default defineConfig({...base,testMatch:['save-read-timeout.spec.js',...base.testMatch]});
