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
import { PickerEmpty } from '@/shared/components/picker-item';
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
            <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              placeholder="Search connections..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9"
              autoFocus
            />
          </div>

          {/* Connection list */}
          <div className="max-h-[300px] space-y-1 overflow-y-auto">
            {filteredOptions.length === 0 ? (
              <PickerEmpty>
                <Inbox className="mx-auto mb-2 size-8 opacity-50" />
                <p>No connections found</p>
              </PickerEmpty>
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
                    className={`flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors ${
                      isSelected
                        ? 'bg-primary/10 text-primary'
                        : 'text-foreground hover:bg-accent'
                    }`}
                  >
                    {/* Icon */}
                    <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-muted">
                      {platformIcon ? (
                        <span className="text-lg">{platformIcon}</span>
                      ) : (
                        <Link className="size-4 text-muted-foreground" />
                      )}
                    </div>

                    {/* Label and platform */}
                    <div className="min-w-0 flex-1">
                      <p
                        className="break-words font-medium"
                        title={option.label}
                      >
                        {option.label}
                      </p>
                      {platformName && (
                        <p className="truncate text-xs text-muted-foreground">
                          {platformName}
                        </p>
                      )}
                    </div>

                    {/* Selected indicator */}
                    {isSelected && (
                      <Check className="size-4 shrink-0 text-primary" />
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
