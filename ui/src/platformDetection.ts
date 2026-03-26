import { UAParser } from "ua-parser-js";

export interface PlatformInfo {
  osName: string | null;
  displayName: string | null;
  isMobile: boolean;
  binaryName: string | null;
  downloadUrl: string | null;
  buttonLabel: string;
}

const LINUX_VARIANTS = new Set([
  "Linux",
  "Ubuntu",
  "Debian",
  "Fedora",
  "Red Hat",
  "Arch",
  "CentOS",
  "Mint",
  "SUSE",
  "Gentoo",
  "Mandriva",
  "PCLinuxOS",
  "Slackware",
]);

export function detectPlatform(releaseBaseUrl: string): PlatformInfo {
  const parser = new UAParser();
  const result = parser.getResult();
  const osName = result.os.name ?? null;

  const isMobile =
    result.device.type === "mobile" || result.device.type === "tablet" || osName === "iOS" || osName === "Android";

  if (isMobile) {
    return { osName, displayName: null, isMobile: true, binaryName: null, downloadUrl: null, buttonLabel: "" };
  }

  let binaryName: string | null = null;
  let displayName: string | null = null;

  if (osName === "Windows") {
    binaryName = "oatmeal-windows.exe";
    displayName = "Windows";
  } else if (osName === "Mac OS") {
    binaryName = "oatmeal-mac";
    displayName = "macOS";
  } else if (osName && LINUX_VARIANTS.has(osName)) {
    binaryName = "oatmeal-linux";
    displayName = "Linux";
  }

  const downloadUrl = binaryName ? `${releaseBaseUrl}/${binaryName}` : null;
  const buttonLabel = displayName ? `Download for ${displayName}` : "Download";

  return { osName, displayName, isMobile: false, binaryName, downloadUrl, buttonLabel };
}

export function getReleasesPageUrl(releaseBaseUrl: string): string {
  const idx = releaseBaseUrl.indexOf("/releases");
  if (idx >= 0) return releaseBaseUrl.substring(0, idx + "/releases".length);
  return releaseBaseUrl;
}

export function defaultReleaseBaseUrl(): string {
  return "https://github.com/3p3r/oatmeal/releases/download/rc5";
}
