import type { BridgeConfig, ConnectionStatus } from "./types";
import type { StatusResponse, SystemInfoResponse } from "./types/api";

/**
 * Low-level Cntrl Bridge client for REST API communication.
 * For most use cases, prefer the React hooks instead.
 */
export class BridgeClient {
  private config: BridgeConfig;
  private status: ConnectionStatus = "disconnected";

  constructor(config: BridgeConfig) {
    this.config = config;
  }

  /**
   * Get the base URL for the bridge API
   */
  get baseUrl(): string {
    const protocol = this.config.secure ? "https" : "http";
    return `${protocol}://${this.config.host}:${this.config.port}`;
  }

  /**
   * Get the WebSocket URL for the bridge
   */
  get wsUrl(): string {
    const protocol = this.config.secure ? "wss" : "ws";
    return `${protocol}://${this.config.host}:${this.config.port}/api/ws`;
  }

  /**
   * Get default headers (including API key if configured)
   */
  private get headers(): HeadersInit {
    const headers: HeadersInit = {};
    if (this.config.apiKey) {
      headers.Authorization = `Bearer ${this.config.apiKey}`;
    }
    return headers;
  }

  /**
   * Get current connection status
   */
  getConnectionStatus(): ConnectionStatus {
    return this.status;
  }

  /**
   * Health check - always public, no auth required
   */
  async ping(): Promise<boolean> {
    try {
      const response = await fetch(`${this.baseUrl}/api/status`);
      return response.ok;
    } catch {
      return false;
    }
  }

  /**
   * Get health check with details
   */
  async getStatus(): Promise<StatusResponse> {
    const response = await fetch(`${this.baseUrl}/api/status`);
    if (!response.ok) {
      throw new Error(`Failed to get status: ${response.statusText}`);
    }
    return response.json();
  }

  /**
   * Get static system info (CPU, GPU, RAM specs)
   */
  async getSystemInfo(): Promise<SystemInfoResponse> {
    const response = await fetch(`${this.baseUrl}/api/system`, {
      headers: this.headers,
    });
    if (!response.ok) {
      throw new Error(`Failed to get system info: ${response.statusText}`);
    }
    return response.json();
  }

  /**
   * Power control: shutdown
   */
  async shutdown(): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/pw/shutdown`, {
      method: "POST",
      headers: this.headers,
    });
    if (!response.ok) {
      throw new Error(`Failed to shutdown: ${response.statusText}`);
    }
  }

  /**
   * Power control: restart
   */
  async restart(): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/pw/restart`, {
      method: "POST",
      headers: this.headers,
    });
    if (!response.ok) {
      throw new Error(`Failed to restart: ${response.statusText}`);
    }
  }

  /**
   * Power control: sleep
   */
  async sleep(): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/pw/sleep`, {
      method: "POST",
      headers: this.headers,
    });
    if (!response.ok) {
      throw new Error(`Failed to sleep: ${response.statusText}`);
    }
  }

  /**
   * Power control: hibernate
   */
  async hibernate(): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/pw/hibernate`, {
      method: "POST",
      headers: this.headers,
    });
    if (!response.ok) {
      throw new Error(`Failed to hibernate: ${response.statusText}`);
    }
  }

  /**
   * Kill a process by PID
   */
  async killProcess(pid: number): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/processes/kill`, {
      method: "POST",
      headers: {
        ...this.headers,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ pid }),
    });
    if (!response.ok) {
      throw new Error(`Failed to kill process: ${response.statusText}`);
    }
  }

  /**
   * Kill a process by name
   */
  async killProcessByName(name: string): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/processes/kill`, {
      method: "POST",
      headers: {
        ...this.headers,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ name }),
    });
    if (!response.ok) {
      throw new Error(`Failed to kill process: ${response.statusText}`);
    }
  }

  /**
   * Launch an application
   */
  async launchProcess(path: string, args?: string[]): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/processes/launch`, {
      method: "POST",
      headers: {
        ...this.headers,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ path, args }),
    });
    if (!response.ok) {
      throw new Error(`Failed to launch process: ${response.statusText}`);
    }
  }
}

/**
 * Create a new bridge client instance
 */
export function createBridgeClient(config: BridgeConfig): BridgeClient {
  return new BridgeClient(config);
}
