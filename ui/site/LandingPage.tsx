import React from "react";
import { OatmealLaunchButton } from "../src/OatmealLaunchButton";
import "./LandingPage.css";

const FEATURES = [
  { emoji: "\u{1F50C}", label: "MCP Server", desc: "Connects to any AI agent that speaks Model Context Protocol" },
  {
    emoji: "\u{1F310}",
    label: "Browser Automation",
    desc: "Control a real browser \u2014 click, type, screenshot, extract",
  },
  { emoji: "\u26A1", label: "Shell & Python", desc: "Execute Bash scripts and Python code in a sandboxed runtime" },
  { emoji: "\u{1F4BB}", label: "Cross-Platform", desc: "Single binary for Windows, macOS, and Linux" },
  {
    emoji: "\u{1F5A5}\uFE0F",
    label: "System Tray",
    desc: "Runs quietly in your tray \u2014 always ready, never in the way",
  },
];

export class LandingPage extends React.Component {
  render() {
    return (
      <div className="oatmeal-landing">
        <div className="oatmeal-container">
          <img className="oatmeal-logo" src="/oatmeal/logo.png" alt="Oatmeal" />
          <h1 className="oatmeal-title">Oatmeal</h1>
          <p className="oatmeal-tagline">
            Extend your AI agent with browser automation, shell execution, and more — one binary, zero setup.
          </p>

          <div className="oatmeal-download">
            <OatmealLaunchButton />
          </div>

          <div className="oatmeal-features">
            {FEATURES.map((f) => (
              <div className="oatmeal-feature" key={f.label}>
                <span className="oatmeal-feature-icon" role="img" aria-label={f.label}>
                  {f.emoji}
                </span>
                <span>
                  <strong className="oatmeal-feature-label">{f.label}</strong>
                  {" — "}
                  <span className="oatmeal-feature-desc">{f.desc}</span>
                </span>
              </div>
            ))}
          </div>

          <footer className="oatmeal-footer">
            <a href="https://github.com/3p3r/oatmeal">GitHub</a>
            <span className="oatmeal-footer-sep">&middot;</span>
            <a href="https://github.com/3p3r/oatmeal/releases">Releases</a>
          </footer>
        </div>
      </div>
    );
  }
}
