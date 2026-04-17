import { expect, Locator, Page } from '@playwright/test';

export abstract class BasePage {
  protected readonly page: Page;
  abstract readonly path: string;

  constructor(page: Page) {
    this.page = page;
  }

  async goto(): Promise<void> {
    if (process.env.E2E_DEBUG === 'true') {
      this.page.on('pageerror', (err) =>
        console.log(`[page-error] ${this.path}:`, err.message)
      );
      this.page.on('console', (msg) => {
        if (msg.type() === 'error')
          console.log(`[browser-error] ${this.path}:`, msg.text());
      });
    }
    await this.page.goto(this.path);
    await this.waitForReady();
  }

  /** Default readiness check: page is interactive and sidebar nav is visible. */
  async waitForReady(): Promise<void> {
    await this.page.waitForLoadState('domcontentloaded');
    await expect(this.main).toBeVisible();
  }

  get main(): Locator {
    return this.page.locator('main');
  }

  get heading(): Locator {
    return this.page.getByRole('heading').first();
  }

  get sidebar(): Locator {
    return this.page.locator('[data-testid="sidebar"], aside').first();
  }

  async expectHeading(pattern: RegExp | string): Promise<void> {
    const re = typeof pattern === 'string' ? new RegExp(pattern, 'i') : pattern;
    await expect(
      this.page.getByRole('heading', { name: re }).first()
    ).toBeVisible();
  }

  /**
   * Take a deterministic screenshot for visual regression.
   * Skipped unless E2E_VISUAL=true is set — snapshots are platform-specific
   * (darwin vs linux font rendering differs), so they should only be generated
   * and compared on one canonical platform (our CI ubuntu runners). Running
   * without the env var keeps local runs fast and avoids committing darwin
   * snapshots alongside linux ones. See e2e/README.md for the workflow.
   */
  async expectMatchesSnapshot(name: string): Promise<void> {
    if (process.env.E2E_VISUAL !== 'true') return;
    await expect(this.page).toHaveScreenshot(`${name}.png`, {
      fullPage: true,
      maxDiffPixelRatio: 0.01,
      animations: 'disabled',
    });
  }
}
