import { BasePage } from './BasePage';

export class CreateTriggerPage extends BasePage {
  readonly path = '/invocation-triggers/create';
}

export class EditTriggerPage extends BasePage {
  readonly path: string;
  constructor(page: import('@playwright/test').Page, triggerId: string) {
    super(page);
    this.path = `/invocation-triggers/${triggerId}`;
  }
}
