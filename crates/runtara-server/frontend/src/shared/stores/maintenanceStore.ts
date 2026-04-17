import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';

interface MaintenanceState {
  isMaintenanceMode: boolean;
  setMaintenanceMode: (value: boolean) => void;
}

export const useMaintenanceStore = create<MaintenanceState>()(
  devtools(
    immer((set) => ({
      isMaintenanceMode: false,
      setMaintenanceMode: (value) => set({ isMaintenanceMode: value }),
    }))
  )
);
