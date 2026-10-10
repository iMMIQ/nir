import base from './playwright.save-write.config.js';
import {defineConfig} from '@playwright/test';
export default defineConfig({...base,testMatch:['save-warning.spec.js',...base.testMatch]});
