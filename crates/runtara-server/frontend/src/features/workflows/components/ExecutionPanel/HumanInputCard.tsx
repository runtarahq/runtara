import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { Badge } from '@/shared/components/ui/badge';
import { MessageSquare, Wrench } from 'lucide-react';
import { type PendingInput } from '@/features/workflows/queries';
import { ActionForm } from '@/features/workflows/components/ActionForm';

interface HumanInputCardProps {
  pendingInput: PendingInput;
  onSubmit: (signalId: string, payload: Record<string, any>) => void;
  isSubmitting: boolean;
}

export function HumanInputCard({
  pendingInput,
  onSubmit,
  isSubmitting,
}: HumanInputCardProps) {
  return (
    <Card className="border-amber-500/50 bg-amber-500/5">
      <CardHeader className="pb-3">
        <CardTitle className="text-sm font-medium flex items-center gap-2">
          <MessageSquare className="h-4 w-4 text-amber-600" />
          Human Input Required
        </CardTitle>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <Badge variant="outline" className="text-xs gap-1">
            <Wrench className="h-3 w-3" />
            {pendingInput.toolName}
          </Badge>
          <span>Iteration {pendingInput.iteration}</span>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* AI Agent's message */}
        {pendingInput.message && (
          <div className="rounded-md bg-muted/50 p-3 text-sm">
            {pendingInput.message}
          </div>
        )}

        <ActionForm
          key={pendingInput.signalId}
          inputSchema={pendingInput.responseSchema}
          disabled={isSubmitting}
          onSubmit={(payload) => onSubmit(pendingInput.signalId, payload)}
        />
      </CardContent>
    </Card>
  );
}
