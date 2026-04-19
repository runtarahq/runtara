import { BasePage } from './BasePage';

export class WorkflowLogsPage extends BasePage {
  readonly path: string;
  constructor(
    page: import('@playwright/test').Page,
    workflowId: string,
    instanceId: string
  ) {
    super(page);
    this.path = `/workflows/${workflowId}/history/${instanceId}/logs`;
  }
}

export class WorkflowChatPage extends BasePage {
  readonly path: string;
  constructor(
    page: import('@playwright/test').Page,
    workflowId: string,
    instanceId?: string
  ) {
    super(page);
    this.path = instanceId
      ? `/workflows/${workflowId}/chat/${instanceId}`
      : `/workflows/${workflowId}/chat`;
  }
}
