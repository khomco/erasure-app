import { useEffect, useRef, useState } from "react";

import type { ErasureEvent, Job, JobActivity, JobStateLabel, JobUpdate, StationInfo } from "./types";

/**
 * The server's unified Job broadcast envelope (see wipe_engine::JobBroadcast).
 * One of three variants — outer Job state change, a new typed activity
 * was appended, or an inner ErasureEvent update.
 */
export type JobBroadcast =
  | {
      kind: "job_state_changed";
      job_id: string;
      from: { state: JobStateLabel };
      to: { state: JobStateLabel };
      at: string;
    }
  | { kind: "activity_added"; job_id: string; activity: JobActivity }
  | { kind: "erasure_update"; job_id: string; erasure_event_id: string; update: JobUpdate };

export type FleetEvent =
  | { PeerDiscovered: StationInfo }
  | { PeerUpdated: StationInfo }
  | { PeerLost: string }
  | { LeadChanged: string | null };

export type WsEnvelope =
  | { type: "hello"; tool_version: string }
  | { type: "heartbeat" }
  | { type: "job_broadcast"; payload: JobBroadcast }
  | { type: "fleet_event"; PeerDiscovered?: StationInfo; PeerUpdated?: StationInfo; PeerLost?: string; LeadChanged?: string | null };

function buildWsUrl(): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
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
        } catch {
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
 * Hook: track a single outer Job's live state by folding broadcast events
 * into its activity chain and outer state. Initial state comes from the
 * REST endpoint.
 */
export function useJobLiveState(initial: Job | undefined): Job | undefined {
  const [job, setJob] = useState<Job | undefined>(initial);
  useEffect(() => setJob(initial), [initial?.id]);
  useEventStream((env) => {
    if (env.type !== "job_broadcast") return;
    const b = env.payload;
    const targetId = "job_id" in b ? b.job_id : null;
    if (!initial || targetId !== initial.id) return;
    setJob((prev) => {
      if (!prev) return prev;
      switch (b.kind) {
        case "job_state_changed":
          return { ...prev, state: b.to };
        case "activity_added":
          return { ...prev, activities: [...prev.activities, b.activity] };
        case "erasure_update": {
          const next = prev.activities.map((a) => {
            if (a.type !== "erasure" || a.id !== b.erasure_event_id) return a;
            const erasure = a as JobActivity & { type: "erasure" };
            const nextEvents = [...(erasure as unknown as ErasureEvent).events, b.update];
            let nextState = (erasure as unknown as ErasureEvent).state;
            let nextProgress = (erasure as unknown as ErasureEvent).progress;
            if (b.update.event.kind === "state_changed") {
              nextState = b.update.event.to;
            }
            if (b.update.event.kind === "progress") {
              nextProgress = {
                fraction: b.update.event.fraction,
                eta_seconds: b.update.event.eta_seconds,
                stage: b.update.event.stage,
                bytes_processed: b.update.event.bytes_processed,
                bytes_total: b.update.event.bytes_total,
              };
            }
            return {
              ...erasure,
              events: nextEvents,
              state: nextState,
              progress: nextProgress,
            };
          });
          return { ...prev, activities: next };
        }
      }
    });
  });
  return job;
}

/** Convenience: most-recent ErasureEvent activity on a Job, if any. */
export function latestErasure(job: Job | undefined): ErasureEvent | undefined {
  if (!job) return undefined;
  for (let i = job.activities.length - 1; i >= 0; i--) {
    const a = job.activities[i];
    if (a.type === "erasure") {
      return a as unknown as ErasureEvent;
    }
  }
  return undefined;
}
