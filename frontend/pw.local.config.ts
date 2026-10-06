import base from './playwright.config';
import { defineConfig } from '@playwright/test';
// Local-only: reuse an already installed headless shell (not committed).
export default defineConfig({
  ...base,
  use: { ...base.use, launchOptions: { executablePath: process.env.PW_EXEC } },
});
