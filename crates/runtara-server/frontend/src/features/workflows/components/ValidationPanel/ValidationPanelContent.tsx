import { CheckCircle2 } from 'lucide-react';
import { ValidationFilterTabs } from './ValidationFilterTabs';
import { ValidationMessageItem } from './ValidationMessageItem';
import { useValidationStore } from '../../stores/validationStore';

interface ValidationPanelContentProps {
  onNavigateToStep: (stepId: string) => void;
}

/**
 * Content area of the validation panel.
 * Contains filter tabs and scrollable message list.
 * Shows "No problems" message when there are no issues.
 */
export function ValidationPanelContent({
  onNavigateToStep,
}: ValidationPanelContentProps) {
  const allMessages = useValidationStore((s) => s.messages);
  // Subscribe to activeFilter so this component re-renders when the filter tab changes
  useValidationStore((s) => s.activeFilter);
  const getFilteredMessages = useValidationStore((s) => s.getFilteredMessages);
  const filteredMessages = getFilteredMessages();

  const hasAnyMessages = allMessages.length > 0;

  // Show empty state when there are no messages at all
  if (!hasAnyMessages) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center text-muted-foreground">
        <CheckCircle2 className="mb-2 size-8 text-success" />
        <p className="text-sm">No problems detected</p>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <ValidationFilterTabs />

      <div className="flex-1 overflow-y-auto">
        <div className="space-y-0.5 p-2">
          {filteredMessages.length === 0 ? (
            <div className="py-8 text-center text-sm text-muted-foreground">
              No issues in this category
            </div>
          ) : (
            filteredMessages.map((message) => (
              <ValidationMessageItem
                key={message.id}
                message={message}
                onNavigate={onNavigateToStep}
              />
            ))
          )}
        </div>
      </div>
    </div>
  );
}
