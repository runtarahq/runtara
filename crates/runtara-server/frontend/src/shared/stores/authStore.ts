import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';

/** Effective permission → access level ("allow" | "own"), as returned by GET /api/runtime/me. */
export type Permissions = Record<string, string>;

interface AuthState {
  userGroups: string[];
  orgId: string;
  /** Caller's tenant role from /me (Valkey-sourced). Null outside SaaS enforcement. */
  role: string | null;
  /** Caller's effective permissions from /me. Empty until /me resolves. */
  permissions: Permissions;
  setUserGroups: (groups: string[]) => void;
  clearUserGroups: () => void;
  setOrgId: (orgId: string) => void;
  clearOrgId: () => void;
  setMe: (me: { role: string | null; permissions: Permissions }) => void;
  clearMe: () => void;
}

export const useAuthStore = create<AuthState>()(
  devtools(
    immer((set) => ({
      userGroups: [],
      orgId: '',
      role: null,
      permissions: {},
      setUserGroups: (groups) => set({ userGroups: groups }),
      clearUserGroups: () => set({ userGroups: [] }),
      setOrgId: (orgId) => set({ orgId }),
      clearOrgId: () => set({ orgId: '' }),
      setMe: (me) => set({ role: me.role, permissions: me.permissions }),
      clearMe: () => set({ role: null, permissions: {} }),
    }))
  )
);

/**
 * True if the caller may perform `permission` (its access is "allow" or "own").
 *
 * When no permission set is resolved — membership enforcement disabled, or `/me`
 * not yet loaded — this returns `true` so the UI does not hide controls the
 * server would actually allow. Gating here is UX only; runtara remains the
 * enforcement point.
 */
export function useHasPermission(permission: string): boolean {
  return useAuthStore((state) => {
    if (Object.keys(state.permissions).length === 0) return true;
    return permission in state.permissions;
  });
}

/**
 * Like `useHasPermission`, but default-DENY: `true` only when `/me` explicitly
 * granted `permission`. For opt-in surfaces that don't exist in every
 * deployment (e.g. the user-management link into the managed control plane),
 * where "no permission data yet" must hide the entry rather than show it.
 */
export function useHasExplicitPermission(permission: string): boolean {
  return useAuthStore((state) => permission in state.permissions);
}
