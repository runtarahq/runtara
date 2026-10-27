import { useEffect } from 'react';
import { useAuth } from 'react-oidc-context';
import logoIcon from '@/assets/logo/runtara-logo-icon.svg';
import { Button } from '@/shared/components/ui/button';
import { Loader } from '@/shared/components/loader.tsx';
import { mountReturnPath } from '@/shared/config/oidcConfig';
import {
  chooseTenant,
  tenantHome,
  type MyTenant,
  type TenantChoice,
} from './myTenants';

/**
 * Shown to a user signed in with a token that names no tenant: at the bare
 * mount it sends them to their only tenant or lets them pick one; at a
 * workspace they do not belong to (`deniedTenant`) it says so and offers
 * the ones they do, never moving them silently.
 */
export function TenantGate({
  tenants,
  deniedTenant,
}: {
  tenants: MyTenant[] | null;
  deniedTenant?: string;
}) {
  const { signoutRedirect } = useAuth();
  const chosen = tenants === null ? null : chooseTenant(tenants);
  const choice: TenantChoice | null =
    deniedTenant && chosen?.kind === 'redirect'
      ? { kind: 'pick', tenants: tenants ?? [] }
      : chosen;

  const redirectTo = choice?.kind === 'redirect' ? choice.id : null;
  useEffect(() => {
    if (redirectTo) {
      window.location.replace(tenantHome(mountReturnPath, redirectTo));
    }
  }, [redirectTo]);

  if (choice === null || choice.kind === 'redirect') {
    return <Loader />;
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <div className="w-full max-w-md text-center">
        <div className="mb-8 flex justify-center">
          <img src={logoIcon} alt="Runtara" className="size-12" />
        </div>
        <div className="rounded-lg border bg-card px-8 py-8 text-left shadow-sm">
          {choice.kind === 'pick' ? (
            <>
              <h1 className="mb-1 text-lg font-semibold">
                {deniedTenant ? 'No access' : 'Choose a workspace'}
              </h1>
              <p className="mb-6 text-sm text-muted-foreground">
                {deniedTenant
                  ? `You are not a member of the workspace ${deniedTenant}. Open one of yours instead.`
                  : 'You are a member of several workspaces on this server.'}
              </p>
              <ul className="flex flex-col gap-2">
                {choice.tenants.map((tenant) => (
                  <li key={tenant.id}>
                    <Button
                      variant="secondary"
                      className="w-full justify-between"
                      onClick={() =>
                        window.location.assign(
                          tenantHome(mountReturnPath, tenant.id)
                        )
                      }
                    >
                      <span className="truncate">{tenant.id}</span>
                      {tenant.role && (
                        <span className="text-xs text-muted-foreground">
                          {tenant.role}
                        </span>
                      )}
                    </Button>
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <>
              <h1 className="mb-1 text-lg font-semibold">No workspace</h1>
              <p className="mb-6 text-sm text-muted-foreground">
                Your account is not a member of any workspace on this server.
                Ask a workspace owner to invite you.
              </p>
              <Button variant="secondary" onClick={() => signoutRedirect()}>
                Sign out
              </Button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
