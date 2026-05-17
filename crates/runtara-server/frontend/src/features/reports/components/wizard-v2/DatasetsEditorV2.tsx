import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { Plus, Trash2 } from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import {
  ReportDatasetDefinition,
  ReportDefinition,
} from '../../types';

interface DatasetsEditorV2Props {
  definition: ReportDefinition;
  schemas: Schema[];
  onChange: (definition: ReportDefinition) => void;
}

function newDataset(): ReportDatasetDefinition {
  const id = `dataset_${Math.random().toString(36).slice(2, 7)}`;
  return {
    id,
    label: 'New dataset',
    source: { schema: '' },
    dimensions: [],
    measures: [],
  };
}

export function DatasetsEditorV2({
  definition,
  schemas,
  onChange,
}: DatasetsEditorV2Props) {
  const datasets = definition.datasets ?? [];

  const updateDatasets = (next: ReportDatasetDefinition[]) =>
    onChange({ ...definition, datasets: next });

  const updateDataset = (
    id: string,
    updater: (dataset: ReportDatasetDefinition) => ReportDatasetDefinition
  ) =>
    updateDatasets(datasets.map((d) => (d.id === id ? updater(d) : d)));

  return (
    <div className="grid gap-3">
      {datasets.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No datasets yet. Datasets pre-aggregate data so multiple blocks can
          share a query.
        </p>
      ) : (
        <div className="grid gap-3">
          {datasets.map((dataset) => (
            <Card key={dataset.id}>
              <CardHeader className="flex flex-row items-center justify-between gap-2 space-y-0 py-3">
                <CardTitle className="text-sm">{dataset.label}</CardTitle>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7 text-destructive"
                  onClick={() =>
                    updateDatasets(datasets.filter((d) => d.id !== dataset.id))
                  }
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </CardHeader>
              <CardContent className="grid gap-3 pt-0">
                <div className="grid grid-cols-2 gap-3">
                  <div className="grid gap-1.5">
                    <Label className="text-xs">Label</Label>
                    <Input
                      value={dataset.label}
                      onChange={(event) =>
                        updateDataset(dataset.id, (d) => ({
                          ...d,
                          label: event.target.value,
                        }))
                      }
                    />
                  </div>
                  <div className="grid gap-1.5">
                    <Label className="text-xs">Schema</Label>
                    <Select
                      value={dataset.source.schema || ''}
                      onValueChange={(value) =>
                        updateDataset(dataset.id, (d) => ({
                          ...d,
                          source: { ...d.source, schema: value },
                        }))
                      }
                    >
                      <SelectTrigger className="h-9">
                        <SelectValue placeholder="Pick a schema" />
                      </SelectTrigger>
                      <SelectContent>
                        {schemas.map((schema) => (
                          <SelectItem key={schema.name} value={schema.name}>
                            {schema.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                </div>

                <p className="text-xs text-muted-foreground">
                  Dimensions: {dataset.dimensions.length} · Measures:{' '}
                  {dataset.measures.length}. Dimension / measure editing keeps
                  using the legacy wizard until v2 ships those forms.
                </p>
              </CardContent>
            </Card>
          ))}
        </div>
      )}
      <div>
        <Button
          type="button"
          variant="outline"
          onClick={() => updateDatasets([...datasets, newDataset()])}
        >
          <Plus className="mr-1 h-3.5 w-3.5" /> Add dataset
        </Button>
      </div>
    </div>
  );
}
