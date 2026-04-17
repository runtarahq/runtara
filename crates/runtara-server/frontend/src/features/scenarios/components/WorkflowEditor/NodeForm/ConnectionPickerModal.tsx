import { useState, useMemo } from 'react';
import { Search, Inbox, Link, Check } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/shared/components/ui/dialog';
import { Input } from '@/shared/components/ui/input';
import { getPlatformIcon, getPlatformName } from '@/shared/utils/platform-info';
import { ConnectionDto } from '@/generated/RuntaraRuntimeApi';

interface ConnectionOption {
  label: string;
  value: string;
  integrationId: string | null;
}

interface ConnectionPickerModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (connectionId: string) => void;
  connections: ConnectionDto[];
  currentConnectionId?: string;
}

/**
 * Modal dialog for selecting a connection
 */
export function ConnectionPickerModal({
  open,
  onOpenChange,
  onSelect,
  connections,
  currentConnectionId,
}: ConnectionPickerModalProps) {
  const [searchQuery, setSearchQuery] = useState('');

  // Build connection options
  const connectionOptions: ConnectionOption[] = useMemo(() => {
    const noneOption: ConnectionOption = {
      label: 'None (Manual auth)',
      value: '__none__',
      integrationId: null,
    };
    const options: ConnectionOption[] =
      connections?.map((connection) => ({
        label: connection.title || connection.id,
        value: connection.id,
        integrationId: connection.integrationId || null,
      })) || [];
    return [noneOption, ...options];
  }, [connections]);

  // Filter by search
  const filteredOptions = useMemo(() => {
    if (!searchQuery.trim()) return connectionOptions;
    const query = searchQuery.toLowerCase();
    return connectionOptions.filter(
      (opt) =>
        opt.label.toLowerCase().includes(query) ||
        (opt.integrationId &&
          getPlatformName(opt.integrationId).toLowerCase().includes(query))
    );
  }, [connectionOptions, searchQuery]);

  const handleSelect = (connectionId: string) => {
    onSelect(connectionId === '__none__' ? '' : connectionId);
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
      <DialogContent className="sm:max-w-[400px]">
        <DialogHeader>
          <DialogTitle>Select Connection</DialogTitle>
          <DialogDescription>
            Choose a connection for this operation
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {/* Search input */}
          <div className="relative">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder="Search connections..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9"
              autoFocus
            />
          </div>

          {/* Connection list */}
          <div className="max-h-[300px] overflow-y-auto space-y-1">
            {filteredOptions.length === 0 ? (
              <div className="text-center py-8 text-muted-foreground">
                <Inbox className="h-8 w-8 mx-auto mb-2 opacity-50" />
                <p>No connections found</p>
              </div>
            ) : (
              filteredOptions.map((option) => {
                const isSelected = currentConnectionId
                  ? option.value === currentConnectionId
                  : option.value === '__none__';
                const platformIcon = option.integrationId
                  ? getPlatformIcon(option.integrationId)
                  : null;
                const platformName = option.integrationId
                  ? getPlatformName(option.integrationId)
                  : null;

                return (
                  <button
                    key={option.value}
                    type="button"
                    onClick={() => handleSelect(option.value)}
                    className={`w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-left transition-colors ${
                      isSelected
                        ? 'bg-primary/10 text-primary'
                        : 'hover:bg-accent text-foreground'
                    }`}
                  >
                    {/* Icon */}
                    <div className="shrink-0 w-8 h-8 rounded-md bg-muted flex items-center justify-center">
                      {platformIcon ? (
                        <span className="text-lg">{platformIcon}</span>
                      ) : (
                        <Link className="h-4 w-4 text-muted-foreground" />
                      )}
                    </div>

                    {/* Label and platform */}
                    <div className="flex-1 min-w-0">
                      <p className="font-medium truncate">{option.label}</p>
                      {platformName && (
                        <p className="text-xs text-muted-foreground truncate">
                          {platformName}
                        </p>
                      )}
                    </div>

                    {/* Selected indicator */}
                    {isSelected && (
                      <Check className="h-4 w-4 shrink-0 text-primary" />
                    )}
                  </button>
                );
              })
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
