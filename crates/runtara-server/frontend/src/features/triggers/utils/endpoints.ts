import { config } from '@/shared/config/runtimeConfig';

// Trigger URLs are copied by users into external systems (webhook configs,
// BotFather, etc.), so they must always be absolute. When `apiBaseUrl` is
// absolute we use it as-is; otherwise we anchor to the current page origin.
function resolveBaseUrl(): string {
  const configured = (config.apiBaseUrl ?? '').replace(/\/$/, '');
  if (/^https?:\/\//i.test(configured)) {
    return configured;
  }
  return `${window.location.origin}${configured}`;
}

export function getHttpTriggerUrl(
  triggerId: string,
  tenantId: string,
  eventType: string = 'http'
): string {
  return `${resolveBaseUrl()}/api/events/${tenantId}/${eventType}/${triggerId}/my-action`;
}

/**
 * Synchronous execution endpoint (api/handlers/workflows_sync.rs). Executes
 * the workflow inline and returns the result in the HTTP response — 30 s hard
 * timeout, no execution history recorded. Keyed by workflow id, not trigger
 * id, and accepts any HTTP method.
 */
export function getHttpSyncUrl(workflowId: string, tenantId: string): string {
  return `${resolveBaseUrl()}/api/events/${tenantId}/http-sync/${workflowId}`;
}

export function getEmailTriggerAddress(triggerId: string): string {
  const domain =
    import.meta.env.VITE_EMAIL_EVENTS_DOMAIN || 'events.example.com';
  return `workflow-${triggerId}@${domain}`;
}

export function getChannelWebhookUrl(
  tenantId: string,
  connectionId: string,
  platform: string = 'telegram'
): string {
  return `${resolveBaseUrl()}/api/events/${tenantId}/webhook/${platform}/${connectionId}`;
}
