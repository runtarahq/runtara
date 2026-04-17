import { mergeTests } from '@playwright/test';
import { test as authTest, expect as authExpect } from './auth.fixture';
import { test as apiTest, mockData } from './api.fixture';
import { test as mockTest } from './mock.fixture';
import { test as a11yTest } from './a11y.fixture';

/**
 * Combined test fixture with all fixtures merged.
 * Use this as the default import for tests that need multiple fixtures.
 */
export const test = mergeTests(authTest, apiTest, mockTest, a11yTest);

export { authExpect as expect, mockData };

/**
 * Re-export individual fixtures for selective use.
 */
export { test as authTest } from './auth.fixture';
export { test as apiTest, mockData as apiMockData } from './api.fixture';
export { test as mockTest } from './mock.fixture';
export { test as a11yTest } from './a11y.fixture';
export * from './builders';
export type { MockApi } from './mock.fixture';
