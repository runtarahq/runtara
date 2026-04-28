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
import { ReportDefinitionBuilder } from '../components/ReportDefinitionBuilder';
import { ReportDefinition, ReportStatus } from '../types';
import {
  extractBlockPlaceholders,
  extractLayoutBlockReferences,
  slugify,
} from '../utils';

const EMPTY_DEFINITION: ReportDefinition = {
  definitionVersion: 1,
  markdown: '# Report',
  layout: [{ id: 'intro', type: 'markdown', content: '# Report' }],
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
  const [definition, setDefinition] =
    useState<ReportDefinition>(EMPTY_DEFINITION);
  const [selectedSchema, setSelectedSchema] = useState('');
  const [localError, setLocalError] = useState<string | null>(null);

  useEffect(() => {
    if (!existingReport) return;
    setName(existingReport.name);
    setSlug(existingReport.slug);
    setDescription(existingReport.description ?? '');
    setStatus(existingReport.status);
    setDefinition(existingReport.definition);
  }, [existingReport]);

  useEffect(() => {
    if (selectedSchema || schemas.length === 0) return;
    setSelectedSchema(schemas[0]?.name ?? '');
  }, [schemas, selectedSchema]);

  const definitionErrors = useMemo(
    () => validateReportDefinition(definition),
    [definition]
  );

  const canSave =
    name.trim().length > 0 &&
    slug.trim().length > 0 &&
    definitionErrors.length === 0 &&
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
      layout: [
        {
          id: 'intro',
          type: 'markdown',
          content: `# ${name || schema.name}`,
        },
        {
          id: 'summary_metrics',
          type: 'metric_row',
          blocks: ['total_records'],
        },
        {
          id: 'records_node',
          type: 'block',
          blockId: 'records',
        },
      ],
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

    setDefinition(starter);
    setLocalError(null);
  };

  const handleSave = async () => {
    if (definitionErrors.length > 0) {
      setLocalError(definitionErrors[0]);
      return;
    }

    setLocalError(null);
    const payload = {
      name: name.trim(),
      slug: slug.trim(),
      description: description.trim() || null,
      tags: [],
      status,
      definition,
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

        <div className="flex flex-col gap-3">
          <ReportDefinitionBuilder
            value={definition}
            schemas={schemas}
            selectedSchema={selectedSchema}
            onSelectedSchemaChange={setSelectedSchema}
            onChange={(nextDefinition) => {
              setDefinition(nextDefinition);
              setLocalError(null);
            }}
          />
          {(localError || definitionErrors.length > 0) && (
            <p className="text-sm text-destructive">
              {localError ?? definitionErrors[0]}
            </p>
          )}
        </div>
      </div>
    </TilesPage>
  );
}

function validateReportDefinition(definition: ReportDefinition): string[] {
  const errors: string[] = [];
  const blockIds = new Set<string>();

  for (const block of definition.blocks) {
    if (!block.id.trim()) {
      errors.push('Every report block needs an ID.');
      continue;
    }
    if (blockIds.has(block.id)) {
      errors.push(`Duplicate report block ID: ${block.id}`);
    }
    blockIds.add(block.id);
    if (!block.source.schema.trim()) {
      errors.push(`Block "${block.id}" needs a schema.`);
    }
  }

  for (const placeholder of extractBlockPlaceholders(definition.markdown)) {
    if (!blockIds.has(placeholder)) {
      errors.push(`Markdown references unknown block: ${placeholder}`);
    }
  }

  for (const blockId of extractLayoutBlockReferences(definition.layout)) {
    if (!blockIds.has(blockId)) {
      errors.push(`Layout references unknown block: ${blockId}`);
    }
  }

  return errors;
}
