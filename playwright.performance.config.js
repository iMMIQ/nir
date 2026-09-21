import base from './playwright.config.js';
export default {
  ...base, testDir:'./tests/performance', timeout:600000,
  reporter:[['list'],['json',{outputFile:'reports/performance-results.json'}]],
};
