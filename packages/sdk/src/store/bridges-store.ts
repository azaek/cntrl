import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { StoredBridge } from "../types";

interface BridgesState {
  /** Persisted bridge configurations */
  bridges: Record<string, StoredBridge>;

  /** Whether the store has been hydrated from localStorage */
  _hasHydrated: boolean;

  /** Set hydration state (called by persist middleware) */
  _setHasHydrated: (state: boolean) => void;

  /** Add a new bridge and return its ID */
  addBridge: (bridge: Omit<StoredBridge, "id">) => string;

  /** Remove a bridge by ID */
  removeBridge: (id: string) => void;

  /** Update a bridge's configuration */
  updateBridge: (id: string, updates: Partial<Omit<StoredBridge, "id">>) => void;

  /** Get a bridge by ID */
  getBridge: (id: string) => StoredBridge | undefined;

  /** Get all bridges as array */
  getAllBridges: () => StoredBridge[];
}

export const useBridgesStore = create<BridgesState>()(
  persist(
    (set, get) => ({
      bridges: {},
      _hasHydrated: false,

      _setHasHydrated: (state) => {
        set({ _hasHydrated: state });
      },

      addBridge: (bridge) => {
        const id = crypto.randomUUID();
        const storedBridge: StoredBridge = { ...bridge, id };
        set((state) => ({
          bridges: { ...state.bridges, [id]: storedBridge },
        }));
        return id;
      },

      removeBridge: (id) => {
        set((state) => {
          const { [id]: _removed, ...rest } = state.bridges;
          void _removed; // Silence unused variable warning
          return { bridges: rest };
        });
      },

      updateBridge: (id, updates) => {
        set((state) => {
          const existing = state.bridges[id];
          if (!existing) return state;
          return {
            bridges: {
              ...state.bridges,
              [id]: { ...existing, ...updates },
            },
          };
        });
      },

      getBridge: (id) => get().bridges[id],

      getAllBridges: () => Object.values(get().bridges),
    }),
    {
      name: "cntrl-bridges", // localStorage key
      onRehydrateStorage: () => (state) => {
        state?._setHasHydrated(true);
      },
    },
  ),
);
