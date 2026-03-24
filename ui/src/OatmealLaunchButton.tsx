/// <reference types="vite/client" />
import React from "react";
import "./OatmealLaunchButton.css";

function getDownloadUrl(override?: string): string | null {
  if (override) return override;
  if (import.meta.env.DEV) return "/oatmeal";
  return (import.meta.env.VITE_OATMEAL_DOWNLOAD_URL as string | undefined) ?? null;
}

export type Phase = "idle" | "waiting" | "downloading" | "run_it" | "connected" | "error";

export interface OatmealLaunchButtonProps {
  downloadUrl?: string;
  host?: string;
  port?: number;
  storageKey?: string;
  pollIntervalMs?: number;
  connectTimeoutMs?: number;
  postDownloadTimeoutMs?: number;
  heartbeatIntervalMs?: number;
  children?: React.ReactNode;
  onPhaseChange?: (phase: Phase) => void;
  onError?: (error: string) => void;
  onDownloadProgress?: (progress: number) => void;
  onConnected?: () => void;
  onDisconnected?: () => void;
}

interface OatmealLaunchButtonState {
  phase: Phase;
  progress: number;
  error: string;
}

export class OatmealLaunchButton extends React.Component<OatmealLaunchButtonProps, OatmealLaunchButtonState> {
  static defaultProps = {
    host: "127.0.0.1",
    port: 9607,
    storageKey: "oatmeal_installed",
    pollIntervalMs: 500,
    connectTimeoutMs: 5_000,
    postDownloadTimeoutMs: 120_000,
    heartbeatIntervalMs: 3_000,
  };

  private timerRef?: ReturnType<typeof setInterval>;
  private deadlineRef?: ReturnType<typeof setTimeout>;
  private heartbeatRef?: ReturnType<typeof setInterval>;

  constructor(props: OatmealLaunchButtonProps) {
    super(props);
    this.state = {
      phase: "idle",
      progress: 0,
      error: "",
    };
  }

  componentDidMount() {
    this.startHeartbeatIfConnected();
  }

  componentDidUpdate(_prevProps: OatmealLaunchButtonProps, prevState: OatmealLaunchButtonState) {
    if (prevState.phase !== this.state.phase) {
      this.props.onPhaseChange?.(this.state.phase);
      if (this.state.phase === "connected") {
        this.props.onConnected?.();
        this.startHeartbeatIfConnected();
      } else {
        this.stopHeartbeat();
      }
    }
  }

  componentWillUnmount() {
    this.clearTimers();
    this.stopHeartbeat();
  }

  private startHeartbeatIfConnected = () => {
    if (this.state.phase === "connected") {
      this.heartbeatRef = setInterval(async () => {
        if (!(await this.probeRunning())) {
          this.setState({ phase: "idle" });
          this.props.onDisconnected?.();
        }
      }, this.props.heartbeatIntervalMs ?? OatmealLaunchButton.defaultProps.heartbeatIntervalMs);
    }
  };

  private stopHeartbeat = () => {
    if (this.heartbeatRef) {
      clearInterval(this.heartbeatRef);
      this.heartbeatRef = undefined;
    }
  };

  private probeRunning = async (): Promise<boolean> => {
    try {
      const host = this.props.host ?? OatmealLaunchButton.defaultProps.host;
      const port = this.props.port ?? OatmealLaunchButton.defaultProps.port;
      const healthUrl = `http://${host}:${port}/health`;
      const r = await fetch(healthUrl, {
        method: "HEAD",
        mode: "cors",
        signal: AbortSignal.timeout(800),
      });
      return r.ok;
    } catch {
      return false;
    }
  };

  private updatePhase = (newPhase: Phase) => {
    this.setState({ phase: newPhase });
  };

  private reportError = (errorMsg: string) => {
    this.setState({ error: errorMsg });
    this.props.onError?.(errorMsg);
  };

  private clearTimers = () => {
    if (this.timerRef) clearInterval(this.timerRef);
    if (this.deadlineRef) clearTimeout(this.deadlineRef);
    this.timerRef = undefined;
    this.deadlineRef = undefined;
  };

  private pollFor = (timeoutMs: number, onOk: () => void, onFail: () => void) => {
    this.clearTimers();
    this.deadlineRef = setTimeout(() => {
      this.clearTimers();
      onFail();
    }, timeoutMs);
    this.timerRef = setInterval(async () => {
      if (await this.probeRunning()) {
        this.clearTimers();
        onOk();
      }
    }, this.props.pollIntervalMs ?? OatmealLaunchButton.defaultProps.pollIntervalMs);
  };

  private download = async (url: string) => {
    this.updatePhase("downloading");
    this.setState({ progress: 0 });
    this.props.onDownloadProgress?.(0);
    try {
      const res = await fetch(url);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const total = Number(res.headers.get("content-length") || 0);
      const reader = res.body?.getReader();
      if (!reader) throw new Error("Response body is null");
      const chunks: Uint8Array[] = [];
      let got = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        got += value.length;
        if (total > 0) {
          const pct = Math.round((got / total) * 100);
          this.setState({ progress: pct });
          this.props.onDownloadProgress?.(pct);
        }
      }
      this.setState({ progress: 100 });
      this.props.onDownloadProgress?.(100);
      const blob = new Blob(chunks as BlobPart[], { type: "application/octet-stream" });
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = "oatmeal";
      a.click();
      URL.revokeObjectURL(a.href);
      localStorage.setItem(this.props.storageKey ?? OatmealLaunchButton.defaultProps.storageKey, "1");

      this.updatePhase("run_it");
      this.pollFor(
        this.props.postDownloadTimeoutMs ?? OatmealLaunchButton.defaultProps.postDownloadTimeoutMs,
        () => this.updatePhase("connected"),
        () => {
          const msg = "Timed out waiting for Oatmeal to start. Run the downloaded binary and try again.";
          this.reportError(msg);
          this.updatePhase("error");
        },
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : "Download failed";
      this.reportError(msg);
      this.updatePhase("error");
    }
  };

  private launch = () => {
    this.clearTimers();
    this.setState({ error: "" });
    this.updatePhase("waiting");
    this.pollFor(
      this.props.connectTimeoutMs ?? OatmealLaunchButton.defaultProps.connectTimeoutMs,
      () => this.updatePhase("connected"),
      () => {
        if (localStorage.getItem(this.props.storageKey ?? OatmealLaunchButton.defaultProps.storageKey)) {
          this.updatePhase("run_it");
          this.pollFor(
            this.props.postDownloadTimeoutMs ?? OatmealLaunchButton.defaultProps.postDownloadTimeoutMs,
            () => this.updatePhase("connected"),
            () => {
              const msg = "Timed out waiting for Oatmeal to start. Run the binary and try again.";
              this.reportError(msg);
              this.updatePhase("error");
            },
          );
          return;
        }
        const url = getDownloadUrl(this.props.downloadUrl);
        if (!url) {
          const msg = "Download URL not configured. Set VITE_OATMEAL_DOWNLOAD_URL.";
          this.reportError(msg);
          this.updatePhase("error");
          return;
        }
        void this.download(url);
      },
    );
  };

  private retry = () => {
    this.clearTimers();
    this.updatePhase("idle");
    this.setState({ error: "", progress: 0 });
  };

  render() {
    const { phase, progress, error } = this.state;
    const { children } = this.props;

    if (phase === "idle") {
      const host = this.props.host ?? OatmealLaunchButton.defaultProps.host;
      const port = this.props.port ?? OatmealLaunchButton.defaultProps.port;
      const oatmealUri = `oatmeal://open?host=${encodeURIComponent(host)}&port=${encodeURIComponent(port)}`;
      return (
        <a href={oatmealUri} className="oatmeal-btn" onClick={this.launch}>
          {children ?? "Launch Oatmeal"}
        </a>
      );
    }

    if (phase === "waiting") {
      return (
        <button type="button" className="oatmeal-btn oatmeal-btn--loading" disabled>
          <span className="oatmeal-spinner" />
          Connecting…
        </button>
      );
    }

    if (phase === "downloading") {
      return (
        <button type="button" className="oatmeal-btn oatmeal-btn--loading" disabled>
          <span className="oatmeal-spinner" />
          Downloading {progress}%
        </button>
      );
    }

    if (phase === "run_it") {
      return (
        <button type="button" className="oatmeal-btn oatmeal-btn--loading" disabled>
          <span className="oatmeal-spinner" />
          Waiting for user to start Oatmeal…
        </button>
      );
    }

    if (phase === "connected") {
      return (
        <button type="button" className="oatmeal-btn oatmeal-btn--connected" disabled>
          ✓ Connected
        </button>
      );
    }

    return (
      <button type="button" className="oatmeal-btn oatmeal-btn--error" onClick={this.retry}>
        {error || "Connection failed — try again"}
      </button>
    );
  }
}
