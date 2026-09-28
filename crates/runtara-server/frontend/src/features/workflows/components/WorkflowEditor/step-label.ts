const GENERATED_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** A step without a name is labelled by its id, unless the id was generated. */
export function stepIdLabel(id?: string): string | undefined {
  return id && !GENERATED_ID.test(id) ? id : undefined;
}
