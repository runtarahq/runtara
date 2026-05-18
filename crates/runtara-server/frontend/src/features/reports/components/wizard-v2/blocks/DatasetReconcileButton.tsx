import { useMemo, useState } from 'react';
import { Button } from '@/shared/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/shared/components/ui/dialog';
import { RotateCcw } from 'lucide-react';
import {
  ReportBlockDefinition,
  ReportDatasetDefinition,
} from '../../../types';
import { reconcileDatasetBlock } from '../../../datasetBlocks';

interface DatasetReconcileButtonProps {
  block: ReportBlockDefinition;
  dataset: ReportDatasetDefinition;
  onChange: (block: ReportBlockDefinition) => void;
}

/** Explicit replacement for the wizard v1's silent on-load reconcile. Shows a
 *  JSON diff between the current block and the schema-driven reset before
 *  asking the user to confirm. */
export function DatasetReconcileButton({
  block,
  dataset,
  onChange,
}: DatasetReconcileButtonProps) {
  const [open, setOpen] = useState(false);

  const proposed = useMemo(
    () => (open ? reconcileDatasetBlock(block, dataset) : block),
    [block, dataset, open]
  );

  const sameJson =
    JSON.stringify(block) === JSON.stringify(proposed);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button type="button" variant="outline" size="sm" className="h-7">
          <RotateCcw className="mr-1 h-3 w-3" /> Reset to dataset schema
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Reset block to dataset schema</DialogTitle>
          <DialogDescription>
            Rebuild this block's columns, sort, and series from the dataset's
            current dimensions and measures. Other settings are preserved.
          </DialogDescription>
        </DialogHeader>

        {sameJson ? (
          <p className="text-sm text-muted-foreground">
            Block already matches the dataset schema. Nothing to reset.
          </p>
        ) : (
          <div className="grid grid-cols-2 gap-3">
            <pre className="max-h-[40vh] overflow-auto rounded bg-muted/30 p-2 text-xs">
              {JSON.stringify(block, null, 2)}
            </pre>
            <pre className="max-h-[40vh] overflow-auto rounded bg-emerald-50 p-2 text-xs dark:bg-emerald-950/30">
              {JSON.stringify(proposed, null, 2)}
            </pre>
          </div>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button
            disabled={sameJson}
            onClick={() => {
              onChange(proposed);
              setOpen(false);
            }}
          >
            Apply reset
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
