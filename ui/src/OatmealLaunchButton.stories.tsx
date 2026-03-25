import type { Meta, StoryObj } from "@storybook/react";
import { OatmealLaunchButton } from "./OatmealLaunchButton";

const meta: Meta<typeof OatmealLaunchButton> = {
  title: "Oatmeal/LaunchButton",
  component: OatmealLaunchButton,
  parameters: { layout: "centered" },
  tags: ["autodocs"],
};

export default meta;

type Story = StoryObj<typeof OatmealLaunchButton>;

export const Default: Story = {
  args: { children: "Launch Oatmeal" },
};

export const CustomLabel: Story = {
  args: { children: "Connect to Oatmeal" },
};

export const CustomDownloadUrl: Story = {
  name: "Custom Download URL",
  args: {
    children: "Launch Oatmeal",
    downloadUrl: "https://example.com/oatmeal",
  },
};

export const CustomHostPort: Story = {
  name: "Custom Host and Port",
  args: {
    children: "Launch Oatmeal",
    host: "127.0.0.1",
    port: 9911,
  },
};
