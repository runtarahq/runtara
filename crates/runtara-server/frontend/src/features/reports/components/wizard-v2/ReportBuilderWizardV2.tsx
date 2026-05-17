import { Schema } from '@/generated/RuntaraRuntimeApi';
import { ReportBlockResult, ReportDefinition } from '../../types';
import { FiltersEditorV2 } from './FiltersEditorV2';
import { DatasetsEditorV2 } from './DatasetsEditorV2';
import { GridContainer } from './GridContainer';
import { ViewsEditorV2 } from './ViewsEditorV2';

interface ReportBuilderWizardV2Props {
  definition: ReportDefinition;
  schemas: Schema[];
  editing?: boolean;
  /** Live block-preview results keyed by block id. Lets `BlockHostInEdit`
   *  render the block exactly as the viewer would. */
  blockResults?: Partial<Record<string, ReportBlockResult>>;
  /** When set, viewer-side filter values for hydrated previews. */
  filters?: Record<string, unknown>;
  /** Existing report id (for `ReportBlockHost` to issue block-data
   *  requests). Undefined on the new-report page. */
  reportId?: string;
  onChange: (definition: ReportDefinition) => void;
}

/** Wizard v2 — operates on `ReportDefinition` directly. No intermediate
 *  state model. Each editor receives slices of the definition and emits a
 *  full updated definition back. */
export function ReportBuilderWizardV2({
  definition,
  schemas,
  editing = true,
  blockResults,
  filters,
  reportId,
  onChange,
}: ReportBuilderWizardV2Props) {
  if (!editing) {
    // View mode: the ReportRenderer renders the saved report directly. The
    // wizard only mounts in edit mode in ReportPage, so this branch is a
    // defensive fallback.
    return null;
  }

  return (
    <div className="flex flex-col gap-5">
      <section>
        <header className="mb-3">
          <h2 className="text-sm font-semibold">Layout</h2>
          <p className="text-xs text-muted-foreground">
            Arrange blocks inside grids. A "section" is a 1-column grid; a
            row of metric blocks is a 1×N grid; everything is a grid.
          </p>
        </header>
        <GridContainer
          definition={definition}
          schemas={schemas}
          blockResults={blockResults}
          reportId={reportId}
          filters={filters ?? {}}
          onChange={onChange}
        />
      </section>

      <section className="border-t pt-4">
        <header className="mb-3">
          <h2 className="text-sm font-semibold">Filters</h2>
          <p className="text-xs text-muted-foreground">
            Controls rendered at the top of the report. Wire each one to the
            blocks it should constrain.
          </p>
        </header>
        <FiltersEditorV2
          definition={definition}
          schemas={schemas}
          onChange={onChange}
        />
      </section>

      <section className="border-t pt-4">
        <header className="mb-3">
          <h2 className="text-sm font-semibold">Datasets</h2>
          <p className="text-xs text-muted-foreground">
            Pre-aggregated semantic data sources. Blocks bind to a dataset
            instead of querying a schema directly.
          </p>
        </header>
        <DatasetsEditorV2
          definition={definition}
          schemas={schemas}
          onChange={onChange}
        />
      </section>

      <section className="border-t pt-4">
        <header className="mb-3">
          <h2 className="text-sm font-semibold">Views</h2>
          <p className="text-xs text-muted-foreground">
            Optional named layouts for drill-downs.
          </p>
        </header>
        <ViewsEditorV2 definition={definition} onChange={onChange} />
      </section>
    </div>
  );
}
