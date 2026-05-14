import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, CheckCircle2 } from 'lucide-react';
import { cn } from '@/lib/utils';
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from '@/shared/components/ui/alert';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import {
  ReportBlockResult,
  ReportDatasetDefinition,
  ReportDefinition,
} from '../../types';
import {
  WizardBlock,
  WizardFilter,
  WizardGrid,
  WizardState,
} from './wizardTypes';
import {
  definitionToWizardState,
  wizardStateToDefinition,
} from './wizardSerialization';
import { BlocksStep } from './steps/BlocksStep';
import { ControlsStep } from './steps/ControlsStep';
import { DatasetsStep } from './steps/DatasetsStep';

interface ReportBuilderWizardProps {
  definition: ReportDefinition;
  schemas: Schema[];
  /** Per-block live results, keyed by block id. Empty while preview hasn't returned yet. */
  blockResults?: Record<string, ReportBlockResult>;
  /** When false, hides every editing control — sections still render with real data
   *  so the same DOM is reused for view mode. Defaults to true. */
  editing?: boolean;
  onChange: (definition: ReportDefinition) => void;
}

function schemaFieldsByName(schemas: Schema[]): Record<string, string[]> {
  return Object.fromEntries(
    schemas.map((schema) => [
      schema.name,
      schema.columns.map((column) => column.name),
    ])
  );
}

function readinessChecks(
  state: WizardState
): Array<{ label: string; ok: boolean }> {
  const datasetIds = new Set(state.datasets.map((d) => d.id));

  const allBlocksReady = state.blocks.every((block) => {
    if (block.type === 'markdown') return Boolean(block.markdownContent);
    if (block.dataset) {
      const dimensions = block.dataset.dimensions ?? [];
      const measures = block.dataset.measures ?? [];
      return (
        datasetIds.has(block.dataset.id) &&
        dimensions.length + measures.length > 0
      );
    }
    if (!block.schema) return false;
    if (block.type === 'metric') {
      const op = block.metricAggregate ?? 'count';
      return op === 'count' || Boolean(block.metricField);
    }
    if (block.type === 'chart') return Boolean(block.chartGroupBy);
    return block.fields.length > 0;
  });

  const datasetsReady = state.datasets.every(
    (dataset) =>
      Boolean(dataset.source.schema) &&
      dataset.dimensions.length + dataset.measures.length > 0
  );

  return [
    {
      label:
        state.blocks.length === 0
          ? 'Add at least one block'
          : 'Each block has a data source and content',
      ok: state.blocks.length > 0 && allBlocksReady,
    },
    {
      label:
        state.datasets.length === 0
          ? "No datasets configured — that's fine"
          : 'Every dataset has a source and at least one field',
      ok: datasetsReady,
    },
    {
      label:
        state.filters.length === 0
          ? "No filters configured — that's fine"
          : 'Every filter is connected',
      ok:
        state.filters.length === 0 ||
        state.filters.every((filter) => filter.target !== '__none__'),
    },
  ];
}

export function ReportBuilderWizard({
  definition,
  schemas,
  blockResults,
  editing = true,
  onChange,
}: ReportBuilderWizardProps) {
  const fallbackSchema = schemas[0]?.name ?? '';

  const initial = useMemo(
    () => definitionToWizardState(definition, fallbackSchema),
    // Compute once on mount; later edits update state directly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    []
  );

  const [state, setState] = useState<WizardState>(initial.state);
  const [compatibility] = useState(initial.compatibility);

  const onChangeRef = useRef(onChange);
  const definitionRef = useRef(definition);
  const schemasRef = useRef(schemas);
  useEffect(() => {
    onChangeRef.current = onChange;
    definitionRef.current = definition;
    schemasRef.current = schemas;
  });

  const commit = useCallback((nextState: WizardState) => {
    setState(nextState);
    const fieldsByName = schemaFieldsByName(schemasRef.current);
    const nextDefinition = wizardStateToDefinition(
      nextState,
      fieldsByName,
      definitionRef.current
    );
    onChangeRef.current(nextDefinition);
  }, []);

  // Adopt the first available schema as defaultSchema once data loads.
  useEffect(() => {
    if (!state.defaultSchema && fallbackSchema) {
      commit({ ...state, defaultSchema: fallbackSchema });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fallbackSchema]);

  function setBlocks(blocks: WizardBlock[]) {
    commit({ ...state, blocks });
  }

  function setGrids(grids: WizardGrid[]) {
    commit({ ...state, grids });
  }

  function setGridsAndBlocks(grids: WizardGrid[], blocks: WizardBlock[]) {
    commit({ ...state, grids, blocks });
  }

  function setFilters(filters: WizardFilter[]) {
    commit({ ...state, filters });
  }

  function setDatasets(datasets: ReportDatasetDefinition[]) {
    commit({ ...state, datasets });
  }

  const checks = readinessChecks(state);
  const allReady = checks.every((check) => check.ok);

  return (
    <div className="flex flex-col gap-5">
      {!compatibility.fullyEditable && (
        <Alert>
          <AlertTriangle className="h-4 w-4" />
          <AlertTitle>This report uses advanced features</AlertTitle>
          <AlertDescription>
            <p className="mb-1">
              The editor doesn't expose all of these, but they're preserved in
              the saved report and continue to render:
            </p>
            <ul className="list-disc pl-5 text-sm">
              {compatibility.reasons.map((reason) => (
                <li key={reason}>{reason}</li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      )}

      {/* The Layout section keeps the SAME DOM in both view and edit modes —
       *  editing just adds the toolbars and config affordances inside it. */}
      <section>
        <BlocksStep
          grids={state.grids}
          blocks={state.blocks}
          schemas={schemas}
          defaultSchema={state.defaultSchema}
          datasets={state.datasets}
          filters={state.filters}
          blockResults={blockResults}
          editing={editing}
          onGridsChange={setGrids}
          onBlocksChange={setBlocks}
          onGridsAndBlocksChange={setGridsAndBlocks}
        />
      </section>

      {editing ? (
        <section className="mt-6 border-t pt-4">
          <header className="mb-3">
            <h2 className="text-sm font-semibold">Datasets</h2>
            <p className="text-xs text-muted-foreground">
              Pre-aggregated semantic data sources with named dimensions and
              measures. Blocks pick a dataset to query instead of going to a
              schema directly.
            </p>
          </header>
          <DatasetsStep
            datasets={state.datasets}
            schemas={schemas}
            defaultSchema={state.defaultSchema}
            onChange={setDatasets}
          />
        </section>
      ) : null}

      {editing ? (
        <section className="mt-6 border-t pt-4">
          <header className="mb-3">
            <h2 className="text-sm font-semibold">Viewer filters</h2>
            <p className="text-xs text-muted-foreground">
              Optional controls that appear at the top of the report and
              constrain the blocks you target.
            </p>
          </header>
          <ControlsStep
            filters={state.filters}
            blocks={state.blocks}
            schemas={schemas}
            onChange={setFilters}
          />
        </section>
      ) : null}

      {editing ? (
        <section className="mt-6 border-t pt-4">
          <header className="mb-2 flex items-center gap-2">
            {allReady ? (
              <CheckCircle2 className="h-4 w-4 text-emerald-600" />
            ) : (
              <AlertTriangle className="h-4 w-4 text-amber-600" />
            )}
            <h2 className="text-sm font-semibold">
              {allReady ? 'Ready to save' : 'A few things to finish'}
            </h2>
          </header>
          <ul className="grid gap-1.5 text-sm">
            {checks.map((check) => (
              <li
                key={check.label}
                className={cn(
                  'flex items-center gap-2',
                  check.ok
                    ? 'text-foreground'
                    : 'text-amber-700 dark:text-amber-300'
                )}
              >
                {check.ok ? (
                  <CheckCircle2 className="h-3.5 w-3.5 text-emerald-600" />
                ) : (
                  <AlertTriangle className="h-3.5 w-3.5 text-amber-600" />
                )}
                {check.label}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
