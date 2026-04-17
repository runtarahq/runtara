/* eslint-disable react-hooks/rules-of-hooks */
import { test as base, Page, Route } from '@playwright/test';

/**
 * API mocking fixture for e2e tests
 * Provides utilities for intercepting and mocking API responses
 */

export interface ApiFixtures {
  /**
   * Mock a specific API endpoint
   */
  mockApi: (
    page: Page,
    urlPattern: string | RegExp,
    response: object | ((route: Route) => Promise<void>),
    options?: { status?: number; contentType?: string }
  ) => Promise<void>;

  /**
   * Mock multiple API endpoints at once
   */
  mockApis: (
    page: Page,
    mocks: Array<{
      urlPattern: string | RegExp;
      response: object;
      status?: number;
    }>
  ) => Promise<void>;

  /**
   * Wait for a specific API call
   */
  waitForApi: (page: Page, urlPattern: string | RegExp) => Promise<void>;
}

/**
 * Extended test with API fixtures
 */
export const test = base.extend<ApiFixtures>({
  /**
   * Mock a single API endpoint
   */
  mockApi: async ({}, use) => {
    const mockApiFn = async (
      page: Page,
      urlPattern: string | RegExp,
      response: object | ((route: Route) => Promise<void>),
      options: { status?: number; contentType?: string } = {}
    ) => {
      const { status = 200, contentType = 'application/json' } = options;

      await page.route(urlPattern, async (route) => {
        if (typeof response === 'function') {
          await response(route);
        } else {
          await route.fulfill({
            status,
            contentType,
            body: JSON.stringify(response),
          });
        }
      });
    };

    await use(mockApiFn);
  },

  /**
   * Mock multiple API endpoints
   */
  mockApis: async ({}, use) => {
    const mockApisFn = async (
      page: Page,
      mocks: Array<{
        urlPattern: string | RegExp;
        response: object;
        status?: number;
      }>
    ) => {
      for (const mock of mocks) {
        await page.route(mock.urlPattern, async (route) => {
          await route.fulfill({
            status: mock.status ?? 200,
            contentType: 'application/json',
            body: JSON.stringify(mock.response),
          });
        });
      }
    };

    await use(mockApisFn);
  },

  /**
   * Wait for a specific API call to complete
   */
  waitForApi: async ({}, use) => {
    const waitForApiFn = async (page: Page, urlPattern: string | RegExp) => {
      await page.waitForResponse((response) => {
        const url = response.url();
        if (typeof urlPattern === 'string') {
          return url.includes(urlPattern);
        }
        return urlPattern.test(url);
      });
    };

    await use(waitForApiFn);
  },
});

/**
 * Common API mock data factories
 */
export const mockData = {
  /**
   * Create mock connection data
   */
  connection: (overrides = {}) => ({
    id: 'conn-123',
    name: 'Test Connection',
    type: 'api',
    status: 'active',
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    ...overrides,
  }),

  /**
   * Create mock trigger data
   */
  trigger: (overrides = {}) => ({
    id: 'trigger-123',
    name: 'Test Trigger',
    type: 'webhook',
    enabled: true,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    ...overrides,
  }),

  /**
   * Create mock scenario data
   */
  scenario: (overrides = {}) => ({
    id: 'scenario-123',
    name: 'Test Scenario',
    status: 'active',
    steps: [],
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    ...overrides,
  }),

  /**
   * Create paginated response wrapper
   */
  paginated: <T>(items: T[], page = 1, pageSize = 10, total?: number) => ({
    data: items,
    pagination: {
      page,
      pageSize,
      total: total ?? items.length,
      totalPages: Math.ceil((total ?? items.length) / pageSize),
    },
  }),
};

export { expect } from '@playwright/test';
