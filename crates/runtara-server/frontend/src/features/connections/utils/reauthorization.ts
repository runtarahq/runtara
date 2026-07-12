import type { ConnectionFieldBehavior } from '@/generated/RuntaraRuntimeApi';

/**
 * True when a set of touched/cleared parameter fields includes any field the
 * descriptor marks `requiresReauthorization` — i.e. saving would reset the
 * provider authorization. Drives the Save & Reconnect prompt and guard.
 */
export function patchStripsAuthorization(
  fieldBehaviors: Partial<Record<string, ConnectionFieldBehavior>>,
  touchedFields: Iterable<string>
): boolean {
  for (const name of touchedFields) {
    if (fieldBehaviors[name]?.requiresReauthorization) return true;
  }
  return false;
}

/** Labels of the touched fields that trigger reauthorization, for microcopy. */
export function reauthorizationFieldLabels(
  fieldBehaviors: Partial<Record<string, ConnectionFieldBehavior>>,
  labels: Record<string, string>,
  touchedFields: Iterable<string>
): string[] {
  const result: string[] = [];
  for (const name of touchedFields) {
    if (fieldBehaviors[name]?.requiresReauthorization) {
      result.push(labels[name] ?? name.replace(/_/g, ' '));
    }
  }
  return result;
}
