import { BasePage } from './BasePage';

export class ConnectionDetailPage extends BasePage {
  readonly path: string;
  constructor(page: import('@playwright/test').Page, id: string) {
    super(page);
    this.path = `/connections/${id}`;
  }
}

export class CreateConnectionPage extends BasePage {
  readonly path: string;
  constructor(page: import('@playwright/test').Page, integrationId: string) {
    super(page);
    this.path = `/connections/${integrationId}/create`;
  }
}
