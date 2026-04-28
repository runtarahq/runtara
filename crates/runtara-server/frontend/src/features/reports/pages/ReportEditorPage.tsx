import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { Save } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Textarea } from '@/shared/components/ui/textarea';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { TileList, TilesPage } from '@/shared/components/tiles-page';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useObjectSchemaDtos } from '@/features/objects/hooks/useObjectSchemas';
import {
  useCreateReport,
  useReport,
  useUpdateReport,
} from '../hooks/useReports';
import { ReportDefinition, ReportStatus } from '../types';
import { slugify } from '../utils';

const EMPTY_DEFINITION: ReportDefinition = {
  definitionVersion: 1,
  markdown: '# Report\n\n{{ block.records }}',
  filters: [],
  blocks: [],
};

export function ReportEditorPage() {
  const { reportId } = useParams();
  const isEditing = Boolean(reportId);
  const navigate = useNavigate();
  const { data: existingReport, isFetching } = useReport(reportId);
  const { data: schemas = [] } = useObjectSchemaDtos();
  const createReport = useCreateReport();
  const updateReport = useUpdateReport();

  usePageTitle(isEditing ? 'Edit Report' : 'New Report');

  const [name, setName] = useState('');
  const [slug, setSlug] = useState('');
  const [description, setDescription] = useState('');
  const [status, setStatus] = useState<ReportStatus>('published');
  const [definitionText, setDefinitionText] = useState(
    JSON.stringify(EMPTY_DEFINITION, null, 2)
  );
  const [selectedSchema, setSelectedSchema] = useState('');
  const [localError, setLocalError] = useState<string | null>(null);

  useEffect(() => {
    if (!existingReport) return;
    setName(existingReport.name);
    setSlug(existingReport.slug);
    setDescription(existingReport.description ?? '');
    setStatus(existingReport.status);
    setDefinitionText(JSON.stringify(existingReport.definition, null, 2));
  }, [existingReport]);

  useEffect(() => {
    if (selectedSchema || schemas.length === 0) return;
    setSelectedSchema(schemas[0]?.name ?? '');
  }, [schemas, selectedSchema]);

  const parsedDefinition = useMemo(() => {
    try {
      return JSON.parse(definitionText) as ReportDefinition;
    } catch {
      return null;
    }
  }, [definitionText]);

  const canSave =
    name.trim().length > 0 &&
    slug.trim().length > 0 &&
    parsedDefinition !== null &&
    !createReport.isPending &&
    !updateReport.isPending;

  const handleNameChange = (value: string) => {
    setName(value);
    if (!isEditing || slug.length === 0) {
      setSlug(slugify(value));
    }
  };

  const handleGenerateStarter = () => {
    const schema = schemas.find(
      (candidate) => candidate.name === selectedSchema
    );
    if (!schema) return;

    const columns = schema.columns
      .filter((column) => column.name !== 'id')
      .slice(0, 6)
      .map((column) => ({
        field: column.name,
        label: column.name
          .split(/[_-]/)
          .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
          .join(' '),
      }));

    const starter: ReportDefinition = {
      definitionVersion: 1,
      markdown: `# ${name || schema.name}\n\n{{ block.total_records }}\n\n{{ block.records }}`,
      filters: [],
      blocks: [
        {
          id: 'total_records',
          type: 'metric',
          title: 'Total records',
          source: {
            schema: schema.name,
            mode: 'aggregate',
            aggregates: [{ alias: 'value', op: 'count' }],
          },
          metric: {
            valueField: 'value',
            label: 'Total records',
            format: 'number',
          },
        },
        {
          id: 'records',
          type: 'table',
          title: 'Records',
          lazy: false,
          source: {
            schema: schema.name,
            mode: 'filter',
          },
          table: {
            columns,
            pagination: {
              defaultPageSize: 50,
              allowedPageSizes: [25, 50, 100],
            },
          },
        },
      ],
    };

    setDefinitionText(JSON.stringify(starter, null, 2));
    setLocalError(null);
  };

  const handleSave = async () => {
    if (!parsedDefinition) {
      setLocalError('Report definition JSON is invalid.');
      return;
    }

    setLocalError(null);
    const payload = {
      name: name.trim(),
      slug: slug.trim(),
      description: description.trim() || null,
      tags: [],
      status,
      definition: parsedDefinition,
    };

    if (isEditing && reportId) {
      const report = await updateReport.mutateAsync({
        id: reportId,
        data: payload,
      });
      navigate(`/reports/${report.id}`);
    } else {
      const report = await createReport.mutateAsync(payload);
      navigate(`/reports/${report.id}`);
    }
  };

  if (isEditing && isFetching) {
    return (
      <TilesPage kicker="Reports" title="Loading report">
        <TileList>
          <div className="h-96 animate-pulse rounded-xl bg-muted/30" />
        </TileList>
      </TilesPage>
    );
  }

  return (
    <TilesPage
      kicker="Reports"
      title={isEditing ? 'Edit report' : 'New report'}
      action={
        <div className="flex w-full flex-col gap-2 sm:w-auto sm:flex-row">
          <Link
            to={isEditing && reportId ? `/reports/${reportId}` : '/reports'}
          >
            <Button
              variant="outline"
              className="h-11 w-full rounded-full sm:px-5"
            >
              Cancel
            </Button>
          </Link>
          <Button
            className="h-11 rounded-full sm:px-5"
            disabled={!canSave}
            onClick={handleSave}
          >
            <Save className="mr-2 h-4 w-4" />
            Save
          </Button>
        </div>
      }
    >
      <div className="grid gap-5 xl:grid-cols-[minmax(280px,360px)_1fr]">
        <section className="space-y-4 rounded-lg border bg-background p-4">
          <div className="space-y-2">
            <Label htmlFor="report-name">Name</Label>
            <Input
              id="report-name"
              value={name}
              onChange={(event) => handleNameChange(event.target.value)}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="report-slug">Slug</Label>
            <Input
              id="report-slug"
              value={slug}
              onChange={(event) => setSlug(slugify(event.target.value))}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="report-description">Description</Label>
            <Textarea
              id="report-description"
              value={description}
              onChange={(event) => setDescription(event.target.value)}
            />
          </div>
          <div className="space-y-2">
            <Label>Status</Label>
            <Select
              value={status}
              onValueChange={(value) => setStatus(value as ReportStatus)}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="draft">Draft</SelectItem>
                <SelectItem value="published">Published</SelectItem>
                <SelectItem value="archived">Archived</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-2 rounded-lg bg-muted/30 p-3">
            <Label>Starter schema</Label>
            <Select value={selectedSchema} onValueChange={setSelectedSchema}>
              <SelectTrigger>
                <SelectValue placeholder="Select schema" />
              </SelectTrigger>
              <SelectContent>
                {schemas.map((schema) => (
                  <SelectItem key={schema.id} value={schema.name}>
                    {schema.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              type="button"
              variant="outline"
              className="w-full"
              disabled={!selectedSchema}
              onClick={handleGenerateStarter}
            >
              Generate starter definition
            </Button>
          </div>
        </section>

        <section className="space-y-3 rounded-lg border bg-background p-4">
          <div>
            <Label htmlFor="report-definition">Definition JSON</Label>
            <p className="mt-1 text-xs text-muted-foreground">
              Markdown and typed blocks are saved as the report definition.
            </p>
          </div>
          <Textarea
            id="report-definition"
            className="min-h-[560px] font-mono text-sm"
            value={definitionText}
            onChange={(event) => setDefinitionText(event.target.value)}
          />
          {(localError || parsedDefinition === null) && (
            <p className="text-sm text-destructive">
              {localError ?? 'Report definition JSON is invalid.'}
            </p>
          )}
        </section>
      </div>
    </TilesPage>
  );
}
