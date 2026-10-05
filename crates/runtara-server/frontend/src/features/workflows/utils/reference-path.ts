/**
 * Building and splitting reference paths (`data.order.id`,
 * `steps['fetch'].outputs.body`, `data["a.b"]`) the way the backend tokenizer
 * reads them (`tokenize_reference` in
 * crates/runtara-workflow-stdlib/src/reference_path.rs).
 *
 * A dot separates segments. A `[..]` body is one segment: a quoted body
 * (`"..."` or `'...'`) is an opaque key that runs to its closing quote, so
 * dots and brackets inside it belong to the key; an unquoted body ends at the
 * first `]`. There is no escaping — a key containing one quote character is
 * written with the other.
 */

/**
 * Names written in dotted form. Anything else is bracket-quoted, which keeps
 * a schema field named `a.b`, `a]b` or `a[0]` a single key instead of a path
 * that splits or fails validation. Same charset as step ids.
 */
const PLAIN_SEGMENT = /^[A-Za-z0-9_-]+$/;

/** Characters the tokenizer treats as structure outside a quoted key. */
const STRUCTURAL = /[.[\]]/;

/**
 * Appends one key to a reference path: `.name` for plain names, otherwise
 * `["name"]` (or `['name']` when the name contains `"`).
 *
 * Returns null when no spelling reads back as exactly this key, and callers
 * leave such a field out rather than insert a path to some other key. That is
 * the empty name, and a name containing both quote characters plus `.`, `[` or
 * `]`. With no escaping, its own quotes would end a quoted key early:
 * `a"]["b'c` written as `data["a"]["b'c"]` reads as the key `a`, then `b'c`.
 */
export function appendPathSegment(path: string, name: string): string | null {
  if (!name) {
    return null;
  }
  if (PLAIN_SEGMENT.test(name)) {
    return `${path}.${name}`;
  }
  if (!name.includes('"')) {
    return `${path}["${name}"]`;
  }
  if (!name.includes("'")) {
    return `${path}['${name}']`;
  }
  // Both quote characters: no quoted spelling. The dotted form reads back as
  // this one key unless the name has structure of its own.
  return STRUCTURAL.test(name) ? null : `${path}.${name}`;
}

/**
 * Splits a reference path into lookup segments, mirroring the backend
 * tokenizer's segments (its defect reporting is not mirrored). Empty segments
 * are dropped, a stray `]` is kept as key text, and an unterminated `[` takes
 * the rest of the path as its body.
 */
export function referenceSegments(path: string): string[] {
  const segments: string[] = [];
  let current = '';
  let index = 0;

  const flush = () => {
    if (current) {
      segments.push(current);
      current = '';
    }
  };

  while (index < path.length) {
    const ch = path[index];
    if (ch === '.') {
      flush();
      index += 1;
    } else if (ch === '[') {
      flush();
      const quoted = readQuotedKey(path, index + 1);
      if (quoted) {
        if (quoted.key) {
          segments.push(quoted.key);
        }
        index = quoted.end;
        continue;
      }
      const close = path.indexOf(']', index + 1);
      const body = path.slice(index + 1, close === -1 ? undefined : close);
      const key = stripMatchingQuotes(body.trim());
      if (key) {
        segments.push(key);
      }
      index = close === -1 ? path.length : close + 1;
    } else {
      current += ch;
      index += 1;
    }
  }

  flush();
  return segments;
}

/**
 * Reads a well-formed quoted key starting right after a `[`: optional
 * whitespace, a quote, the key, the same quote, optional whitespace, `]`.
 * Returns null when the body is unquoted or malformed, in which case it is
 * read up to the first `]` instead.
 */
function readQuotedKey(
  path: string,
  start: number
): { key: string; end: number } | null {
  let open = start;
  while (open < path.length && /\s/.test(path[open])) {
    open += 1;
  }
  const quote = path[open];
  if (quote !== '"' && quote !== "'") {
    return null;
  }
  const closeQuote = path.indexOf(quote, open + 1);
  if (closeQuote === -1) {
    return null;
  }
  let after = closeQuote + 1;
  while (after < path.length && /\s/.test(path[after])) {
    after += 1;
  }
  if (path[after] !== ']') {
    return null;
  }
  return { key: path.slice(open + 1, closeQuote), end: after + 1 };
}

/** Strips one layer of quotes when the body opens and closes with the same one. */
function stripMatchingQuotes(body: string): string {
  const quote = body[0];
  if (
    body.length >= 2 &&
    (quote === '"' || quote === "'") &&
    body.endsWith(quote)
  ) {
    return body.slice(1, -1);
  }
  return body;
}
