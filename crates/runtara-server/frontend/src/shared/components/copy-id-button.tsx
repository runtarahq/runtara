import { Key } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/shared/components/ui/button.tsx';

type Props = {
  id: string;
  variant?:
    | 'default'
    | 'outline'
    | 'secondary'
    | 'ghost'
    | 'link'
    | 'destructive';
  size?: 'default' | 'sm' | 'lg' | 'icon';
  className?: string;
};

export function CopyIdButton(props: Props) {
  const { id, variant = 'ghost', size = 'icon', className = '' } = props;

  const handleClick = () => {
    if (id) {
      navigator.clipboard.writeText(id);
      toast.success('ID copied to clipboard');
    }
  };

  return (
    <Button
      variant={variant}
      size={size}
      className={className || 'h-7 w-7'}
      onClick={handleClick}
      title={`Click to copy ID: ${id}`}
    >
      <Key className={size === 'icon' ? 'h-4 w-4' : 'h-4 w-4 mr-2'} />
      {size !== 'icon' && 'Copy ID'}
    </Button>
  );
}
