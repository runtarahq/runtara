import { useState, useMemo } from 'react';
import { Search } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { VisuallyHidden } from '@radix-ui/react-visually-hidden';
import { Input } from '@/shared/components/ui/input';
import {
  ConnectionTypeDto,
  ConnectionCategoryDto,
} from '@/generated/RuntaraRuntimeApi';
import {
  getCategoryIcon,
  getCategoryLabel,
} from '@/features/connections/utils/category-icons';

interface ConnectionPickerModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (connectionType: ConnectionTypeDto) => void;
  connectionTypes: ConnectionTypeDto[];
  categories?: ConnectionCategoryDto[];
  isLoading?: boolean;
}

/**
 * Modal for selecting connection type when creating a new connection.
 * Similar UI to StepPickerModal with search and categorized browsing.
 */
export function ConnectionPickerModal({
  open,
  onOpenChange,
  onSelect,
  connectionTypes,
  categories = [],
  isLoading = false,
}: ConnectionPickerModalProps) {
  const [searchQuery, setSearchQuery] = useState('');

  // Reset state when modal closes
  const handleOpenChange = (newOpen: boolean) => {
    onOpenChange(newOpen);
    if (!newOpen) {
      setSearchQuery('');
    }
  };

  // Group connection types by category field
  const groupedConnectionTypes = useMemo(() => {
    const groups = new Map<string, ConnectionTypeDto[]>();

    for (const ct of connectionTypes) {
      const category = ct.category || 'general';
      if (!groups.has(category)) {
        groups.set(category, []);
      }
      groups.get(category)!.push(ct);
    }

    // Sort connection types within each group alphabetically
    groups.forEach((items) => {
      items.sort((a, b) =>
        (a.displayName || '').localeCompare(b.displayName || '')
      );
    });

    // Order by backend category order, then remaining
    const categoryOrder = categories.map((c) => c.id);
    const result: {
      category: string;
      Icon: ReturnType<typeof getCategoryIcon>;
      label: string;
      items: ConnectionTypeDto[];
    }[] = [];

    for (const catId of categoryOrder) {
      const items = groups.get(catId);
      if (items && items.length > 0) {
        const backendCategory = categories.find((c) => c.id === catId);
        result.push({
          category: catId,
          Icon: getCategoryIcon(catId),
          label: backendCategory?.displayName || getCategoryLabel(catId),
          items,
        });
        groups.delete(catId);
      }
    }

    // Add any remaining groups not in backend categories
    groups.forEach((items, cat) => {
      if (items.length > 0) {
        result.push({
          category: cat,
          Icon: getCategoryIcon(cat),
          label: getCategoryLabel(cat),
          items,
        });
      }
    });

    return result;
  }, [connectionTypes, categories]);

  // Search results
  const searchResults = useMemo(() => {
    if (!searchQuery.trim()) return null;

    const query = searchQuery.toLowerCase();

    return connectionTypes
      .filter(
        (ct) =>
          ct.displayName?.toLowerCase().includes(query) ||
          ct.category?.toLowerCase().includes(query) ||
          ct.description?.toLowerCase().includes(query)
      )
      .sort((a, b) => (a.displayName || '').localeCompare(b.displayName || ''));
  }, [searchQuery, connectionTypes]);

  const handleSelect = (ct: ConnectionTypeDto) => {
    onSelect(ct);
    handleOpenChange(false);
  };

  const isSearching = searchQuery.trim().length > 0;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="sm:max-w-[500px] p-0 gap-0"
        hideCloseButton
        aria-describedby={undefined}
      >
        {/* Visually hidden title for screen readers */}
        <VisuallyHidden>
          <DialogTitle>New Connection</DialogTitle>
        </VisuallyHidden>

        {/* Header */}
        <div className="flex items-center gap-2 p-4 border-b">
          <div className="flex-1">
            <h2 className="text-lg font-semibold">New Connection</h2>
            <p className="text-sm text-muted-foreground">
              {isSearching ? 'Search results' : 'Choose a connection type'}
            </p>
          </div>
        </div>

        {/* Search */}
        <div className="p-4 border-b">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder="Search connection types..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9"
              autoFocus
            />
          </div>
        </div>

        {/* Content */}
        <div className="max-h-[400px] overflow-y-auto p-4">
          {isLoading ? (
            <div className="text-center py-8 text-muted-foreground">
              Loading connection types...
            </div>
          ) : isSearching ? (
            // Search Results View
            <div className="space-y-1">
              {searchResults && searchResults.length === 0 ? (
                <div className="text-center py-8 text-muted-foreground">
                  No results found for "{searchQuery}"
                </div>
              ) : (
                searchResults?.map((ct) => {
                  const Icon = getCategoryIcon(ct.category);
                  return (
                    <button
                      key={ct.integrationId}
                      type="button"
                      onClick={() => handleSelect(ct)}
                      className="w-full flex items-center gap-3 px-3 py-2 rounded-lg text-left transition-colors hover:bg-muted"
                    >
                      <Icon className="h-5 w-5 text-muted-foreground" />
                      <div>
                        <div className="font-medium">{ct.displayName}</div>
                        {ct.category && (
                          <div className="text-xs text-muted-foreground">
                            {getCategoryLabel(ct.category)}
                          </div>
                        )}
                      </div>
                    </button>
                  );
                })
              )}
            </div>
          ) : (
            // Browse View - Grouped by Category
            <div className="space-y-6">
              {groupedConnectionTypes.length === 0 ? (
                <div className="text-center py-8 text-muted-foreground">
                  No connection types available
                </div>
              ) : (
                groupedConnectionTypes.map((group) => (
                  <div key={group.category}>
                    <div className="flex items-center gap-2 mb-2">
                      <group.Icon className="h-4 w-4 text-muted-foreground" />
                      <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
                        {group.label}
                      </span>
                    </div>
                    <div className="space-y-1">
                      {group.items.map((ct) => (
                        <button
                          key={ct.integrationId}
                          type="button"
                          onClick={() => handleSelect(ct)}
                          className="w-full flex items-center gap-3 px-3 py-2 rounded-lg text-left transition-colors hover:bg-muted"
                        >
                          <group.Icon className="h-5 w-5 text-muted-foreground" />
                          <div>
                            <div className="font-medium">{ct.displayName}</div>
                          </div>
                        </button>
                      ))}
                    </div>
                  </div>
                ))
              )}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
