import type { Meta, StoryObj } from "@storybook/react";
import { OatmealLaunchButton } from "./OatmealLaunchButton";
import { defaultReleaseBaseUrl } from "./platformDetection";

const meta: Meta<typeof OatmealLaunchButton> = {
  title: "Oatmeal/LaunchButton",
  component: OatmealLaunchButton,
  parameters: { layout: "centered" },
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

export const ChildrenOverride: Story = {
  name: "Children Override Label",
  args: {
    children: "Get Oatmeal Now",
    downloadUrl: defaultReleaseBaseUrl(),
  },
};
