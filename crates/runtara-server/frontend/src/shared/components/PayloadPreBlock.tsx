import { Badge } from '@/shared/components/ui/badge';
import { formatPayloadForDisplay } from '@/shared/utils/truncated-payload';

interface PayloadPreBlockProps {
  data: unknown;
  className?: string;
  textClassName?: string;
}

export function PayloadPreBlock({
  data,
  className = '',
  textClassName = 'text-xs',
}: PayloadPreBlockProps) {
  const { text, truncated, originalSizeFormatted } =
    formatPayloadForDisplay(data);

  return (
    <div>
      <pre
        className={`${textClassName} bg-muted p-2 rounded overflow-x-auto overflow-y-auto font-mono ${className}`}
      >
        {text}
      </pre>
      {truncated && (
        <div className="mt-1">
          <Badge
            variant="outline"
            className="text-[10px] text-muted-foreground"
          >
            Truncated — original: {originalSizeFormatted}
          </Badge>
        </div>
      )}
    </div>
  );
}
