import type { QueryClient } from "@tanstack/react-query";
import type { ConnectionStatus } from "../types";
import type { WSIncomingEvent, WSOutgoingMessage } from "../types/ws";

/** Callback for connection status changes */
export type StatusChangeCallback = (status: ConnectionStatus) => void;

/** Error from the bridge WebSocket server */
export interface BridgeWsError {
  source: "bridge";
  code?: string;
  message: string;
  bridgeId: string;
}

/** Error from the custom persistence/storage layer */
export interface PersistenceOperationError {
  source: "persistence";
  message: string;
  operation: "add" | "remove" | "update" | "refresh";
}

/** All error types the SDK can emit via onError */
export type SdkError = BridgeWsError | PersistenceOperationError;

/** @deprecated Use SdkError instead */
export type BridgeError = BridgeWsError;

/** Callback for errors from the SDK (bridge WS + persistence) */
export type ErrorCallback = (error: SdkError) => void;

/**
 * WebSocket connection manager for a single bridge.
 * Handles:
 * - Connection/reconnection with exponential backoff
 * - Topic subscription with reference counting
 * - Routing messages to React Query cache
 *
 * NOTE: The bridge server **replaces** the entire subscription set on each
 * `subscribe` message. To handle multiple hooks subscribing independently,
 * we batch subscription syncs via microtask so all hooks within a single
 * render commit accumulate before we send one message with the full topic set.
 */
export class WebSocketManager {
  private ws: WebSocket | null = null;
  private subscriptions: Map<string, number> = new Map(); // topic -> refcount
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 5;
  private reconnectTimeoutId: ReturnType<typeof setTimeout> | null = null;
  private isIntentionalClose = false;
  private syncPending = false;

  constructor(
    private bridgeId: string,
    private url: string,
    private queryClient: QueryClient,
    private onStatusChange: StatusChangeCallback,
    private apiKey?: string,
    private onError?: ErrorCallback,
  ) {}

  /**
   * Connect to the WebSocket server.
   * @param explicit - If true, resets intentional-close flag (used by user-facing connect actions)
   */
  connect(explicit = false): void {
    if (
      this.ws?.readyState === WebSocket.OPEN ||
      this.ws?.readyState === WebSocket.CONNECTING
    ) {
      return;
    }

    // After an intentional disconnect, only an explicit connect should reconnect
    if (this.isIntentionalClose && !explicit) {
      return;
    }

    this.isIntentionalClose = false;
    this.onStatusChange("connecting");

    try {
      // Build URL with API key if provided
      const wsUrl = this.apiKey
        ? `${this.url}?api_key=${encodeURIComponent(this.apiKey)}`
        : this.url;

      this.ws = new WebSocket(wsUrl);
      this.setupEventHandlers();
    } catch (error) {
      console.error(`[WS ${this.bridgeId}] Failed to create WebSocket:`, error);
      this.onStatusChange("error");
      this.scheduleReconnect();
    }
  }

  /**
   * Disconnect from the WebSocket server
   */
  disconnect(): void {
    this.isIntentionalClose = true;
    this.clearReconnectTimeout();

    if (this.ws) {
      this.ws.close(1000, "Client disconnect");
      this.ws = null;
    }

    this.subscriptions.clear();
    this.reconnectAttempts = 0;
    this.onStatusChange("disconnected");
  }

  /**
   * Subscribe to a topic. Returns an unsubscribe function.
   * Uses reference counting to avoid duplicate subscriptions.
   *
   * The bridge server replaces the entire subscription set on each
   * subscribe message, so we batch changes via microtask and send
   * the full accumulated topic list once per tick.
   */
  subscribe(topic: string): () => void {
    const currentCount = this.subscriptions.get(topic) ?? 0;
    this.subscriptions.set(topic, currentCount + 1);

    // Schedule a batched sync (coalesces all subscribes within the same microtask)
    if (currentCount === 0) {
      this.scheduleSyncSubscriptions();
    }

    // Return unsubscribe function
    return () => {
      const count = this.subscriptions.get(topic) ?? 0;
      if (count <= 1) {
        this.subscriptions.delete(topic);
      } else {
        this.subscriptions.set(topic, count - 1);
      }

      // Schedule re-sync after topic removal
      if (count <= 1) {
        this.scheduleSyncSubscriptions();
      }
    };
  }

  /**
   * Send a message to the WebSocket server
   */
  send(message: WSOutgoingMessage): void {
    if (this.ws?.readyState !== WebSocket.OPEN) {
      console.warn(`[WS ${this.bridgeId}] Cannot send - not connected`);
      return;
    }

    this.ws.send(JSON.stringify(message));
  }

  /**
   * Check if connected
   */
  isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  private setupEventHandlers(): void {
    if (!this.ws) return;

    this.ws.onopen = () => {
      console.log(`[WS ${this.bridgeId}] Connected`);
      this.reconnectAttempts = 0;
      this.onStatusChange("connected");

      // Resubscribe to all active topics (flush immediately, don't batch)
      this.flushSyncSubscriptions();
    };

    this.ws.onmessage = (event) => {
      try {
        const message = JSON.parse(event.data) as WSIncomingEvent;
        this.handleMessage(message);
      } catch (error) {
        console.error(`[WS ${this.bridgeId}] Failed to parse message:`, error);
      }
    };

    this.ws.onerror = (event) => {
      console.error(`[WS ${this.bridgeId}] Error:`, event);
      this.onStatusChange("error");
    };

    this.ws.onclose = (event) => {
      console.log(
        `[WS ${this.bridgeId}] Closed: code=${event.code}, reason=${event.reason}`,
      );

      if (!this.isIntentionalClose) {
        this.onStatusChange("disconnected");
        this.scheduleReconnect();
      }
    };
  }

  private handleMessage(message: WSIncomingEvent): void {
    switch (message.type) {
      case "system_stats":
        this.queryClient.setQueryData(["stats", this.bridgeId], message.data);
        break;

      case "media_update":
        this.queryClient.setQueryData(["media", this.bridgeId], message.data);
        break;

      case "media_feedback":
        // Could emit an event or update a feedback query
        console.log(`[WS ${this.bridgeId}] Media feedback:`, message.data);
        break;

      case "process_list":
        this.queryClient.setQueryData(["processes", this.bridgeId], message.data);
        break;

      case "process_feedback":
        // Could emit an event or update a feedback query
        console.log(`[WS ${this.bridgeId}] Process feedback:`, message.data);
        break;

      case "connected":
        console.log(`[WS ${this.bridgeId}] Server ack:`, message.data.message);
        break;

      case "error":
        console.error(`[WS ${this.bridgeId}] Server error:`, message.data);
        // Call error callback for DX (toasts, etc.)
        this.onError?.({
          source: "bridge",
          code: message.data.code,
          message: message.data.message,
          bridgeId: this.bridgeId,
        });
        break;

      default:
        console.warn(`[WS ${this.bridgeId}] Unknown message type:`, message);
    }
  }

  /**
   * Schedule a subscription sync via microtask.
   * All subscribe/unsubscribe calls within the same tick are batched
   * into a single message with the full topic set.
   */
  private scheduleSyncSubscriptions(): void {
    if (this.syncPending) return;
    this.syncPending = true;
    queueMicrotask(() => {
      this.syncPending = false;
      this.flushSyncSubscriptions();
    });
  }

  /**
   * Send the full accumulated topic set to the server immediately.
   * If no topics remain, sends an unsubscribe for all.
   */
  private flushSyncSubscriptions(): void {
    if (this.ws?.readyState !== WebSocket.OPEN) return;

    const topics = Array.from(this.subscriptions.keys());
    if (topics.length === 0) return;

    this.ws.send(
      JSON.stringify({
        op: "subscribe",
        data: { topics },
      }),
    );
  }

  private scheduleReconnect(): void {
    if (this.isIntentionalClose) return;
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      console.error(`[WS ${this.bridgeId}] Max reconnect attempts reached`);
      this.onStatusChange("error");
      return;
    }

    const backoffMs = Math.min(1000 * Math.pow(2, this.reconnectAttempts), 30000);
    this.reconnectAttempts++;

    console.log(
      `[WS ${this.bridgeId}] Reconnecting in ${backoffMs}ms (attempt ${this.reconnectAttempts})`,
    );

    this.reconnectTimeoutId = setTimeout(() => {
      this.connect();
    }, backoffMs);
  }

  private clearReconnectTimeout(): void {
    if (this.reconnectTimeoutId) {
      clearTimeout(this.reconnectTimeoutId);
      this.reconnectTimeoutId = null;
    }
  }
}
