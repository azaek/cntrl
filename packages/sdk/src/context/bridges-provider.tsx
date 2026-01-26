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
import { useBridgesStore } from "../store/bridges-store";
import type { BridgeConnection, ConnectionStatus, StoredBridge } from "../types";
import { WebSocketManager } from "../ws/ws-manager";

interface BridgesContextValue {
  /** All bridges with runtime state */
  bridges: Map<string, BridgeConnection>;

  /** Add a new bridge and return its ID */
  addBridge: (bridge: Omit<StoredBridge, "id">) => string;

  /** Remove a bridge by ID */
  removeBridge: (id: string) => void;

  /** Update a bridge's configuration */
  updateBridge: (id: string, updates: Partial<Omit<StoredBridge, "id">>) => void;

  /** Connect to a bridge */
  connect: (id: string) => void;

  /** Disconnect from a bridge */
  disconnect: (id: string) => void;

  /** Get WebSocket manager for a bridge (for hooks to use) */
  getWsManager: (id: string) => WebSocketManager | undefined;
}

const BridgesContext = createContext<BridgesContextValue | null>(null);

interface BridgesProviderProps {
  children: ReactNode;
  /** Auto-connect to all bridges on mount (default: true) */
  autoConnect?: boolean;
}

export function BridgesProvider({ children, autoConnect = true }: BridgesProviderProps) {
  const queryClient = useQueryClient();

  // Zustand store for persistence
  const storedBridges = useBridgesStore((state) => state.bridges);
  const storeAddBridge = useBridgesStore((state) => state.addBridge);
  const storeRemoveBridge = useBridgesStore((state) => state.removeBridge);
  const storeUpdateBridge = useBridgesStore((state) => state.updateBridge);

  // Runtime state: connection statuses
  const [statuses, setStatuses] = useState<Record<string, ConnectionStatus>>({});

  // WebSocket managers (one per bridge)
  const wsManagersRef = useRef<Map<string, WebSocketManager>>(new Map());

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
        );
        wsManagersRef.current.set(bridge.id, manager);
      }

      return manager;
    },
    [buildWsUrl, queryClient],
  );

  // Connect to a bridge
  const connect = useCallback(
    (id: string) => {
      const bridge = storedBridges[id];
      if (!bridge) {
        console.warn(`[BridgesProvider] Bridge not found: ${id}`);
        return;
      }

      const manager = getOrCreateWsManager(bridge);
      manager.connect();
    },
    [storedBridges, getOrCreateWsManager],
  );

  // Disconnect from a bridge
  const disconnect = useCallback((id: string) => {
    const manager = wsManagersRef.current.get(id);
    if (manager) {
      manager.disconnect();
    }
  }, []);

  // Add bridge (wraps store + auto-connect if enabled)
  const addBridge = useCallback(
    (bridge: Omit<StoredBridge, "id">) => {
      const id = storeAddBridge(bridge);
      setStatuses((prev) => ({ ...prev, [id]: "disconnected" }));

      if (autoConnect) {
        // Defer connection to next tick to allow state to update
        setTimeout(() => connect(id), 0);
      }

      return id;
    },
    [storeAddBridge, autoConnect, connect],
  );

  // Remove bridge (disconnect + cleanup)
  const removeBridge = useCallback(
    (id: string) => {
      disconnect(id);
      wsManagersRef.current.delete(id);
      storeRemoveBridge(id);
      setStatuses((prev) => {
        const { [id]: _removed, ...rest } = prev;
        void _removed; // Silence unused variable warning
        return rest;
      });

      // Clear React Query cache for this bridge
      queryClient.removeQueries({ queryKey: ["stats", id] });
      queryClient.removeQueries({ queryKey: ["media", id] });
      queryClient.removeQueries({ queryKey: ["processes", id] });
      queryClient.removeQueries({ queryKey: ["system-info", id] });
    },
    [disconnect, storeRemoveBridge, queryClient],
  );

  // Update bridge
  const updateBridge = useCallback(
    (id: string, updates: Partial<Omit<StoredBridge, "id">>) => {
      storeUpdateBridge(id, updates);

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
    [storeUpdateBridge, disconnect, connect],
  );

  // Get WebSocket manager (for hooks)
  const getWsManager = useCallback((id: string): WebSocketManager | undefined => {
    return wsManagersRef.current.get(id);
  }, []);

  // Track if store has hydrated
  const hasHydrated = useBridgesStore((state) => state._hasHydrated);

  // Auto-connect to all bridges after hydration
  useEffect(() => {
    if (!autoConnect || !hasHydrated) return;

    Object.keys(storedBridges).forEach((id) => {
      // Initialize status if not already set
      if (!statuses[id]) {
        setStatuses((prev) => ({ ...prev, [id]: "disconnected" }));
      }
      // Only connect if not already connected/connecting
      const currentStatus = statuses[id];
      if (currentStatus !== "connected" && currentStatus !== "connecting") {
        connect(id);
      }
    });
  }, [hasHydrated, autoConnect, storedBridges, statuses, connect]);

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
    }),
    [bridges, addBridge, removeBridge, updateBridge, connect, disconnect, getWsManager],
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
