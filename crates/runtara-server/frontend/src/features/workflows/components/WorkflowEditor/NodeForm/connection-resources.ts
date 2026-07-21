export interface ConnectionResourceDefinition {
  name: string;
  description?: string;
}

/**
 * Return the connection-local model catalog advertised by its extractor.
 * Callers use the advertised name verbatim; they do not infer a provider.
 */
export function findModelResourceName(resources: unknown): string | null {
  if (!Array.isArray(resources)) return null;

  const names = resources
    .map((resource) =>
      resource &&
      typeof resource === 'object' &&
      typeof (resource as ConnectionResourceDefinition).name === 'string'
        ? (resource as ConnectionResourceDefinition).name
        : null
    )
    .filter((name): name is string => Boolean(name));

  return (
    names.find((name) => name === 'models') ??
    names.find((name) => name.endsWith('.models')) ??
    null
  );
}
