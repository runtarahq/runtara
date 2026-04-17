import { useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { Badge } from '@/shared/components/ui/badge';
import { cn } from '@/lib/utils';
import {
  parseStructuredError,
  getErrorBadgeVariant,
  getErrorType,
} from '@/shared/utils/structured-error';
import {
  getLocalizedMessage,
  getErrorGuidance,
} from '@/shared/constants/error-messages';
import type { StructuredError } from '@/shared/types/structured-error';

interface StructuredErrorDisplayProps {
  /** Error string (can be JSON-serialized structured error or legacy plain text) */
  error: string | null | undefined;
  /** Display mode: compact (inline) or expanded (detailed) */
  mode?: 'compact' | 'expanded';
  /** Additional CSS classes */
  className?: string;
  /** Show error code badge */
  showCode?: boolean;
  /** Show category badge */
  showCategory?: boolean;
  /** Show attributes section */
  showAttributes?: boolean;
  /** Show actionable guidance when available */
  showGuidance?: boolean;
}

/**
 * Display component for structured errors with fallback to plain text.
 *
 * Features:
 * - Parses JSON-serialized structured errors
 * - Falls back to plain text for legacy errors
 * - Color-coded badges based on error category and severity
 * - Expandable attributes section
 * - Actionable guidance for common errors
 *
 * @example
 * ```tsx
 * // Compact inline display
 * <StructuredErrorDisplay error={execution.error} mode="compact" />
 *
 * // Expanded detailed display with guidance
 * <StructuredErrorDisplay
 *   error={execution.error}
 *   mode="expanded"
 *   showGuidance
 * />
 * ```
 */
export function StructuredErrorDisplay({
  error,
  mode = 'compact',
  className,
  showCode = true,
  showCategory = true,
  showAttributes = true,
  showGuidance = true,
}: StructuredErrorDisplayProps) {
  const [attributesExpanded, setAttributesExpanded] = useState(false);

  if (!error) {
    return null;
  }

  const structured = parseStructuredError(error);

  // Legacy plain text error - display as-is
  if (!structured) {
    return (
      <div
        className={cn(
          'text-sm text-destructive bg-destructive/10 p-3 rounded',
          className
        )}
      >
        <strong>Error:</strong> {error}
      </div>
    );
  }

  return (
    <StructuredErrorContent
      structured={structured}
      mode={mode}
      className={className}
      showCode={showCode}
      showCategory={showCategory}
      showAttributes={showAttributes}
      showGuidance={showGuidance}
      attributesExpanded={attributesExpanded}
      setAttributesExpanded={setAttributesExpanded}
    />
  );
}

interface StructuredErrorContentProps {
  structured: StructuredError;
  mode: 'compact' | 'expanded';
  className?: string;
  showCode: boolean;
  showCategory: boolean;
  showAttributes: boolean;
  showGuidance: boolean;
  attributesExpanded: boolean;
  setAttributesExpanded: (expanded: boolean) => void;
}

function StructuredErrorContent({
  structured,
  mode,
  className,
  showCode,
  showCategory,
  showAttributes,
  showGuidance,
  attributesExpanded,
  setAttributesExpanded,
}: StructuredErrorContentProps) {
  // Normalize attributes — backend may omit or send as `context`
  const attributes = structured.attributes ?? {};
  const errorType = getErrorType(structured);
  const badgeVariant = getErrorBadgeVariant(structured);
  const localizedMessage = getLocalizedMessage(structured);
  const guidance = showGuidance ? getErrorGuidance(structured.code) : null;
  const hasAttributes = showAttributes && Object.keys(attributes).length > 0;

  // Color mapping for error types
  const bgColorClass =
    errorType === 'transient'
      ? 'bg-warning/10'
      : errorType === 'business'
        ? 'bg-warning/10'
        : 'bg-destructive/10';

  const textColorClass =
    errorType === 'transient'
      ? 'text-warning'
      : errorType === 'business'
        ? 'text-warning'
        : 'text-destructive';

  if (mode === 'compact') {
    return (
      <div
        className={cn(
          'text-sm rounded p-3',
          bgColorClass,
          textColorClass,
          className
        )}
      >
        <div className="flex items-start gap-2 flex-wrap">
          {showCode && (
            <Badge variant={badgeVariant} className="shrink-0">
              {structured.code}
            </Badge>
          )}
          {showCategory && (
            <Badge variant="outline" className="shrink-0">
              {structured.category}
            </Badge>
          )}
          <span className="flex-1 min-w-0">{localizedMessage}</span>
        </div>
      </div>
    );
  }

  // Expanded mode
  return (
    <div className={cn('rounded border', bgColorClass, className)}>
      <div className="p-4 space-y-3">
        {/* Header with badges */}
        <div className="flex items-center gap-2 flex-wrap">
          {showCode && (
            <Badge variant={badgeVariant} className="font-mono">
              {structured.code}
            </Badge>
          )}
          {showCategory && (
            <Badge variant="outline" className="capitalize">
              {structured.category}
            </Badge>
          )}
          <Badge variant="muted" className="capitalize">
            {structured.severity}
          </Badge>
        </div>

        {/* Error message */}
        <div className={cn('text-sm font-medium', textColorClass)}>
          {localizedMessage}
        </div>

        {/* Guidance */}
        {guidance && (
          <div className="text-sm text-muted-foreground bg-background/50 p-2 rounded border">
            <strong>Suggestion:</strong> {guidance}
          </div>
        )}

        {/* Attributes (expandable) */}
        {hasAttributes && (
          <div className="border-t pt-3">
            <button
              onClick={() => setAttributesExpanded(!attributesExpanded)}
              className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
            >
              {attributesExpanded ? (
                <ChevronDown className="h-3 w-3" />
              ) : (
                <ChevronRight className="h-3 w-3" />
              )}
              <span>Additional Details</span>
            </button>

            {attributesExpanded && (
              <div className="mt-2 space-y-1">
                {Object.entries(attributes).map(([key, value]) => (
                  <div key={key} className="text-xs font-mono">
                    <span className="text-muted-foreground">{key}:</span>{' '}
                    <span className={textColorClass}>
                      {JSON.stringify(value)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
