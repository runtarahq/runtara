import { afterEach, describe, expect, it } from 'vitest';
import { config } from '@/shared/config/runtimeConfig';
import {
  getChannelWebhookUrl,
  getHttpSyncUrl,
  getHttpTriggerUrl,
} from './endpoints';

describe('trigger URLs', () => {
  const saved = { ...config };
  afterEach(() => {
    config.apiBaseUrl = saved.apiBaseUrl;
    config.eventsBaseUrl = saved.eventsBaseUrl;
  });

  it('are made on the events host when one is configured', () => {
    config.apiBaseUrl = 'https://app.runtara.dev';
    config.eventsBaseUrl = 'https://events.runtara.dev/';
    expect(getHttpTriggerUrl('t1', 'acme')).toBe(
      'https://events.runtara.dev/api/events/acme/http/t1/my-action'
    );
    expect(getHttpSyncUrl('w1', 'acme')).toBe(
      'https://events.runtara.dev/api/events/acme/http-sync/w1'
    );
    expect(getChannelWebhookUrl('acme', 'c1', 'slack')).toBe(
      'https://events.runtara.dev/api/events/acme/webhook/slack/c1'
    );
  });

  it("are made on the console's host otherwise", () => {
    config.apiBaseUrl = 'https://app.runtara.dev';
    config.eventsBaseUrl = undefined;
    expect(getHttpTriggerUrl('t1', 'acme')).toBe(
      'https://app.runtara.dev/api/events/acme/http/t1/my-action'
    );
  });
});
