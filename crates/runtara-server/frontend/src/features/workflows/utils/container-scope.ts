/**
 * Container-scope resolution for the workflow editor, computed on the FLAT
 * store nodes (each node carries parentId), not the composed execution graph
 * (which nests subgraphs and defeats naive lookups).
 *
 * The runtime scopes bare `data.*` per container chain (see
 * crates/runtara-workflows/src/validation.rs DataScope):
 * - a Split body rebinds `data.*` to the current iteration item, validated
 *   against the Split's declared iteration schema (for_split_body);
 * - While bodies pass the enclosing data scope through unchanged
 *   (for_while_body) — so a step inside Split > While > … is still in the
 *   Split's item scope;
 * - WaitForSignal onWait bodies anchor to the onWait graph's own schema,
 *   which the editor never declares — statically unknowable.
 */
import { parseSchema } from './schema';
import type { SchemaField } from '../components/WorkflowEditor/EditorSidebar/SchemaFieldsEditor';

/** Minimal structural shape of a workflow-store node. */
export interface ScopeNode {
  id: string;
  parentId?: string;
  data?: {
    stepType?: string;
    inputSchema?: Record<string, unknown>;
  };
}

export type DataScope =
  | { kind: 'workflow' }
  | { kind: 'split'; splitNode: ScopeNode }
  | { kind: 'unknown' };

export interface ContainerScope {
  /** Enclosing containers, innermost first. Empty at top level. */
  chain: ScopeNode[];
  /** What bare `data.*` resolves to at this position. */
  dataScope: DataScope;
  /** loop.* is in scope (immediate container is a While body). */
  isInsideWhileLoop: boolean;
  /** Split iteration scope (transitively through While bodies). */
  isInsideSplit: boolean;
  /** WaitForSignal onWait scope (immediate container). */
  isInsideWaitScope: boolean;
}

const TOP_LEVEL_SCOPE: ContainerScope = {
  chain: [],
  dataScope: { kind: 'workflow' },
  isInsideWhileLoop: false,
  isInsideSplit: false,
  isInsideWaitScope: false,
};

/**
 * Resolves the scope for a position whose enclosing container is
 * `containerId` (undefined = top level). Walks parentId links upward.
 */
export function resolveContainerScope(
  nodes: ScopeNode[],
  containerId: string | undefined
): ContainerScope {
  if (!containerId) {
    return TOP_LEVEL_SCOPE;
  }

  const byId = new Map(nodes.map((n) => [n.id, n]));
  const chain: ScopeNode[] = [];
  const seen = new Set<string>();
  let cursor: string | undefined = containerId;
  while (cursor && !seen.has(cursor) && chain.length < 32) {
    seen.add(cursor);
    const node = byId.get(cursor);
    if (!node) {
      break;
    }
    chain.push(node);
    cursor = node.parentId;
  }

  let dataScope: DataScope = { kind: 'workflow' };
  for (const node of chain) {
    const stepType = node.data?.stepType;
    if (stepType === 'While') {
      // While bodies inherit the enclosing data scope — keep walking.
      continue;
    }
    if (stepType === 'Split') {
      dataScope = { kind: 'split', splitNode: node };
      break;
    }
    // WaitForSignal onWait (own, undeclared schema) or anything unexpected:
    // not statically checkable.
    dataScope = { kind: 'unknown' };
    break;
  }

  return {
    chain,
    dataScope,
    isInsideWhileLoop: chain[0]?.data?.stepType === 'While',
    isInsideSplit: dataScope.kind === 'split',
    isInsideWaitScope: chain[0]?.data?.stepType === 'WaitForSignal',
  };
}

/**
 * Bridges utils/schema's parsed fields (optional type/required/description)
 * to the editor SchemaField shape the NodeForm context uses.
 */
export function normalizeSchemaField(
  field: import('./schema').SchemaField
): SchemaField {
  return {
    ...field,
    type: field.type ?? 'string',
    required: field.required ?? false,
    description: field.description ?? '',
    properties: field.properties?.map(normalizeSchemaField),
  };
}

/**
 * The declared iteration schema of the scope's governing Split, as editor
 * SchemaFields — or undefined when not in a Split scope / nothing declared.
 */
export function splitItemSchemaFieldsFromScope(
  scope: ContainerScope
): SchemaField[] | undefined {
  if (scope.dataScope.kind !== 'split') {
    return undefined;
  }
  const schema = scope.dataScope.splitNode.data?.inputSchema;
  if (!schema || Object.keys(schema).length === 0) {
    return undefined;
  }
  try {
    const fields = parseSchema(schema).map(normalizeSchemaField);
    return fields.length > 0 ? fields : undefined;
  } catch {
    return undefined;
  }
}
