/* eslint-disable react-hooks/rules-of-hooks */
import { test as base, Page } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Authentication fixture for e2e tests
 *
 * Most tests will automatically use the saved auth state from auth.setup.ts
 * via the storageState in playwright.config.ts.
 *
 * This fixture provides additional utilities for tests that need
 * to manipulate auth state or test logout flows.
 */

// ES module equivalent of __dirname
const __dirname = path.dirname(fileURLToPath(import.meta.url));

const authFile = path.join(__dirname, '../.auth/user.json');

export interface AuthFixtures {
  /**
   * Check if the current page is authenticated
   */
  isAuthenticated: (page: Page) => Promise<boolean>;

  /**
   * Clear authentication state (logout)
   */
  clearAuth: (page: Page) => Promise<void>;

  /**
   * Get the path to the auth state file
   */
  authStatePath: string;
}

/**
 * Extended test with auth fixtures
 */
export const test = base.extend<AuthFixtures>({
  /**
   * Path to the authentication state file
   */
  authStatePath: authFile,

  /**
   * Check if the current session is authenticated
   */
  isAuthenticated: async ({}, use) => {
    const checkAuth = async (page: Page): Promise<boolean> => {
      // Check for OIDC tokens in storage
      const hasToken = await page.evaluate(() => {
        // Check localStorage for OIDC tokens
        const keys = Object.keys(localStorage);
        const oidcKey = keys.find((key) => key.startsWith('oidc.'));
        if (oidcKey) {
          const data = localStorage.getItem(oidcKey);
          if (data) {
            try {
              const parsed = JSON.parse(data);
              return !!parsed.access_token;
            } catch {
              return false;
            }
          }
        }
        return false;
      });

      return hasToken;
    };

    await use(checkAuth);
  },

  /**
   * Clear authentication state
   */
  clearAuth: async ({}, use) => {
    const clearAuthFn = async (page: Page) => {
      await page.evaluate(() => {
        // Clear OIDC-related items from localStorage
        const keys = Object.keys(localStorage);
        keys.forEach((key) => {
          if (key.startsWith('oidc.')) {
            localStorage.removeItem(key);
          }
        });

        // Clear sessionStorage as well
        const sessionKeys = Object.keys(sessionStorage);
        sessionKeys.forEach((key) => {
          if (key.startsWith('oidc.')) {
            sessionStorage.removeItem(key);
          }
        });
      });

      // Clear cookies
      await page.context().clearCookies();
    };

    await use(clearAuthFn);
  },
});

export { expect } from '@playwright/test';
