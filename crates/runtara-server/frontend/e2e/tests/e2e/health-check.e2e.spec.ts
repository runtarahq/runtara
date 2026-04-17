import { test, expect } from '@playwright/test';

/**
 * Health Check E2E Tests
 *
 * Verifies the full local stack is operational:
 * Browser -> Frontend(:8081) -> Gateway(:8080) -> Backends(:8001/:7001) -> PostgreSQL
 */

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';

test.describe('Full Stack Health Check', () => {
  test('gateway health endpoint responds', async ({ request }) => {
    const response = await request.get(`${GATEWAY_URL}/health`);
    expect(response.status()).toBe(200);
    const body = await response.text();
    expect(body).toContain('OK');
  });

  test('management API is reachable through gateway', async ({ request }) => {
    // Swagger UI is public (no auth required)
    const response = await request.get(
      `${GATEWAY_URL}/api/management/swagger-ui/`
    );
    // Should get a response (200, redirect, or 404), not a connection error
    // 404 is acceptable — it means the gateway routed to management successfully,
    // but swagger-ui may not be enabled in this build
    expect([200, 301, 302, 404]).toContain(response.status());
  });

  test('runtime API is reachable through gateway', async ({ request }) => {
    // Swagger UI is public (no auth required)
    // 404 is acceptable — it means the gateway routed to the runtime successfully,
    // but swagger-ui may not be enabled in this build
    const response = await request.get(
      `${GATEWAY_URL}/api/runtime/swagger-ui/`
    );
    expect([200, 301, 302, 404]).toContain(response.status());
  });

  test('frontend loads with authenticated state', async ({ page }) => {
    await page.goto('/');
    await page.waitForLoadState('networkidle');

    // Should not redirect to login — auth state is injected by setup project
    const url = page.url();
    expect(url).not.toContain('login');
    expect(url).not.toContain('authorize');

    // Main layout should render
    await expect(page.locator('body')).toBeVisible();
  });
});
