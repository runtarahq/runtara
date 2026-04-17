import { useAuth } from 'react-oidc-context';
import { LogOut } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { useAuthStore } from '@/shared/stores/authStore.ts';

export default function LogoutButton() {
  const { signoutRedirect } = useAuth();

  const clearUserGroups = useAuthStore((state) => state.clearUserGroups);
  const clearOrgId = useAuthStore((state) => state.clearOrgId);

  return (
    <Button
      className="w-full"
      onClick={() => {
        // Clear tenant-scoped state synchronously before redirect to avoid
        // leaking the previous tenant's orgId into any in-flight requests.
        clearUserGroups();
        clearOrgId();
        signoutRedirect();
      }}
    >
      <LogOut size={16} />
      Sign out
    </Button>
  );
}
