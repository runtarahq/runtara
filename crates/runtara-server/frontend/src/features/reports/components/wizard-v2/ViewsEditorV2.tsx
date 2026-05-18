import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { Plus, Trash2 } from 'lucide-react';
import { ReportDefinition, ReportViewDefinition } from '../../types';

interface ViewsEditorV2Props {
  definition: ReportDefinition;
  onChange: (definition: ReportDefinition) => void;
}

function newView(): ReportViewDefinition {
  const id = `view_${Math.random().toString(36).slice(2, 7)}`;
  return {
    id,
    title: 'New view',
    layout: { id: `${id}_root`, columns: 1, rows: 1, items: [] },
  };
}

export function ViewsEditorV2({ definition, onChange }: ViewsEditorV2Props) {
  const views = definition.views ?? [];

  const updateViews = (next: ReportViewDefinition[]) =>
    onChange({ ...definition, views: next });

  const updateView = (
    id: string,
    updater: (view: ReportViewDefinition) => ReportViewDefinition
  ) => updateViews(views.map((v) => (v.id === id ? updater(v) : v)));

  return (
    <div className="grid gap-3">
      {views.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No named views. Add one to enable drill-down navigation from row or
          chart clicks.
        </p>
      ) : (
        <div className="grid gap-3">
          {views.map((view) => (
            <Card key={view.id}>
              <CardHeader className="flex flex-row items-center justify-between gap-2 space-y-0 py-3">
                <CardTitle className="text-sm">
                  {view.title || view.id}
                </CardTitle>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7 text-destructive"
                  onClick={() =>
                    updateViews(views.filter((v) => v.id !== view.id))
                  }
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </CardHeader>
              <CardContent className="grid gap-3 pt-0">
                <div className="grid grid-cols-2 gap-3">
                  <div className="grid gap-1.5">
                    <Label className="text-xs">ID</Label>
                    <Input
                      value={view.id}
                      onChange={(event) =>
                        updateView(view.id, (v) => ({
                          ...v,
                          id: event.target.value,
                        }))
                      }
                    />
                  </div>
                  <div className="grid gap-1.5">
                    <Label className="text-xs">Title</Label>
                    <Input
                      value={view.title ?? ''}
                      onChange={(event) =>
                        updateView(view.id, (v) => ({
                          ...v,
                          title: event.target.value || null,
                        }))
                      }
                    />
                  </div>
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}
      <div>
        <Button
          type="button"
          variant="outline"
          onClick={() => updateViews([...views, newView()])}
        >
          <Plus className="mr-1 h-3.5 w-3.5" /> Add view
        </Button>
      </div>
    </div>
  );
}
