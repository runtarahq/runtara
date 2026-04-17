import { BasePage } from './BasePage';

export class ScenarioLogsPage extends BasePage {
  readonly path: string;
  constructor(
    page: import('@playwright/test').Page,
    scenarioId: string,
    instanceId: string
  ) {
    super(page);
    this.path = `/scenarios/${scenarioId}/history/${instanceId}/logs`;
  }
}

export class ScenarioChatPage extends BasePage {
  readonly path: string;
  constructor(
    page: import('@playwright/test').Page,
    scenarioId: string,
    instanceId?: string
  ) {
    super(page);
    this.path = instanceId
      ? `/scenarios/${scenarioId}/chat/${instanceId}`
      : `/scenarios/${scenarioId}/chat`;
  }
}
