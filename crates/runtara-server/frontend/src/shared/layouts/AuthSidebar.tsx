import { useAuth } from 'react-oidc-context';
import { LogIn, LogOut } from 'lucide-react';
import { useAuthStore } from '@/shared/stores/authStore.ts';
import { isOidcAuth } from '@/shared/config/runtimeConfig';
import { Button } from '@/shared/components/ui/button';

export function AuthSidebar() {
  const { isAuthenticated, signoutRedirect, signinRedirect } = useAuth();

  const clearUserGroups = useAuthStore((state) => state.clearUserGroups);
  const clearOrgId = useAuthStore((state) => state.clearOrgId);

  // Local auth mode has no OIDC session to enter or leave, so there is nothing
  // to offer. Branching on `isAuthenticated` alone rendered "Sign in" here, and
  // pressing it handed the user to whichever Auth0 tenant was baked into the
  // build — landing them on a 403 outside the app, with only the browser's back
  // button to return.
  if (!isOidcAuth) {
    return null;
  }

  return isAuthenticated ? (
    <Button
      variant="secondary"
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
      variant="secondary"
      size="icon"
      className="relative size-9 shrink-0"
      aria-label="Sign in"
      onClick={() => signinRedirect()}
    >
      {/* Distinct from the sign-out arrow — the two states previously shared
          the same icon, so the control looked identical either way. */}
      <LogIn className="size-4" />
      <span className="sr-only">Sign in</span>
    </Button>
  );
}
