export function inlineOptions(
  schema: Record<string, unknown> | null | undefined,
  field?: string | null,
  infer = false
): { field: string; values: unknown[] } | null {
  const fields = (schema?.properties ?? schema ?? {}) as Record<
    string,
    { enum?: unknown[] }
  >;
  const name =
    field ??
    (infer
      ? Object.keys(fields).find((k) => Array.isArray(fields[k]?.enum))
      : undefined);
  return name && Array.isArray(fields[name]?.enum)
    ? { field: name, values: fields[name].enum! }
    : null;
}
