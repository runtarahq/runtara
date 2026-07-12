import type {
  ConnectionStatus,
  ConnectionTypeDto,
} from '@/generated/RuntaraRuntimeApi';
import type { StatusTone } from '@/shared/components/console';
import type { FormDefinition } from '@/shared/forms';

export type ConnectionStatusPill = { tone: StatusTone; label: string };

/**
 * Single source of truth for the connection status pill, shared by the
 * connections list and the edit-page status card so the two never diverge.
 */
export function connectionStatusPill(
  status: ConnectionStatus
): ConnectionStatusPill {
  switch (status) {
    case 'ACTIVE':
      return { tone: 'success', label: 'Connected' };
    case 'REQUIRES_RECONNECTION':
      return { tone: 'warning', label: 'Reconnect required' };
    case 'INVALID_CREDENTIALS':
      return { tone: 'error', label: 'Invalid credentials' };
    default:
      return { tone: 'neutral', label: 'Unknown' };
  }
}

type ConnectionTypeWithForm = ConnectionTypeDto & {
  formDefinition?: FormDefinition;
};

export type ConnectionIdentityEntry = { label: string; value: string };

/**
 * Human-readable identity of a connection sourced from its provider-managed
 * `read`-access projection values — e.g. the QuickBooks company realm captured
 * at consent. Only read-access (grant-state) fields are surfaced so the line
 * stays a stable identity rather than echoing editable config; secrets are
 * never present because the projection omits write-access values.
 */
export function connectionIdentity(
  connectionType: ConnectionTypeWithForm,
  values: Record<string, unknown> | undefined
): ConnectionIdentityEntry[] {
  const definition = connectionType.formDefinition;
  if (!definition || !values) return [];
  const entries: ConnectionIdentityEntry[] = [];
  for (const [name, field] of Object.entries(definition.fields)) {
    if (field.access !== 'read') continue;
    const raw = values[name];
    if (raw === undefined || raw === null || raw === '') continue;
    entries.push({
      label: field.label ?? name.replace(/_/g, ' '),
      value: String(raw),
    });
  }
  return entries;
}
