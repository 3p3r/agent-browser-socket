/// <reference types="vite/client" />
import React from "react";
import "./OatmealLaunchButton.css";
import { defaultReleaseBaseUrl, detectPlatform, getReleasesPageUrl } from "./platformDetection";
import type { PlatformInfo } from "./platformDetection";

function getReleaseBaseUrl(override?: string): string {
  if (override) return override;
  return (import.meta.env.VITE_OATMEAL_DOWNLOAD_URL as string | undefined) ?? defaultReleaseBaseUrl();
}

export type Phase = "idle" | "waiting" | "run_it" | "connected" | "error";

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

  private launch = () => {
    this.clearTimers();
    this.setState({ error: "" });
    this.updatePhase("waiting");
    localStorage.setItem(this.props.storageKey ?? OatmealLaunchButton.defaultProps.storageKey, "1");
    this.pollFor(
      this.props.connectTimeoutMs ?? OatmealLaunchButton.defaultProps.connectTimeoutMs,
      () => this.updatePhase("connected"),
      () => {
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
      },
    );
  };

  private retry = () => {
    this.clearTimers();
    this.updatePhase("idle");
    this.setState({ error: "" });
  };

  render() {
    const { phase, error } = this.state;
    const { children } = this.props;
    const releaseBaseUrl = getReleaseBaseUrl(this.props.downloadUrl);

    if (phase === "idle") {
      const platform: PlatformInfo = detectPlatform(releaseBaseUrl);
      const releasesPageUrl = getReleasesPageUrl(releaseBaseUrl);

      if (platform.isMobile) {
        return (
          <span className="oatmeal-notice">
            <strong>Oatmeal is a desktop application</strong>
            <a href={releasesPageUrl} target="_blank" rel="noopener noreferrer">
              View releases on GitHub
            </a>
          </span>
        );
      }

      const href = platform.downloadUrl ?? releasesPageUrl;
      const label = children ?? platform.buttonLabel;
      const handleClick = platform.downloadUrl
        ? (e: React.MouseEvent) => {
            e.preventDefault();
            window.open(href, "_blank", "noopener,noreferrer");
            this.launch();
          }
        : undefined;

      return (
        <React.Fragment>
          <a href={href} className="oatmeal-btn" onClick={handleClick}>
            {label}
          </a>
          <a href={releasesPageUrl} className="oatmeal-downloads-link" target="_blank" rel="noopener noreferrer">
            View all downloads
          </a>
        </React.Fragment>
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
