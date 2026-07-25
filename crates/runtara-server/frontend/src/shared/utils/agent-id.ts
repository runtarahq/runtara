/**
 * Canonical agent id folding — the frontend half of the rule the backend
 * already applies everywhere.
 *
 * Kebab is canonical: the component dispatcher forces each `meta.json` `id` to
 * kebab, `GET /api/runtime/agents` advertises kebab, and WASM component
 * packages are named `runtara:agent-<kebab>`. Workflow JSON authored against an
 * older snake_case id (`object_model`) — or with stray capitalization — is
 * still accepted by the runtime, the compiler and the MCP authoring tools,
 * which all fold it through `canonical_agent_id`
 * (`crates/runtara-dsl/src/agent_meta.rs`).
 *
 * Without the same fold here, `agents.find((a) => a.id === step.agentId)`
 * misses for every such step, the capability metadata is discarded, and the
 * schema-driven form silently degrades to an untyped key/value table with no
 * labels, descriptions, required markers or optional-field discovery — while
 * the step itself runs perfectly well.
 *
 * Mirrors `id.to_ascii_lowercase().replace('_', "-")`. Deliberately ASCII-only:
 * `String.prototype.toLowerCase` is Unicode-aware (`I` folds to `ı` under a
 * Turkish locale in some engines) and would drift from the Rust definition.
 */
export function canonicalAgentId(id: string | null | undefined): string {
  if (!id) return '';
  let out = '';
  for (const ch of id) {
    if (ch === '_') {
      out += '-';
    } else if (ch >= 'A' && ch <= 'Z') {
      out += String.fromCharCode(ch.charCodeAt(0) + 32);
    } else {
      out += ch;
    }
  }
  return out;
}

/** Whether two agent ids refer to the same agent (see {@link canonicalAgentId}). */
export function sameAgentId(
  a: string | null | undefined,
  b: string | null | undefined
): boolean {
  const left = canonicalAgentId(a);
  return left !== '' && left === canonicalAgentId(b);
}

/**
 * Find an agent in the catalog by id, folding both sides. Use this instead of
 * `agents.find((a) => a.id === agentId)` at every call site.
 */
export function findAgentById<T extends { id: string }>(
  agents: readonly T[] | null | undefined,
  agentId: string | null | undefined
): T | undefined {
  if (!agents || !agentId) return undefined;
  const target = canonicalAgentId(agentId);
  if (target === '') return undefined;
  return agents.find((agent) => canonicalAgentId(agent.id) === target);
}
