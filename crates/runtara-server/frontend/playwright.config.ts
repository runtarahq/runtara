import { defineConfig, devices } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Playwright E2E Test Configuration
 * @see https://playwright.dev/docs/test-configuration
 */

// ES module equivalent of __dirname
const __dirname = path.dirname(fileURLToPath(import.meta.url));

// Path to the authenticated state file
const authFile = path.join(__dirname, 'e2e/.auth/user.json');
const mockedAuthFile = path.join(__dirname, 'e2e/.auth/mocked-user.json');

// Base URL for tests - can be overridden via environment variable.
// Vite dev server serves HTTP by default (no HTTPS plugin configured);
// preview builds (npm run preview) also serve HTTP.
const baseURL = process.env.PLAYWRIGHT_BASE_URL || 'http://localhost:8081';

export default defineConfig({
  // Test directory
  testDir: './e2e',

  // Run tests in parallel
  fullyParallel: true,

  // Fail the build on CI if you accidentally left test.only in the source code
  forbidOnly: !!process.env.CI,

  // Retry on CI only
  retries: process.env.CI ? 2 : 0,

  // Opt out of parallel tests on CI
  workers: process.env.CI ? 1 : undefined,

  // Reporter configuration
  reporter: [['html', { outputFolder: 'e2e-report' }], ['list']],

  // Shared settings for all projects
  use: {
    // Base URL for navigation
    baseURL,

    // Collect trace when retrying the failed test
    trace: 'on-first-retry',

    // Screenshot on failure
    screenshot: 'only-on-failure',

    // Video on failure
    video: 'on-first-retry',

    // Block service workers to prevent Workbox from intercepting API requests
    serviceWorkers: 'block',

    // Accept self-signed certificates from Vite's basicSsl plugin
    ignoreHTTPSErrors: true,
  },

  // Configure projects
  projects: [
    // Setup project - runs auth.setup.ts first to authenticate (real Auth0)
    {
      name: 'setup',
      testMatch: /auth\.setup\.ts/,
    },

    // Main test project - uses authenticated state from real Auth0 setup
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        storageState: authFile,
      },
      dependencies: ['setup'],
      testIgnore: [
        /auth\.setup\.ts/,
        /auth\.mocked\.setup\.ts/,
        /tests\/mocked\//,
      ],
    },

    // Smoke tests - real API calls, longer timeout
    {
      name: 'smoke',
      testMatch: /.*\.smoke\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        storageState: authFile,
      },
      dependencies: ['setup'],
      timeout: 60000, // 60s for real API calls
    },

    // Cross-repo E2E tests - full local stack (gateway + backends + DB)
    {
      name: 'e2e',
      testMatch: /.*\.e2e\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        storageState: authFile,
        viewport: { width: 1920, height: 1080 },
        video: { mode: 'on', size: { width: 1920, height: 1080 } },
      },
      dependencies: ['setup'],
      timeout: 120000, // 120s for full-stack round trips
    },

    // ----- Mocked project (PR gate) -----
    //
    // Runs without any network dependencies: a fake OIDC token is injected in
    // auth.mocked.setup.ts and every backend call is intercepted via page.route()
    // inside the specs. Safe to run on fork PRs (no secrets required).
    {
      name: 'mocked-setup',
      testMatch: /auth\.mocked\.setup\.ts/,
    },
    {
      name: 'mocked',
      testMatch: /tests\/mocked\/.*\.mocked\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        storageState: mockedAuthFile,
        viewport: { width: 1280, height: 800 },
      },
      dependencies: ['mocked-setup'],
      timeout: 30000,
    },
  ],

  // Run local dev server before starting tests (skip if using external server via PLAYWRIGHT_BASE_URL)
  webServer: process.env.PLAYWRIGHT_BASE_URL
    ? undefined
    : {
        command: 'npm run dev',
        url: 'http://localhost:8081',
        reuseExistingServer: true,
        timeout: 120 * 1000,
      },

  // Output folder for test artifacts
  outputDir: 'e2e-results',

  // Global timeout for each test
  timeout: 30 * 1000,

  // Expect timeout
  expect: {
    timeout: 5000,
    toHaveScreenshot: {
      maxDiffPixelRatio: 0.01,
      animations: 'disabled',
    },
  },
});
