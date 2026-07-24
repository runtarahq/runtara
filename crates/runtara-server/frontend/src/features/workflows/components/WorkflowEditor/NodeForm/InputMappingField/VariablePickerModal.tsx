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
import { SectionLabel } from '@/shared/components/section-label';
import {
  PICKER_DIALOG_WIDTH,
  PICKER_LIST_MAX_HEIGHT,
} from '@/shared/components/picker-dialog';
import {
  PickerEmpty,
  PickerItem,
  PickerTypeChip,
} from '@/shared/components/picker-item';
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
 * Groups that share the standard label + description row layout, rendered in
 * this order. Step Outputs has its own row layout and renders last.
 */
const STANDARD_GROUPS = [
  'Iteration Context',
  'Loop Context',
  'Split Scope',
  'Wait Scope',
  'Workflow Inputs',
  'Variables',
] as const;

/**
 * Modal dialog for browsing and selecting available variables from previous steps
 */
export function VariablePickerModal({
  open,
  onOpenChange,
  onSelect,
}: VariablePickerModalProps) {
  const [searchQuery, setSearchQuery] = useState('');
  const {
    previousSteps,
    inputSchemaFields,
    variables,
    isInsideWhileLoop,
    isInsideSplit,
    isInsideWaitScope,
    splitItemSchemaFields,
  } = useContext(NodeFormContext);

  // Generate and filter suggestions
  const allSuggestions = useMemo(
    () =>
      composeVariableSuggestions(
        previousSteps,
        inputSchemaFields,
        variables,
        isInsideWhileLoop,
        isInsideSplit,
        isInsideWaitScope,
        splitItemSchemaFields
      ),
    [
      previousSteps,
      inputSchemaFields,
      variables,
      isInsideWhileLoop,
      isInsideSplit,
      isInsideWaitScope,
      splitItemSchemaFields,
    ]
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
      <DialogContent className={PICKER_DIALOG_WIDTH}>
        <DialogHeader>
          <DialogTitle>Select Variable</DialogTitle>
          <DialogDescription>
            Choose a variable from workflow inputs or previous step outputs
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {/* Search input */}
          <div className="relative">
            <Icons.search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              placeholder="Search variables..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9 font-mono text-sm"
              autoFocus
            />
          </div>

          {/* Variable list */}
          <div
            className={`${PICKER_LIST_MAX_HEIGHT} space-y-4 overflow-y-auto`}
          >
            {/* Free-text path entry: any legal reference path can be used
                even when it is not in the suggestion list */}
            {searchQuery.trim() !== '' &&
              !allSuggestions.some(
                (suggestion) => suggestion.value === searchQuery.trim()
              ) && (
                <PickerItem
                  className="border border-dashed"
                  onSelect={() =>
                    handleSelect({
                      label: searchQuery.trim(),
                      value: searchQuery.trim(),
                      group: 'Workflow Inputs',
                    })
                  }
                  label={
                    <>
                      <p className="truncate font-mono text-sm">
                        {searchQuery.trim()}
                      </p>
                      <p className="truncate text-xs opacity-70">
                        Use as custom reference path
                      </p>
                    </>
                  }
                />
              )}
            {filteredSuggestions.length === 0 ? (
              <PickerEmpty>
                <Icons.inbox className="mx-auto mb-2 h-8 w-8 opacity-50" />
                <p>No variables found</p>
              </PickerEmpty>
            ) : (
              <>
                {STANDARD_GROUPS.map(
                  (group) =>
                    groupedSuggestions[group].length > 0 && (
                      <div key={group}>
                        <SectionLabel as="h4" className="mb-2">
                          {group}
                        </SectionLabel>
                        <div className="space-y-0.5">
                          {groupedSuggestions[group].map((suggestion) => (
                            <PickerItem
                              key={suggestion.value}
                              onSelect={() => handleSelect(suggestion)}
                              icon={getIconForType(
                                suggestion.type,
                                suggestion.value
                              )}
                              label={
                                <>
                                  <p className="truncate font-mono text-sm">
                                    {suggestion.label}
                                  </p>
                                  {suggestion.description && (
                                    <p className="truncate text-xs opacity-70">
                                      {suggestion.description}
                                    </p>
                                  )}
                                </>
                              }
                              typeChip={
                                suggestion.type && (
                                  <PickerTypeChip>
                                    {suggestion.type}
                                  </PickerTypeChip>
                                )
                              }
                            />
                          ))}
                        </div>
                      </div>
                    )
                )}

                {/* Step Outputs */}
                {groupedSuggestions['Step Outputs'].length > 0 && (
                  <div>
                    <SectionLabel as="h4" className="mb-2">
                      Step Outputs
                    </SectionLabel>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Step Outputs'].map((suggestion) => (
                        <PickerItem
                          key={suggestion.value}
                          onSelect={() => handleSelect(suggestion)}
                          icon={getIconForType(
                            suggestion.type,
                            suggestion.value
                          )}
                          label={
                            <>
                              <p className="truncate text-sm">
                                <span className="font-medium">
                                  {suggestion.stepName ||
                                    suggestion.description}
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
                                <p className="truncate font-mono text-2xs opacity-50">
                                  {suggestion.stepId}
                                </p>
                              )}
                            </>
                          }
                          typeChip={
                            suggestion.type && (
                              <PickerTypeChip>{suggestion.type}</PickerTypeChip>
                            )
                          }
                        />
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
