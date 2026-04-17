import { toast } from 'sonner';
import { ConfirmationDialog } from '@/shared/components/confirmation-dialog';
import { useRevokeApiKey } from '../../hooks/useApiKeys';

interface RevokeApiKeyDialogProps {
  open: boolean;
  keyId: string | null;
  keyName: string;
  onClose: () => void;
}

export function RevokeApiKeyDialog({
  open,
  keyId,
  keyName,
  onClose,
}: RevokeApiKeyDialogProps) {
  const { mutate: revoke, isPending } = useRevokeApiKey();

  const handleConfirm = () => {
    if (!keyId) return;
    revoke(keyId, {
      onSuccess: () => {
        toast.success('API key revoked');
        onClose();
      },
    });
  };

  return (
    <ConfirmationDialog
      open={open}
      title="Revoke API Key"
      description={`Are you sure you want to revoke "${keyName}"? Any integrations using this key will stop working immediately.`}
      loading={isPending}
      onClose={onClose}
      onConfirm={handleConfirm}
    />
  );
}
