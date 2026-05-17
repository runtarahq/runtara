import { Schema } from '@/generated/RuntaraRuntimeApi';
import { ReportDefinition } from '../../types';
import { BlockListV2 } from './BlockListV2';
import { FiltersEditorV2 } from './FiltersEditorV2';
import { DatasetsEditorV2 } from './DatasetsEditorV2';
import { ViewsEditorV2 } from './ViewsEditorV2';

interface ReportBuilderWizardV2Props {
  definition: ReportDefinition;
  schemas: Schema[];
  editing?: boolean;
  onChange: (definition: ReportDefinition) => void;
}

/** Wizard v2 — operates on `ReportDefinition` directly. No intermediate
 *  state model. Each editor receives slices of the definition and emits a
 *  full updated definition back. */
export function ReportBuilderWizardV2({
  definition,
  schemas,
  editing = true,
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
          <h2 className="text-sm font-semibold">Blocks</h2>
          <p className="text-xs text-muted-foreground">
            Each block is a unit of content — text, chart, table, metric, etc.
            Reorder, edit, or delete inline.
          </p>
        </header>
        <BlockListV2
          definition={definition}
          schemas={schemas}
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
        <FiltersEditorV2 definition={definition} onChange={onChange} />
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
