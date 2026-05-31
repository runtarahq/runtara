import { useState } from 'react';
import { Check, Database, Loader2 } from 'lucide-react';
import { Link } from 'react-router';
import { Button } from '@/shared/components/ui/button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import { cn } from '@/lib/utils';
import { useObjectModelConnectionSelection } from '../hooks/useObjectModelConnectionSelection';

export function ObjectModelConnectionSelector() {
  const {
    connections,
    selectedConnectionId,
    setSelectedConnectionId,
    isLoading,
    isError,
  } = useObjectModelConnectionSelection();
  const [open, setOpen] = useState(false);

  if (isLoading) {
    return (
      <Button
        variant="outline"
        size="icon"
        className="h-9 w-9 shrink-0"
        disabled
        aria-label="Loading database connections"
      >
        <Loader2 className="h-4 w-4 animate-spin" />
      </Button>
    );
  }

  if (isError || connections.length === 0) {
    return (
      <Button asChild variant="outline" size="sm">
        <Link to="/connections/postgres/create">
          <Database className="mr-2 h-4 w-4" />
          Add database connection
        </Link>
      </Button>
    );
  }

  const current = connections.find((c) => c.id === selectedConnectionId);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          size="icon"
          className="h-9 w-9 shrink-0"
          aria-label="Database connection"
          title={
            current ? `Database: ${current.title}` : 'Database connection'
          }
        >
          <Database className="h-4 w-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-72 p-0">
        <div className="border-b px-3 py-2 text-sm font-medium">
          Database connection
        </div>
        <div className="max-h-72 overflow-y-auto p-1">
          {connections.map((connection) => {
            const selected = connection.id === selectedConnectionId;
            return (
              <button
                key={connection.id}
                type="button"
                onClick={() => {
                  setSelectedConnectionId(connection.id);
                  setOpen(false);
                }}
                className={cn(
                  'flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-muted',
                  selected && 'text-primary'
                )}
              >
                <span className="truncate">
                  {connection.title}
                  {connection.defaultFor?.includes('object_model')
                    ? ' (default)'
                    : ''}
                </span>
                {selected && <Check className="h-4 w-4 shrink-0" />}
              </button>
            );
          })}
        </div>
      </PopoverContent>
    </Popover>
  );
}
