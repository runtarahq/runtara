import { AlertCircle, AlertTriangle, ArrowRight } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { ValidationMessage } from '../../types/validation';
import { cn } from '@/lib/utils';

interface ValidationMessageItemProps {
  message: ValidationMessage;
  onNavigate: (stepId: string) => void;
}

/**
 * Individual validation message row in the panel.
 * Shows severity icon, message, step location, and navigation action.
 */
export function ValidationMessageItem({
  message,
  onNavigate,
}: ValidationMessageItemProps) {
  const isError = message.severity === 'error';
  const hasStep = !!message.stepId;

  const handleClick = () => {
    if (message.stepId) {
      onNavigate(message.stepId);
    }
  };

  return (
    <div
      className={cn(
        'group flex items-start gap-3 rounded-md px-3 py-2 text-sm transition-colors',
        hasStep && 'cursor-pointer hover:bg-muted/50'
      )}
      onClick={handleClick}
    >
      {/* Severity Icon */}
      <div className="mt-0.5 shrink-0">
        {isError ? (
          <AlertCircle className="h-4 w-4 text-destructive" />
        ) : (
          <AlertTriangle className="h-4 w-4 text-warning" />
        )}
      </div>

      {/* Content */}
      <div className="min-w-0 flex-1">
        <div className="flex items-start gap-2">
          <span className="font-mono text-xs text-muted-foreground shrink-0">
            [{message.code}]
          </span>
          <span
            className={cn(
              'break-words',
              isError ? 'text-destructive' : 'text-warning'
            )}
          >
            {message.message}
          </span>
        </div>

        {/* Step location */}
        {(message.stepName || message.stepId) && (
          <div className="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
            <span className="inline-flex items-center gap-1 px-1.5 py-0.5 bg-muted rounded text-xs">
              Step: {message.stepName || message.stepId}
            </span>
            {message.fieldName && <span>› {message.fieldName}</span>}
          </div>
        )}
      </div>

      {/* Quick action - navigate to step */}
      {hasStep && (
        <Button
          variant="ghost"
          size="icon"
          className="h-6 w-6 shrink-0 opacity-0 group-hover:opacity-100 transition-opacity"
          onClick={(e) => {
            e.stopPropagation();
            onNavigate(message.stepId!);
          }}
          title="Go to step"
        >
          <ArrowRight className="h-3 w-3" />
        </Button>
      )}
    </div>
  );
}
