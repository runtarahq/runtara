import { Database, Loader2 } from 'lucide-react';
import { Link } from 'react-router';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Button } from '@/shared/components/ui/button';
import { useObjectModelConnectionSelection } from '../hooks/useObjectModelConnectionSelection';

export function ObjectModelConnectionSelector() {
  const {
    connections,
    selectedConnectionId,
    setSelectedConnectionId,
    isLoading,
    isError,
  } = useObjectModelConnectionSelection();

  if (isLoading) {
    return (
      <div className="flex h-11 items-center gap-2 rounded-lg border bg-background px-3 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading database connections
      </div>
    );
  }

  if (isError || connections.length === 0) {
    return (
      <Button asChild variant="outline" className="h-11 rounded-lg">
        <Link to="/connections/postgres/create">
          <Database className="mr-2 h-4 w-4" />
          Add database connection
        </Link>
      </Button>
    );
  }

  return (
    <div className="flex items-center gap-2">
      <Database className="h-4 w-4 text-muted-foreground" />
      <Select
        value={selectedConnectionId ?? ''}
        onValueChange={setSelectedConnectionId}
      >
        <SelectTrigger
          className="h-11 w-[min(22rem,calc(100vw-2rem))] rounded-lg"
          aria-label="Database connection"
        >
          <SelectValue placeholder="Database connection" />
        </SelectTrigger>
        <SelectContent>
          {connections.map((connection) => (
            <SelectItem key={connection.id} value={connection.id}>
              {connection.title}
              {connection.defaultFor?.includes('object_model')
                ? ' (default)'
                : ''}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
