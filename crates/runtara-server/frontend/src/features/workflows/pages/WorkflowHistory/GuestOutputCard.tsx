import { SquareTerminal } from 'lucide-react';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';

/**
 * What the run's guest wrote to stdout and stderr, as its runner captured it.
 * Hidden when the runner keeps no capture.
 */
export function GuestOutputCard({ output }: { output?: string | null }) {
  if (!output) return null;
  return (
    <Card data-testid="guest-output">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <SquareTerminal className="size-5" />
          Guest output
        </CardTitle>
      </CardHeader>
      <CardContent>
        <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-all rounded-md bg-muted p-3 font-mono text-xs text-foreground">
          {output}
        </pre>
      </CardContent>
    </Card>
  );
}
