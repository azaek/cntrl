/**
 * WebSocket message types for Bridge communication
 * These types align with apps/bridge/src-tauri/src/server/types.rs
 */

import type {
  MediaAction,
  MediaStatus,
  OperationFeedback,
  ProcessListPayload,
  StreamPayload,
} from "./api";

// --- Outgoing Messages (Client -> Bridge) ---

export interface SubscribeRequest {
  topics: string[];
}

export interface WSSubscribeMessage {
  op: "subscribe";
  data: SubscribeRequest;
}

export interface WSUnsubscribeMessage {
  op: "unsubscribe";
  data: SubscribeRequest;
}

export interface WSMediaMessage {
  op: "media";
  data: MediaAction;
}

export interface WSProcessKillMessage {
  op: "process_kill";
  data: {
    pid?: number;
    name?: string;
  };
}

export interface WSProcessFocusMessage {
  op: "process_focus";
  data: {
    pid: number;
  };
}

export interface WSProcessLaunchMessage {
  op: "process_launch";
  data: {
    path: string;
    args?: string[];
  };
}

export type WSOutgoingMessage =
  | WSSubscribeMessage
  | WSUnsubscribeMessage
  | WSMediaMessage
  | WSProcessKillMessage
  | WSProcessFocusMessage
  | WSProcessLaunchMessage;

// --- Incoming Events (Bridge -> Client) ---
// These match BroadcastEvent enum in types.rs

export interface WSSystemStatsEvent {
  type: "system_stats";
  data: StreamPayload;
}

export interface WSMediaUpdateEvent {
  type: "media_update";
  data: MediaStatus;
}

export interface WSMediaFeedbackEvent {
  type: "media_feedback";
  data: OperationFeedback;
}

export interface WSProcessListEvent {
  type: "process_list";
  data: ProcessListPayload;
}

export interface WSProcessFeedbackEvent {
  type: "process_feedback";
  data: OperationFeedback;
}

export interface WSConnectedEvent {
  type: "connected";
  data: {
    message: string;
  };
}

export interface WSErrorEvent {
  type: "error";
  data: {
    message: string;
    code?: string;
  };
}

export type WSIncomingEvent =
  | WSSystemStatsEvent
  | WSMediaUpdateEvent
  | WSMediaFeedbackEvent
  | WSProcessListEvent
  | WSProcessFeedbackEvent
  | WSConnectedEvent
  | WSErrorEvent;

/**
 * WebSocket topics for subscription
 */
export type WSTopic =
  | "stats"
  | "stats.cpu"
  | "stats.memory"
  | "stats.gpu"
  | "stats.disks"
  | "stats.network"
  | "media"
  | "processes";
