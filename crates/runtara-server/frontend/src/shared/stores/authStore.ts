import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';

interface AuthState {
  userGroups: string[];
  orgId: string;
  setUserGroups: (groups: string[]) => void;
  clearUserGroups: () => void;
  setOrgId: (orgId: string) => void;
  clearOrgId: () => void;
}

export const useAuthStore = create<AuthState>()(
  devtools(
    immer((set) => ({
      userGroups: [],
      orgId: '',
      setUserGroups: (groups) => set({ userGroups: groups }),
      clearUserGroups: () => set({ userGroups: [] }),
      setOrgId: (orgId) => set({ orgId }),
      clearOrgId: () => set({ orgId: '' }),
    }))
  )
);
