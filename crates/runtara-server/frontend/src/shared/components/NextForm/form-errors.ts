/**
 * Walk a react-hook-form `errors` tree and describe the first real failure.
 *
 * `handleSubmit(onSubmit)` with no second argument silently drops a blocked
 * submit: the button appears inert and nothing anywhere says why. This turns
 * the error tree into something a person can act on — the message plus a
 * human-readable field label.
 *
 * The tree mixes shapes: leaves carry `{ message, type }`, arrays hold sparse
 * entries (index 3 failing with 0-2 empty), and `root` holds array-level
 * errors. Order is object key order, which for RHF follows registration order,
 * so "first" is stable and matches reading order closely enough to be useful.
 */

export interface FirstFormError {
  /** Dotted path, e.g. `inputMapping.2.value`. */
  path: string;
  /** Validation message to show. */
  message: string;
  /** Human-readable field label derived from the path. */
  label: string;
}

interface ErrorLeaf {
  message?: unknown;
  type?: unknown;
}

function isErrorLeaf(node: unknown): node is ErrorLeaf {
  return (
    typeof node === 'object' &&
    node !== null &&
    'message' in node &&
    typeof (node as ErrorLeaf).message === 'string' &&
    (node as ErrorLeaf).message !== ''
  );
}

/**
 * Turn `inputMapping.2.value` into `Input mapping, item 3 — value`, and
 * `executionTimeout` into `Execution timeout`.
 */
export function humanizeFieldPath(path: string): string {
  const parts = path.split('.').filter(Boolean);
  const rendered: string[] = [];

  for (const part of parts) {
    if (part === 'root') continue;
    if (/^\d+$/.test(part)) {
      // 1-based: authors count rows from one.
      rendered.push(`item ${Number(part) + 1}`);
      continue;
    }
    rendered.push(
      part
        // camelCase -> spaced
        .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
        // snake_case / kebab-case -> spaced
        .replace(/[_-]+/g, ' ')
        .toLowerCase()
    );
  }

  if (rendered.length === 0) return path;
  const [head, ...rest] = rendered;
  const capitalized = head.charAt(0).toUpperCase() + head.slice(1);
  return rest.length > 0 ? `${capitalized} — ${rest.join(', ')}` : capitalized;
}

export function describeFirstFormError(
  errors: unknown,
  basePath = ''
): FirstFormError | null {
  if (!errors || typeof errors !== 'object') return null;

  // A leaf at this level wins over anything nested beneath it.
  if (basePath && isErrorLeaf(errors)) {
    return {
      path: basePath,
      message: String((errors as ErrorLeaf).message),
      label: humanizeFieldPath(basePath),
    };
  }

  // Field-array errors are genuinely SPARSE — RHF leaves holes for the rows
  // that passed, so `inputMapping` for a failure on row 6 is `[ <6 empty>, {…} ]`.
  // `Array.prototype.map` preserves holes and `for…of` visits them as
  // `undefined`, which then throws on destructuring. Index explicitly instead.
  if (Array.isArray(errors)) {
    for (let index = 0; index < errors.length; index += 1) {
      const value = errors[index];
      if (value === undefined || value === null) continue;
      const path = basePath ? `${basePath}.${index}` : String(index);
      const found = describeFirstFormError(value, path);
      if (found) return found;
    }
    return null;
  }

  for (const [key, value] of Object.entries(errors as Record<string, unknown>)) {
    if (value === undefined || value === null) continue;
    // `ref`/`types` are RHF bookkeeping, not nested errors.
    if (key === 'ref' || key === 'types') continue;
    const path = basePath ? `${basePath}.${key}` : key;
    const found = describeFirstFormError(value, path);
    if (found) return found;
  }

  return null;
}
