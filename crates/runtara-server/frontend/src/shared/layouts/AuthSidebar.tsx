import { useAuth } from 'react-oidc-context';
import { LogOut } from 'lucide-react';
import { useAuthStore } from '@/shared/stores/authStore.ts';
import { Button } from '@/shared/components/ui/button';

export function AuthSidebar() {
  const { isAuthenticated, signoutRedirect, signinRedirect } = useAuth();

  const clearUserGroups = useAuthStore((state) => state.clearUserGroups);
  const clearOrgId = useAuthStore((state) => state.clearOrgId);

  return isAuthenticated ? (
    <Button
      variant="ghost"
      size="icon"
      className="relative size-9 shrink-0"
      aria-label="Sign out"
      onClick={() => {
        clearUserGroups();
        clearOrgId();
        signoutRedirect();
      }}
    >
      <LogOut className="size-4" />
      <span className="sr-only">Sign out</span>
    </Button>
  ) : (
    <Button
      variant="ghost"
      size="icon"
      className="relative size-9 shrink-0"
      aria-label="Sign in"
      onClick={() => signinRedirect()}
    >
      <LogOut className="size-4" />
      <span className="sr-only">Sign in</span>
    </Button>
  );
}
