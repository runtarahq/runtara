import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { Eye, Save } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { TileList, TilesPage } from '@/shared/components/tiles-page';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useObjectSchemaDtos } from '@/features/objects/hooks/useObjectSchemas';
import {
  useCreateReport,
  useReport,
  useReportPreview,
  useUpdateReport,
  useValidateReport,
} from '../hooks/useReports';
import { ReportDeleteButton } from '../components/ReportDeleteButton';
import { ReportBuilderWizard } from '../components/wizard/ReportBuilderWizard';
import { ReportBlockResult, ReportDefinition } from '../types';
import { slugify } from '../utils';

const EMPTY_DEFINITION: ReportDefinition = {
  definitionVersion: 1,
  layout: [],
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
  const validateReport = useValidateReport();

  usePageTitle(isEditing ? 'Edit Report' : 'New Report');

  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [definition, setDefinition] =
    useState<ReportDefinition>(EMPTY_DEFINITION);
  const [saveError, setSaveError] = useState<string | null>(null);

  // Debounce the definition so preview API isn't spammed on every keystroke.
  const [debouncedDefinition, setDebouncedDefinition] =
    useState<ReportDefinition>(EMPTY_DEFINITION);
  useEffect(() => {
    const handle = setTimeout(() => setDebouncedDefinition(definition), 400);
    return () => clearTimeout(handle);
  }, [definition]);

  // Skip the preview call until at least one block looks queryable. Markdown
  // blocks are fine on their own; data blocks need a schema.
  const canPreview = useMemo(
    () =>
      debouncedDefinition.blocks.some(
        (block) =>
          block.type === 'markdown' ||
          (block.source?.schema && block.source.schema.length > 0)
      ),
    [debouncedDefinition]
  );

  const previewRequest = useMemo(
    () =>
      canPreview
        ? { filters: {}, definition: debouncedDefinition }
        : undefined,
    [canPreview, debouncedDefinition]
  );

  const previewQuery = useReportPreview(previewRequest, canPreview);
  const blockResults: Record<string, ReportBlockResult> = useMemo(
    () => previewQuery.data?.blocks ?? {},
    [previewQuery.data]
  );

  useEffect(() => {
    if (!existingReport) return;
    setName(existingReport.name);
    setDescription(existingReport.description ?? '');
    setDefinition(existingReport.definition);
  }, [existingReport]);

  const canSave =
    name.trim().length > 0 &&
    !createReport.isPending &&
    !updateReport.isPending &&
    !validateReport.isPending;

  const handleSave = async () => {
    setSaveError(null);
    const validation = await validateReport.mutateAsync({ definition });
    if (!validation.valid) {
      setSaveError(validation.errors[0]?.message ?? 'Report is invalid.');
      return;
    }

    const trimmedName = name.trim();
    const payload = {
      name: trimmedName,
      slug: slugify(trimmedName),
      description: description.trim() || null,
      tags: [],
      status: 'published' as const,
      definition,
    };

    if (isEditing && reportId) {
      const report = await updateReport.mutateAsync({
        id: reportId,
        data: payload,
      });
      // Stay in edit mode after save so users can keep iterating.
      navigate(`/reports/${report.id}?edit=1`);
    } else {
      const report = await createReport.mutateAsync(payload);
      // New report: drop into edit mode on the freshly-saved record.
      navigate(`/reports/${report.id}?edit=1`);
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
      title={
        <input
          value={name}
          placeholder="Untitled report"
          onChange={(event) => setName(event.target.value)}
          className="w-full bg-transparent text-xl font-semibold placeholder:text-muted-foreground focus:outline-none"
          style={{
            border: 'none',
            outline: 'none',
            boxShadow: 'none',
            padding: 0,
          }}
        />
      }
      toolbar={
        <input
          value={description}
          placeholder="Optional description shown in the reports list…"
          onChange={(event) => setDescription(event.target.value)}
          className="w-full bg-transparent text-sm text-muted-foreground placeholder:text-muted-foreground focus:outline-none"
          style={{
            border: 'none',
            outline: 'none',
            boxShadow: 'none',
            padding: 0,
          }}
        />
      }
      action={
        <div className="flex w-full flex-col gap-2 sm:w-auto sm:flex-row">
          {isEditing && reportId ? (
            <Link to={`/reports/${reportId}`} className="w-full sm:w-auto">
              <Button
                variant="outline"
                className="h-11 w-full rounded-full sm:px-5"
              >
                <Eye className="mr-2 h-4 w-4" />
                View
              </Button>
            </Link>
          ) : (
            <Link to="/reports" className="w-full sm:w-auto">
              <Button
                variant="outline"
                className="h-11 w-full rounded-full sm:px-5"
              >
                Cancel
              </Button>
            </Link>
          )}
          {isEditing && reportId && existingReport ? (
            <ReportDeleteButton
              reportId={reportId}
              reportName={existingReport.name}
              className="h-11 rounded-full sm:px-5"
            />
          ) : null}
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
      <ReportBuilderWizard
        definition={definition}
        schemas={schemas}
        blockResults={blockResults}
        onChange={(nextDefinition) => {
          setDefinition(nextDefinition);
          setSaveError(null);
          validateReport.reset();
        }}
      />
      {saveError ? (
        <p className="mt-3 text-sm text-destructive">{saveError}</p>
      ) : null}
    </TilesPage>
  );
}
