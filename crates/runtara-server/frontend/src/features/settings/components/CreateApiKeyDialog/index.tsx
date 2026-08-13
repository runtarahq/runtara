import { useState } from 'react';
import { Controller, useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { toast } from 'sonner';
import { Copy, Check, AlertTriangle } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { Button } from '@/shared/components/ui/button';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { FieldError } from '@/shared/components/ui/form';
import { useCreateApiKey } from '../../hooks/useApiKeys';
import { Alert, AlertDescription } from '@/shared/components/ui/alert';

const schema = z.object({
  name: z.string().min(1, 'Name is required').max(100, 'Name is too long'),
  /** Maps to the `read_only` scope on submit; unchecked sends no scope at all. */
  readOnly: z.boolean(),
});

type FormValues = z.infer<typeof schema>;

interface CreateApiKeyDialogProps {
  open: boolean;
  onClose: () => void;
}

export function CreateApiKeyDialog({ open, onClose }: CreateApiKeyDialogProps) {
  const [createdKey, setCreatedKey] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const { mutate: createKey, isPending } = useCreateApiKey();

  const form = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { name: '', readOnly: false },
  });

  const handleSubmit = ({ name, readOnly }: FormValues) => {
    // An unscoped key sends no `scope` at all, so it is stored exactly like every key
    // created before scopes existed.
    createKey(
      { name, ...(readOnly ? { scope: 'read_only' as const } : {}) },
      {
        onSuccess: (data) => {
          setCreatedKey(data.key);
          toast.success('API key created');
        },
      }
    );
  };

  const handleCopy = async () => {
    if (!createdKey) return;
    await navigator.clipboard.writeText(createdKey);
    setCopied(true);
    toast.success('Copied to clipboard');
    setTimeout(() => setCopied(false), 2000);
  };

  const handleClose = () => {
    setCreatedKey(null);
    setCopied(false);
    form.reset();
    onClose();
  };

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent>
        {createdKey ? (
          <>
            <DialogHeader>
              <DialogTitle>API Key Created</DialogTitle>
              <DialogDescription>
                Copy your API key now. You won't be able to see it again.
              </DialogDescription>
            </DialogHeader>
            <div className="space-y-3">
              <Alert variant="warning">
                <AlertTriangle className="size-4" />
                <AlertDescription>
                  This is the only time the full key will be displayed. Store it
                  securely.
                </AlertDescription>
              </Alert>
              <div className="flex items-center gap-2">
                <code className="flex-1 break-all rounded-md bg-muted px-3 py-2 font-mono text-sm">
                  {createdKey}
                </code>
                <Button
                  variant="secondary"
                  bordered
                  size="icon"
                  onClick={handleCopy}
                  className="shrink-0"
                >
                  {copied ? (
                    <Check className="size-4" />
                  ) : (
                    <Copy className="size-4" />
                  )}
                </Button>
              </div>
            </div>
            <DialogFooter>
              <Button onClick={handleClose}>Done</Button>
            </DialogFooter>
          </>
        ) : (
          <form onSubmit={form.handleSubmit(handleSubmit)}>
            <DialogHeader>
              <DialogTitle>Create API Key</DialogTitle>
              <DialogDescription>
                Create an API key for MCP or external integrations.
              </DialogDescription>
            </DialogHeader>
            <div className="space-y-4 py-4">
              <div>
                <Label htmlFor="name">Name</Label>
                <Input
                  id="name"
                  placeholder="e.g. MCP Server, CI/CD Pipeline"
                  {...form.register('name')}
                  className="mt-1.5"
                />
                {form.formState.errors.name && (
                  <FieldError className="mt-1">
                    {form.formState.errors.name.message}
                  </FieldError>
                )}
              </div>
              <div className="flex items-start gap-2.5">
                <Controller
                  control={form.control}
                  name="readOnly"
                  render={({ field }) => (
                    <Checkbox
                      id="read-only"
                      checked={field.value}
                      onCheckedChange={(state) => field.onChange(state === true)}
                      className="mt-0.5"
                    />
                  )}
                />
                <div className="space-y-1">
                  <Label htmlFor="read-only" className="font-normal">
                    Read-only
                  </Label>
                  <p className="text-xs text-muted-foreground">
                    The key acts as you, limited to read operations — it can't
                    create, edit, delete, run workflows, or manage API keys.
                    Scope can't be changed later.
                  </p>
                </div>
              </div>
            </div>
            <DialogFooter>
              <Button
                type="button"
                variant="secondary"
                onClick={handleClose}
                disabled={isPending}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={isPending}>
                {isPending ? 'Creating...' : 'Create'}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}
