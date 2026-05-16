import { ReportDefinition, ReportSource } from './types';

function isObjectModelSource(
  source: ReportSource | undefined
): source is ReportSource {
  return Boolean(
    source?.schema && (!source.kind || source.kind === 'object_model')
  );
}

function withSourceConnection(
  source: ReportSource,
  connectionId: string
): ReportSource {
  if (!isObjectModelSource(source)) return source;
  return {
    ...source,
    connectionId: source.connectionId ?? connectionId,
    join: source.join?.map((join) => ({
      ...join,
      connectionId: join.connectionId ?? connectionId,
    })),
  };
}

export function withDefaultObjectModelConnection(
  definition: ReportDefinition,
  connectionId?: string | null
): ReportDefinition {
  if (!connectionId) return definition;

  return {
    ...definition,
    datasets: definition.datasets?.map((dataset) => ({
      ...dataset,
      source: {
        ...dataset.source,
        connectionId: dataset.source.connectionId ?? connectionId,
      },
    })),
    blocks: definition.blocks.map((block) => ({
      ...block,
      source: withSourceConnection(block.source, connectionId),
    })),
  };
}
