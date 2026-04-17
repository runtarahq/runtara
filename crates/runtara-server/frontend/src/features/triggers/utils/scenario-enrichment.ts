/**
 * Creates a Map of scenario IDs to scenario names for efficient lookup.
 * Used to enrich trigger data with human-readable scenario names.
 */
export function createScenarioNameMap(
  scenarios:
    | { id?: string; name?: string }[]
    | { data?: { id?: string; name?: string }[] }
    | { data?: { content?: { id?: string; name?: string }[] } }
): Map<string, string> {
  const map = new Map<string, string>();
  // Handle direct array, { data: [...] }, and paginated { data: { content: [...] } } formats
  const scenariosAny = scenarios as any;
  const scenarioList =
    scenariosAny?.data?.content || scenariosAny?.data || scenarios || [];
  for (const scenario of scenarioList) {
    if (scenario.id && scenario.name) {
      map.set(scenario.id, scenario.name);
    }
  }
  return map;
}
