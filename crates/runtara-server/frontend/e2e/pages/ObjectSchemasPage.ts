import { Locator } from '@playwright/test';
import { BasePage } from './BasePage';

export class ObjectSchemasPage extends BasePage {
  readonly path = '/objects/types';
  get newSchemaButton(): Locator {
    return this.page.getByRole('button', {
      name: /new (object )?type|create/i,
    });
  }
}

export class CreateObjectSchemaPage extends BasePage {
  readonly path = '/objects/types/create';
  get kicker(): Locator {
    return this.page.getByText(/^objects$/i).first();
  }
}

export class EditObjectSchemaPage extends BasePage {
  readonly path: string;
  constructor(page: import('@playwright/test').Page, schemaId: string) {
    super(page);
    this.path = `/objects/types/${schemaId}`;
  }
}

export class ManageInstancesPage extends BasePage {
  readonly path: string;
  constructor(page: import('@playwright/test').Page, typeName: string) {
    super(page);
    this.path = `/objects/${typeName}`;
  }
}

export class CreateObjectInstancePage extends BasePage {
  readonly path: string;
  constructor(page: import('@playwright/test').Page, typeName: string) {
    super(page);
    this.path = `/objects/${typeName}/create`;
  }
}

export class EditObjectInstancePage extends BasePage {
  readonly path: string;
  constructor(
    page: import('@playwright/test').Page,
    typeName: string,
    instanceId: string
  ) {
    super(page);
    this.path = `/objects/${typeName}/${instanceId}`;
  }
}
