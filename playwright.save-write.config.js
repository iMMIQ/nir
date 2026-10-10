import base from './playwright.save-read.config.js';
import {defineConfig} from '@playwright/test';
export default defineConfig({...base,testMatch:['save-write-timeout.spec.js',...base.testMatch]});
