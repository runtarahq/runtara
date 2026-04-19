import { useState, useContext, useMemo } from 'react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/shared/components/ui/dialog';
import { Input } from '@/shared/components/ui/input';
import { Icons } from '@/shared/components/icons.tsx';
import { NodeFormContext } from '../NodeFormContext';
import {
  composeVariableSuggestions,
  filterSuggestions,
  groupSuggestions,
  VariableSuggestion,
} from '../InputMappingValueField/VariableSuggestions';

interface VariablePickerModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (variable: VariableSuggestion) => void;
}

/**
 * Get icon component based on variable type and path
 */
function getIconForType(type?: string, path?: string) {
  const lowerType = type?.toLowerCase() || '';
  const lowerPath = path?.toLowerCase() || '';

  // Check explicit type first
  if (lowerType.includes('string') || lowerType.includes('text')) {
    return <Icons.type className="h-4 w-4" />;
  }
  if (
    lowerType.includes('number') ||
    lowerType.includes('int') ||
    lowerType.includes('integer') ||
    lowerType.includes('double') ||
    lowerType.includes('float')
  ) {
    return <Icons.hash className="h-4 w-4" />;
  }
  if (lowerType.includes('boolean') || lowerType.includes('bool')) {
    return <Icons.squareCheck className="h-4 w-4" />;
  }
  if (lowerType.includes('array') || lowerType.includes('list')) {
    return <Icons.list className="h-4 w-4" />;
  }
  if (lowerType.includes('object')) {
    return <Icons.braces className="h-4 w-4" />;
  }
  if (
    lowerType.includes('date') ||
    lowerType.includes('time') ||
    lowerPath.includes('date') ||
    lowerPath.includes('time')
  ) {
    return <Icons.calendar className="h-4 w-4" />;
  }

  // Infer from path
  if (lowerPath.includes('email')) {
    return <Icons.mail className="h-4 w-4" />;
  }
  if (lowerPath.includes('name')) {
    return <Icons.user className="h-4 w-4" />;
  }
  if (lowerPath.includes('id') || lowerPath.includes('key')) {
    return <Icons.key className="h-4 w-4" />;
  }
  if (
    lowerPath.includes('price') ||
    lowerPath.includes('amount') ||
    lowerPath.includes('total') ||
    lowerPath.includes('cost')
  ) {
    return <Icons.dollarSign className="h-4 w-4" />;
  }

  // Default icon
  return <Icons.gitBranch className="h-4 w-4" />;
}

/**
 * Modal dialog for browsing and selecting available variables from previous steps
 */
export function VariablePickerModal({
  open,
  onOpenChange,
  onSelect,
}: VariablePickerModalProps) {
  const [searchQuery, setSearchQuery] = useState('');
  const { previousSteps, inputSchemaFields, variables, isInsideWhileLoop } =
    useContext(NodeFormContext);

  // Generate and filter suggestions
  const allSuggestions = useMemo(
    () =>
      composeVariableSuggestions(
        previousSteps,
        inputSchemaFields,
        variables,
        isInsideWhileLoop
      ),
    [previousSteps, inputSchemaFields, variables, isInsideWhileLoop]
  );

  const filteredSuggestions = useMemo(
    () => filterSuggestions(allSuggestions, searchQuery),
    [allSuggestions, searchQuery]
  );

  const groupedSuggestions = useMemo(
    () => groupSuggestions(filteredSuggestions),
    [filteredSuggestions]
  );

  const handleSelect = (suggestion: VariableSuggestion) => {
    onSelect(suggestion);
    onOpenChange(false);
    setSearchQuery('');
  };

  const handleOpenChange = (newOpen: boolean) => {
    onOpenChange(newOpen);
    if (!newOpen) {
      setSearchQuery('');
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-[500px]">
        <DialogHeader>
          <DialogTitle>Select Variable</DialogTitle>
          <DialogDescription>
            Choose a variable from workflow inputs or previous step outputs
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {/* Search input */}
          <div className="relative">
            <Icons.search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder="Search variables..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9 font-mono text-sm"
              autoFocus
            />
          </div>

          {/* Variable list */}
          <div className="max-h-[400px] overflow-y-auto space-y-4">
            {filteredSuggestions.length === 0 ? (
              <div className="text-center py-8 text-muted-foreground">
                <Icons.inbox className="h-8 w-8 mx-auto mb-2 opacity-50" />
                <p>No variables found</p>
              </div>
            ) : (
              <>
                {/* Workflow Inputs */}
                {groupedSuggestions['Workflow Inputs'].length > 0 && (
                  <div>
                    <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                      Workflow Inputs
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Workflow Inputs'].map(
                        (suggestion) => (
                          <button
                            key={suggestion.value}
                            type="button"
                            onClick={() => handleSelect(suggestion)}
                            className="w-full flex items-center gap-2 px-2 py-1.5 rounded hover:bg-accent text-left transition-colors text-muted-foreground hover:text-foreground"
                          >
                            {getIconForType(suggestion.type, suggestion.value)}
                            <div className="flex-1 min-w-0">
                              <p className="font-mono text-sm truncate">
                                {suggestion.label}
                              </p>
                              {suggestion.description && (
                                <p className="text-xs truncate opacity-70">
                                  {suggestion.description}
                                </p>
                              )}
                            </div>
                            {suggestion.type && (
                              <span className="text-[11px] font-mono px-1.5 py-0.5 rounded shrink-0 text-muted-foreground bg-black/5 dark:bg-white/10">
                                {suggestion.type}
                              </span>
                            )}
                          </button>
                        )
                      )}
                    </div>
                  </div>
                )}

                {/* Variables */}
                {groupedSuggestions['Variables'].length > 0 && (
                  <div>
                    <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                      Variables
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Variables'].map((suggestion) => (
                        <button
                          key={suggestion.value}
                          type="button"
                          onClick={() => handleSelect(suggestion)}
                          className="w-full flex items-center gap-2 px-2 py-1.5 rounded hover:bg-accent text-left transition-colors text-muted-foreground hover:text-foreground"
                        >
                          {getIconForType(suggestion.type, suggestion.value)}
                          <div className="flex-1 min-w-0">
                            <p className="font-mono text-sm truncate">
                              {suggestion.label}
                            </p>
                            {suggestion.description && (
                              <p className="text-xs truncate opacity-70">
                                {suggestion.description}
                              </p>
                            )}
                          </div>
                          {suggestion.type && (
                            <span className="text-[11px] font-mono px-1.5 py-0.5 rounded shrink-0 text-muted-foreground bg-black/5 dark:bg-white/10">
                              {suggestion.type}
                            </span>
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {/* Step Outputs */}
                {groupedSuggestions['Step Outputs'].length > 0 && (
                  <div>
                    <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                      Step Outputs
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Step Outputs'].map((suggestion) => (
                        <button
                          key={suggestion.value}
                          type="button"
                          onClick={() => handleSelect(suggestion)}
                          className="w-full flex items-center gap-2 px-2 py-1.5 rounded hover:bg-accent text-left transition-colors text-muted-foreground hover:text-foreground"
                        >
                          {getIconForType(suggestion.type, suggestion.value)}
                          <div className="flex-1 min-w-0">
                            <p className="text-sm truncate">
                              <span className="font-medium">
                                {suggestion.stepName || suggestion.description}
                              </span>
                              {suggestion.fieldPath && (
                                <span className="text-muted-foreground">
                                  {' → '}
                                  <span className="font-mono">
                                    {suggestion.fieldPath}
                                  </span>
                                </span>
                              )}
                            </p>
                            {suggestion.stepId && (
                              <p className="text-[11px] font-mono truncate opacity-50">
                                {suggestion.stepId}
                              </p>
                            )}
                          </div>
                          {suggestion.type && (
                            <span className="text-[11px] font-mono px-1.5 py-0.5 rounded shrink-0 bg-black/5 dark:bg-white/10">
                              {suggestion.type}
                            </span>
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
