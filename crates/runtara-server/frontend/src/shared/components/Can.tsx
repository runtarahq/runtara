import { ReactNode } from 'react';
import { useHasPermission } from '@/shared/stores/authStore.ts';

interface CanProps {
  /** Permission wire string, e.g. `workflow:create` (see GET /api/runtime/me). */
  permission: string;
  /** Rendered when the caller holds the permission. */
  children: ReactNode;
  /** Rendered when the caller lacks it. Defaults to nothing (control hidden). */
  fallback?: ReactNode;
}

/**
 * Renders `children` only when the caller's resolved permissions allow
 * `permission`. Use it to hide mutating controls (create/update/delete/execute)
 * from roles that cannot perform them. Read controls should NOT be wrapped —
 * reads are open to every role.
 *
 * Gating is UX only: runtara enforces the same map server-side, so direct API
 * calls are still rejected. When no permission set is resolved (local/trust_proxy
 * modes, or before `/me` loads) the control is shown — see `useHasPermission`.
 */
export function Can({ permission, children, fallback = null }: CanProps) {
  const allowed = useHasPermission(permission);
  return <>{allowed ? children : fallback}</>;
}
