import { RectangleHorizontal, Rows3 } from 'lucide-react';
import { Tabs, TabsList, TabsTrigger } from '@/shared/components/ui/tabs';
import type { CanvasDetailLevel } from './CustomNodes/node-size';

/** Details: each card lists its configured inputs. Compact: names only. */
export function CanvasDetailToggle({
  value,
  onChange,
}: {
  value: CanvasDetailLevel;
  onChange: (value: CanvasDetailLevel) => void;
}) {
  return (
    <Tabs
      value={value}
      onValueChange={(next) => onChange(next as CanvasDetailLevel)}
    >
      <TabsList className="h-8" aria-label="Step card detail">
        <TabsTrigger
          value="detailed"
          className="h-6 gap-1.5 px-2 text-xs"
          data-testid="canvas-detail-detailed"
          aria-label="Details"
          title="Details: show each step's configured inputs on its card"
        >
          <Rows3 className="size-3.5" aria-hidden="true" />
          <span className="hidden min-[1100px]:inline">Details</span>
        </TabsTrigger>
        <TabsTrigger
          value="compact"
          className="h-6 gap-1.5 px-2 text-xs"
          data-testid="canvas-detail-compact"
          aria-label="Compact"
          title="Compact: show step names only; hover a step for its inputs"
        >
          <RectangleHorizontal className="size-3.5" aria-hidden="true" />
          <span className="hidden min-[1100px]:inline">Compact</span>
        </TabsTrigger>
      </TabsList>
    </Tabs>
  );
}
