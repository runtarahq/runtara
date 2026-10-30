import { Building2, Check } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import { mountReturnPath, tenantIdFromUrl } from '@/shared/config/oidcConfig';
import { tenantHome, useMyTenants } from './myTenants';

/** Switch between the workspaces the user belongs to; hidden with fewer than two. */
export function TenantSwitcher({ className }: { className?: string }) {
  const tenants = useMyTenants();
  if (!tenants || tenants.length < 2) return null;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="secondary"
          size="icon"
          className={cn('relative size-9 shrink-0', className)}
          aria-label="Switch workspace"
        >
          <Building2 className="size-4" />
          <span className="sr-only">Switch workspace</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start">
        <DropdownMenuLabel>Workspaces</DropdownMenuLabel>
        {tenants.map((tenant) => (
          <DropdownMenuItem
            key={tenant.id}
            onSelect={() => {
              if (tenant.id !== tenantIdFromUrl) {
                window.location.assign(tenantHome(mountReturnPath, tenant.id));
              }
            }}
          >
            <Check
              className={cn(
                'size-4',
                tenant.id === tenantIdFromUrl ? 'opacity-100' : 'opacity-0'
              )}
            />
            <span className="truncate">{tenant.id}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
