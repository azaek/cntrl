import { useQueryClient } from "@tanstack/react-query";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useStore, type StoreApi } from "zustand";
import {
  createPlainBridgesStore,
  useBridgesStore,
  type BridgesState,
} from "../store/bridges-store";
import type {
  BridgeConnection,
  BridgePersistence,
  ConnectionStatus,
  StoredBridge,
} from "../types";
import {
  WebSocketManager,
  type ErrorCallback,
  type PersistenceOperationError,
} from "../ws/ws-manager";

/** Storage layer status for the initial load */
export type PersistenceStatus = "idle" | "loading" | "error";

interface BridgesContextValue {
  /** All bridges with runtime state */
  bridges: Map<string, BridgeConnection>;

  /** Add a new bridge and return its ID */
  addBridge: (bridge: Omit<StoredBridge, "id">) => Promise<string>;

  /** Remove a bridge by ID */
  removeBridge: (id: string) => Promise<void>;

  /** Update a bridge's configuration */
  updateBridge: (id: string, updates: Partial<Omit<StoredBridge, "id">>) => Promise<void>;

  /** Connect to a bridge (resets intentional-close flag) */
  connect: (id: string) => void;

  /** Disconnect from a bridge */
  disconnect: (id: string) => void;

  /** Get WebSocket manager for a bridge (for hooks to use) */
  getWsManager: (id: string) => WebSocketManager | undefined;

  /** Whether the store has been hydrated / loaded */
  ready: boolean;

  /** Re-load bridges from persistence (no-op for default localStorage path) */
  refresh: () => Promise<void>;

  /** Whether a custom persistence layer is in use (vs default localStorage) */
  isCustomStorage: boolean;

  /** Status of the initial data load ("idle" | "loading" | "error") */
  persistenceStatus: PersistenceStatus;

  /** Error message from the initial load (null when no error) */
  persistenceError: string | null;

  /**
   * @internal Connect without resetting intentional-close flag.
   * Used by hooks for auto/eager connection modes so that manual disconnect
   * is respected and hooks don't create reconnect loops.
   */
  _hookConnect: (id: string) => void;
}

const BridgesContext = createContext<BridgesContextValue | null>(null);

interface BridgesProviderProps {
  children: ReactNode;
  /** Auto-connect to all bridges on mount (default: true) */
  autoConnect?: boolean;
  /** Callback for error messages from bridges (useful for toasts) */
  onError?: ErrorCallback;
  /** Custom persistence layer. When omitted, uses localStorage via Zustand persist. */
  persistence?: BridgePersistence;
}

export function BridgesProvider({
  children,
  autoConnect = true,
  onError,
  persistence,
}: BridgesProviderProps) {
  const queryClient = useQueryClient();

  // --- Store selection ---
  // Default path: use the global persisted store
  // Custom path: create a plain (non-persisted) store
  const plainStoreRef = useRef<StoreApi<BridgesState> | null>(null);
  if (persistence && !plainStoreRef.current) {
    plainStoreRef.current = createPlainBridgesStore();
  }

  const useCustomStore = plainStoreRef.current;

  // Subscribe to stored bridges from the appropriate store
  const storedBridgesDefault = useBridgesStore((s) => s.bridges);
  const storedBridgesCustom = useStore(
    useCustomStore ?? useBridgesStore,
    (s) => s.bridges,
  );
  const storedBridges = persistence ? storedBridgesCustom : storedBridgesDefault;

  const hasHydratedDefault = useBridgesStore((s) => s._hasHydrated);

  // Runtime state
  const [statuses, setStatuses] = useState<Record<string, ConnectionStatus>>({});
  const [ready, setReady] = useState(!persistence ? hasHydratedDefault : false);
  const wsManagersRef = useRef<Map<string, WebSocketManager>>(new Map());
  const persistenceRef = useRef(persistence);
  persistenceRef.current = persistence;

  // Persistence status tracking (only for initial load)
  const isCustomStorage = !!persistence;
  const [persistenceStatus, setPersistenceStatus] = useState<PersistenceStatus>(
    persistence ? "loading" : "idle",
  );
  const [persistenceError, setPersistenceError] = useState<string | null>(null);

  // Fire onError for persistence operation failures (add/remove/update/refresh)
  const emitPersistenceError = useCallback(
    (operation: PersistenceOperationError["operation"], err: unknown) => {
      const message = err instanceof Error ? err.message : String(err);
      onError?.({ source: "persistence", message, operation });
    },
    [onError],
  );

  // Update ready for default path when hydration completes
  useEffect(() => {
    if (!persistence && hasHydratedDefault) {
      setReady(true);
    }
  }, [persistence, hasHydratedDefault]);

  // Build bridge URL
  const buildWsUrl = useCallback((bridge: StoredBridge): string => {
    const protocol = bridge.config.secure ? "wss" : "ws";
    return `${protocol}://${bridge.config.host}:${bridge.config.port}/api/ws`;
  }, []);

  // Create or get WebSocket manager for a bridge
  const getOrCreateWsManager = useCallback(
    (bridge: StoredBridge): WebSocketManager => {
      let manager = wsManagersRef.current.get(bridge.id);

      if (!manager) {
        manager = new WebSocketManager(
          bridge.id,
          buildWsUrl(bridge),
          queryClient,
          (status) => {
            setStatuses((prev) => ({ ...prev, [bridge.id]: status }));
          },
          bridge.config.apiKey,
          onError,
        );
        wsManagersRef.current.set(bridge.id, manager);
      }

      return manager;
    },
    [buildWsUrl, queryClient, onError],
  );

  // Internal connect helper
  const connectInternal = useCallback(
    (id: string, explicit: boolean) => {
      const store = persistence ? useCustomStore : useBridgesStore;
      const bridge = store?.getState().bridges[id];
      if (!bridge) {
        console.warn(`[BridgesProvider] Bridge not found: ${id}`);
        return;
      }

      const manager = getOrCreateWsManager(bridge);
      manager.connect(explicit);
    },
    [persistence, useCustomStore, getOrCreateWsManager],
  );

  // Connect to a bridge (user-facing, resets intentional-close flag)
  const connect = useCallback(
    (id: string) => connectInternal(id, true),
    [connectInternal],
  );

  // Hook connect (non-explicit, respects intentional-close flag)
  const _hookConnect = useCallback(
    (id: string) => connectInternal(id, false),
    [connectInternal],
  );

  // Disconnect from a bridge
  const disconnect = useCallback((id: string) => {
    const manager = wsManagersRef.current.get(id);
    if (manager) {
      manager.disconnect();
    }
  }, []);

  // Cleanup helper for removing a bridge's WS + cache
  const cleanupBridge = useCallback(
    (id: string) => {
      disconnect(id);
      wsManagersRef.current.delete(id);
      setStatuses((prev) => {
        const { [id]: _removed, ...rest } = prev;
        void _removed;
        return rest;
      });
      queryClient.removeQueries({ queryKey: ["stats", id] });
      queryClient.removeQueries({ queryKey: ["media", id] });
      queryClient.removeQueries({ queryKey: ["processes", id] });
      queryClient.removeQueries({ queryKey: ["system-info", id] });
    },
    [disconnect, queryClient],
  );

  // Add bridge
  const addBridge = useCallback(
    async (bridge: Omit<StoredBridge, "id">): Promise<string> => {
      if (persistence && useCustomStore) {
        const id = crypto.randomUUID();
        const storedBridge: StoredBridge = { ...bridge, id };
        try {
          await persistence.onBridgeAdd(storedBridge);
        } catch (err) {
          emitPersistenceError("add", err);
          throw err;
        }
        useCustomStore.setState((state) => ({
          bridges: { ...state.bridges, [id]: storedBridge },
        }));
        setStatuses((prev) => ({ ...prev, [id]: "disconnected" }));
        if (autoConnect) {
          setTimeout(() => connect(id), 0);
        }
        return id;
      }

      // Default path
      const id = useBridgesStore.getState().addBridge(bridge);
      setStatuses((prev) => ({ ...prev, [id]: "disconnected" }));
      if (autoConnect) {
        setTimeout(() => connect(id), 0);
      }
      return id;
    },
    [persistence, useCustomStore, autoConnect, connect, emitPersistenceError],
  );

  // Remove bridge
  const removeBridge = useCallback(
    async (id: string): Promise<void> => {
      if (persistence && useCustomStore) {
        try {
          await persistence.onBridgeRemove(id);
        } catch (err) {
          emitPersistenceError("remove", err);
          throw err;
        }
        useCustomStore.getState().removeBridge(id);
        cleanupBridge(id);
        return;
      }

      useBridgesStore.getState().removeBridge(id);
      cleanupBridge(id);
    },
    [persistence, useCustomStore, cleanupBridge, emitPersistenceError],
  );

  // Update bridge
  const updateBridge = useCallback(
    async (id: string, updates: Partial<Omit<StoredBridge, "id">>): Promise<void> => {
      if (persistence && useCustomStore) {
        try {
          await persistence.onBridgeUpdate(id, updates);
        } catch (err) {
          emitPersistenceError("update", err);
          throw err;
        }
        useCustomStore.getState().updateBridge(id, updates);
      } else {
        useBridgesStore.getState().updateBridge(id, updates);
      }

      // If config changed, reconnect
      if (updates.config) {
        const existingManager = wsManagersRef.current.get(id);
        if (existingManager?.isConnected()) {
          disconnect(id);
          wsManagersRef.current.delete(id);
          setTimeout(() => connect(id), 0);
        }
      }
    },
    [persistence, useCustomStore, disconnect, connect, emitPersistenceError],
  );

  // Get WebSocket manager (for hooks)
  const getWsManager = useCallback((id: string): WebSocketManager | undefined => {
    return wsManagersRef.current.get(id);
  }, []);

  // Refresh: reload from persistence
  const refresh = useCallback(async (): Promise<void> => {
    if (!persistence || !useCustomStore) return;

    let loaded: StoredBridge[];
    try {
      loaded = await persistence.load();
    } catch (err) {
      emitPersistenceError("refresh", err);
      throw err;
    }

    const store = useCustomStore.getState();
    const currentIds = new Set(Object.keys(store.bridges));
    const loadedMap: Record<string, StoredBridge> = {};
    const loadedIds = new Set<string>();

    for (const bridge of loaded) {
      loadedMap[bridge.id] = bridge;
      loadedIds.add(bridge.id);
    }

    // Remove bridges no longer in persistence
    for (const id of currentIds) {
      if (!loadedIds.has(id)) {
        cleanupBridge(id);
      }
    }

    // Set full state
    useCustomStore.setState({ bridges: loadedMap });

    // Connect new bridges
    if (autoConnect) {
      for (const id of loadedIds) {
        if (!currentIds.has(id)) {
          setStatuses((prev) => ({ ...prev, [id]: "disconnected" }));
          setTimeout(() => connect(id), 0);
        }
      }
    }
  }, [
    persistence,
    useCustomStore,
    cleanupBridge,
    autoConnect,
    connect,
    emitPersistenceError,
  ]);

  // Initial load for custom persistence
  useEffect(() => {
    if (!persistence || !useCustomStore) return;

    let cancelled = false;
    setPersistenceStatus("loading");
    setPersistenceError(null);
    persistence
      .load()
      .then((loaded) => {
        if (cancelled) return;
        const bridgesMap: Record<string, StoredBridge> = {};
        for (const bridge of loaded) {
          bridgesMap[bridge.id] = bridge;
        }
        useCustomStore.setState({ bridges: bridgesMap, _hasHydrated: true });
        setPersistenceStatus("idle");
        setReady(true);
      })
      .catch((err) => {
        if (cancelled) return;
        const message = err instanceof Error ? err.message : String(err);
        setPersistenceStatus("error");
        setPersistenceError(message);
      });

    return () => {
      cancelled = true;
    };
    // Only run once on mount
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Auto-connect to all bridges after ready
  useEffect(() => {
    if (!autoConnect || !ready) return;

    Object.keys(storedBridges).forEach((id) => {
      if (!statuses[id]) {
        setStatuses((prev) => ({ ...prev, [id]: "disconnected" }));
      }
      const currentStatus = statuses[id];
      if (currentStatus !== "connected" && currentStatus !== "connecting") {
        connect(id);
      }
    });
  }, [ready, autoConnect, storedBridges, statuses, connect]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      wsManagersRef.current.forEach((manager) => {
        manager.disconnect();
      });
      wsManagersRef.current.clear();
    };
  }, []);

  // Build bridges map with runtime state
  const bridges = useMemo(() => {
    const map = new Map<string, BridgeConnection>();

    Object.values(storedBridges).forEach((bridge) => {
      map.set(bridge.id, {
        ...bridge,
        status: statuses[bridge.id] ?? "disconnected",
      });
    });

    return map;
  }, [storedBridges, statuses]);

  const value = useMemo(
    (): BridgesContextValue => ({
      bridges,
      addBridge,
      removeBridge,
      updateBridge,
      connect,
      disconnect,
      getWsManager,
      ready,
      refresh,
      isCustomStorage,
      persistenceStatus,
      persistenceError,
      _hookConnect,
    }),
    [
      bridges,
      addBridge,
      removeBridge,
      updateBridge,
      connect,
      disconnect,
      getWsManager,
      ready,
      refresh,
      isCustomStorage,
      persistenceStatus,
      persistenceError,
      _hookConnect,
    ],
  );

  return <BridgesContext.Provider value={value}>{children}</BridgesContext.Provider>;
}

/**
 * Hook to access the bridges context
 */
export function useBridgesContext(): BridgesContextValue {
  const context = useContext(BridgesContext);
  if (!context) {
    throw new Error("useBridgesContext must be used within a BridgesProvider");
  }
  return context;
}
