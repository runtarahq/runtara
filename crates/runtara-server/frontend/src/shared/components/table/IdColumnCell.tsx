import { Key } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { toast } from 'sonner';

interface IdColumnCellProps {
  id: string;
}

export function IdColumnCell({ id }: IdColumnCellProps) {
  const handleCopy = () => {
    navigator.clipboard.writeText(id);
    toast.success('ID copied to clipboard');
  };

  return (
    <div className="pl-3">
      <Button
        variant="secondary"
        size="icon"
        className="size-8"
        onClick={handleCopy}
        title={`Click to copy ID: ${id}`}
      >
        <Key className="size-4" />
      </Button>
    </div>
  );
}
