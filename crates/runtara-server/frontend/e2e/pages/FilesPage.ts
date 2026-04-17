import { Locator } from '@playwright/test';
import { BasePage } from './BasePage';

export class FilesPage extends BasePage {
  readonly path = '/files';

  get uploadButton(): Locator {
    return this.page.getByRole('button', { name: /upload/i });
  }

  get emptyStateMessage(): Locator {
    return this.page.getByText(/no file storage connections/i);
  }

  get connectionSelector(): Locator {
    return this.page.getByRole('combobox').first();
  }
}
