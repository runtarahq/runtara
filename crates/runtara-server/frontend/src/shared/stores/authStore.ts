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

/** True if the caller may perform `permission` (access is "allow" or "own"). */
export function useHasPermission(permission: string): boolean {
  return useAuthStore((state) => permission in state.permissions);
}
