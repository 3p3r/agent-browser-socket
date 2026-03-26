import type { Meta, StoryObj } from "@storybook/react";
import { OatmealLaunchButton } from "./OatmealLaunchButton";
import { defaultReleaseBaseUrl } from "./platformDetection";

const meta: Meta<typeof OatmealLaunchButton> = {
  title: "Oatmeal/LaunchButton",
  component: OatmealLaunchButton,
  parameters: {
    layout: "centered",
    docs: {
      description: {
        component:
          "Launch flow is protocol-first: the button attempts `oatmeal://open?host=...&port=...` and falls back to the platform download URL when the protocol handler is unavailable.",
      },
    },
  },
  tags: ["autodocs"],
};

export default meta;

type Story = StoryObj<typeof OatmealLaunchButton>;

export const Default: Story = {
  args: {
    downloadUrl: defaultReleaseBaseUrl(),
  },
};

export const CustomLabel: Story = {
  args: {
    children: "Connect to Oatmeal",
    downloadUrl: defaultReleaseBaseUrl(),
  },
};

export const CustomDownloadUrl: Story = {
  name: "Custom Download URL",
  args: {
    downloadUrl: defaultReleaseBaseUrl(),
  },
};

export const CustomHostPort: Story = {
  name: "Custom Host and Port",
  args: {
    host: "127.0.0.1",
    port: 9911,
    downloadUrl: defaultReleaseBaseUrl(),
  },
};

export const WithPlatformDetection: Story = {
  name: "Platform Detection (Auto)",
  args: {
    downloadUrl: defaultReleaseBaseUrl(),
  },
};

export const ProtocolFirstLaunch: Story = {
  name: "Protocol-First Launch",
  args: {
    host: "127.0.0.1",
    port: 9607,
    downloadUrl: defaultReleaseBaseUrl(),
    children: "Open Oatmeal",
  },
  parameters: {
    docs: {
      description: {
        story:
          "Click attempts the `oatmeal://` protocol first. If unavailable or unsupported, it opens the download URL in a new tab.",
      },
    },
  },
};

export const ChildrenOverride: Story = {
  name: "Children Override Label",
  args: {
    children: "Get Oatmeal Now",
    downloadUrl: defaultReleaseBaseUrl(),
  },
};
