/**
 * Carrying condition operands across an operator change.
 *
 * `handleOperatorChange` used to rebuild the argument list purely from the new
 * operator's arity, ignoring whatever was already there. Switching Equals →
 * Not equals wiped both sides; switching Logical AND → Logical OR discarded
 * every nested sub-condition. Both are one click, and the editor has no undo.
 *
 * Changing the operator expresses an intent about the *comparison*, never
 * about the operands. Carry them across, truncating only when the new arity
 * genuinely holds fewer, and padding with empty immediates when it holds more.
 */

export type ConditionArity = 'UNARY' | 'BINARY' | 'VARIADIC';

/** A fresh, empty operand. */
export function emptyConditionArgument<T>(): T {
  return { valueType: 'immediate', value: '', immediateType: 'string' } as T;
}

/** How many operands an arity requires, or null when unbounded. */
function requiredCount(arity: ConditionArity): number | null {
  switch (arity) {
    case 'UNARY':
      return 1;
    case 'BINARY':
      return 2;
    default:
      return null; // VARIADIC
  }
}

/**
 * The operand list to use after switching to an operator of `arity`.
 *
 * VARIADIC keeps everything (at least one slot); UNARY and BINARY keep the
 * leading operands and pad the rest. Truncation is the only lossy case and is
 * unavoidable — a unary operator has nowhere to put a second operand.
 */
export function carryConditionArgs<T>(
  existing: readonly T[] | undefined,
  arity: ConditionArity
): T[] {
  const args = Array.isArray(existing) ? [...existing] : [];
  const needed = requiredCount(arity);

  if (needed === null) {
    return args.length > 0 ? args : [emptyConditionArgument<T>()];
  }

  const carried = args.slice(0, needed);
  while (carried.length < needed) {
    carried.push(emptyConditionArgument<T>());
  }
  return carried;
}

/**
 * Whether switching to `arity` will drop operands the author entered.
 * Callers can use this to warn instead of silently truncating.
 */
export function operatorChangeDropsArgs(
  existing: readonly unknown[] | undefined,
  arity: ConditionArity
): boolean {
  const needed = requiredCount(arity);
  if (needed === null || !Array.isArray(existing)) return false;
  return existing.length > needed;
}
