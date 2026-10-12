/**
 * Whether a react-hook-form `dirtyFields` tree marks any field as changed.
 *
 * Use this instead of `formState.isDirty`. `isDirty` deep-compares the whole
 * form against its defaults, so a Controller that registers an optional field
 * (e.g. a mapping row's `typeHint`) adds an `undefined` key and flips it to
 * true with no value changed. `dirtyFields` compares per field, where a
 * missing key and an `undefined` value are equal.
 */
export function hasDirtyFields(dirtyFields: unknown): boolean {
  if (dirtyFields === true) return true;
  if (Array.isArray(dirtyFields)) return dirtyFields.some(hasDirtyFields);
  if (dirtyFields && typeof dirtyFields === 'object') {
    return Object.values(dirtyFields).some(hasDirtyFields);
  }
  return false;
}
