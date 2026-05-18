import { useEffect, useRef, useState } from "react";

import type { Job, JobUpdate, StationInfo } from "./types";

// Broadcast envelope: a JobUpdate plus the job_id it belongs to. Wire
// format key stays "job_update" — only the TS alias was renamed.
type JobUpdateMessage = { job_id: string; event: JobUpdate };

type FleetEvent =
  | { PeerDiscovered: StationInfo }
  | { PeerUpdated: StationInfo }
  | { PeerLost: string }
  | { LeadChanged: string | null };

export type WsEnvelope =
  | { type: "hello"; tool_version: string }
  | { type: "heartbeat" }
  | { type: "job_update"; job_id: string; event: JobUpdate }
  | { type: "fleet_event"; PeerDiscovered?: StationInfo; PeerUpdated?: StationInfo; PeerLost?: string; LeadChanged?: string | null };

function buildWsUrl(): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  // Vite proxy handles /api ws upgrades transparently.
  return `${proto}//${window.location.host}/api/events`;
}

export interface WsConnection {
  connected: boolean;
  lastEnvelope: WsEnvelope | null;
}

/**
 * Subscribes to the wipestation event stream. Calls `onMessage` for every
 * envelope (job/fleet/hello/heartbeat).
 */
export function useEventStream(onMessage: (env: WsEnvelope) => void): WsConnection {
  const [connected, setConnected] = useState(false);
  const [lastEnvelope, setLast] = useState<WsEnvelope | null>(null);
  const handlerRef = useRef(onMessage);
  handlerRef.current = onMessage;

  useEffect(() => {
    let stop = false;
    let ws: WebSocket | null = null;
    let reconnectTimer: number | null = null;

    const connect = () => {
      if (stop) return;
      ws = new WebSocket(buildWsUrl());
      ws.onopen = () => setConnected(true);
      ws.onclose = () => {
        setConnected(false);
        if (!stop) {
          reconnectTimer = window.setTimeout(connect, 1500);
        }
      };
      ws.onerror = () => {
        // The close handler runs after error, which retries.
      };
      ws.onmessage = (msg) => {
        try {
          const env = JSON.parse(msg.data) as WsEnvelope;
          setLast(env);
          handlerRef.current(env);
        } catch (e) {
          // Ignore malformed messages.
        }
      };
    };

    connect();

    return () => {
      stop = true;
      if (reconnectTimer != null) {
        window.clearTimeout(reconnectTimer);
      }
      ws?.close();
    };
  }, []);

  return { connected, lastEnvelope };
}

/**
 * Hook: track a single job's live state by subscribing to the WS stream
 * and folding the latest events into its state. Initial state comes from
 * the REST endpoint.
 */
export function useJobLiveState(initial: Job | undefined): Job | undefined {
  const [job, setJob] = useState<Job | undefined>(initial);
  useEffect(() => setJob(initial), [initial?.id]);
  useEventStream((env) => {
    if (env.type !== "job_update") return;
    if (!initial || env.job_id !== initial.id) return;
    setJob((prev) => {
      if (!prev) return prev;
      const nextEvents = [...prev.events, env.event];
      let nextState = prev.state;
      let nextProgress = prev.progress;
      if (env.event.event.kind === "state_changed") {
        nextState = env.event.event.to;
      }
      if (env.event.event.kind === "progress") {
        nextProgress = {
          fraction: env.event.event.fraction,
          eta_seconds: env.event.event.eta_seconds,
          stage: env.event.event.stage,
          bytes_processed: env.event.event.bytes_processed,
          bytes_total: env.event.event.bytes_total,
        };
      }
      return { ...prev, events: nextEvents, state: nextState, progress: nextProgress };
    });
  });
  return job;
}

export type { JobUpdateMessage, FleetEvent };
