import base from './playwright.save-warning.config.js';
import {defineConfig} from '@playwright/test';
export default defineConfig({...base,testMatch:[...base.testMatch,'history-check.spec.js']});
