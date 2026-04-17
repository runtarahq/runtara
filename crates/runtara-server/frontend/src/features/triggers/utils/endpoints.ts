export function getHttpTriggerUrl(
  triggerId: string,
  tenantId: string,
  eventType: string = 'http'
): string {
  const baseUrl = (import.meta.env.VITE_RUNTARA_API_BASE_URL as string).replace(
    /\/$/,
    ''
  );
  return `${baseUrl}/api/events/${tenantId}/${eventType}/${triggerId}/my-action`;
}

export function getEmailTriggerAddress(triggerId: string): string {
  const domain =
    import.meta.env.VITE_EMAIL_EVENTS_DOMAIN || 'events.example.com';
  return `scenario-${triggerId}@${domain}`;
}

export function getChannelWebhookUrl(
  tenantId: string,
  connectionId: string,
  platform: string = 'telegram'
): string {
  const baseUrl = (import.meta.env.VITE_RUNTARA_API_BASE_URL as string).replace(
    /\/$/,
    ''
  );
  return `${baseUrl}/api/events/${tenantId}/webhook/${platform}/${connectionId}`;
}
