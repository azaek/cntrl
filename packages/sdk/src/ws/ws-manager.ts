import type { QueryClient } from "@tanstack/react-query";
import type { ConnectionStatus } from "../types";
import type { WSIncomingEvent, WSOutgoingMessage } from "../types/ws";

/** Callback for connection status changes */
export type StatusChangeCallback = (status: ConnectionStatus) => void;

/**
 * WebSocket connection manager for a single bridge.
 * Handles:
 * - Connection/reconnection with exponential backoff
 * - Topic subscription with reference counting
 * - Routing messages to React Query cache
 */
export class WebSocketManager {
  private ws: WebSocket | null = null;
  private subscriptions: Map<string, number> = new Map(); // topic -> refcount
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 5;
  private reconnectTimeoutId: ReturnType<typeof setTimeout> | null = null;
  private isIntentionalClose = false;

  constructor(
    private bridgeId: string,
    private url: string,
    private queryClient: QueryClient,
    private onStatusChange: StatusChangeCallback,
    private apiKey?: string,
  ) {}

  /**
   * Connect to the WebSocket server
   */
  connect(): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
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
   */
  subscribe(topic: string): () => void {
    const currentCount = this.subscriptions.get(topic) ?? 0;
    this.subscriptions.set(topic, currentCount + 1);

    // Only send subscribe message if this is the first subscriber
    if (currentCount === 0) {
      this.sendSubscribe([topic]);
    }

    // Return unsubscribe function
    return () => {
      const count = this.subscriptions.get(topic) ?? 0;
      if (count <= 1) {
        this.subscriptions.delete(topic);
        this.sendUnsubscribe([topic]);
      } else {
        this.subscriptions.set(topic, count - 1);
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

      // Resubscribe to all active topics
      const topics = Array.from(this.subscriptions.keys());
      if (topics.length > 0) {
        this.sendSubscribe(topics);
      }
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
        break;

      default:
        console.warn(`[WS ${this.bridgeId}] Unknown message type:`, message);
    }
  }

  private sendSubscribe(topics: string[]): void {
    if (this.ws?.readyState !== WebSocket.OPEN || topics.length === 0) return;

    this.ws.send(
      JSON.stringify({
        op: "subscribe",
        data: { topics },
      }),
    );
  }

  private sendUnsubscribe(topics: string[]): void {
    if (this.ws?.readyState !== WebSocket.OPEN || topics.length === 0) return;

    this.ws.send(
      JSON.stringify({
        op: "unsubscribe",
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
