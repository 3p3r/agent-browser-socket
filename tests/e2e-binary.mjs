#!/usr/bin/env node
/**
 * End-to-end smoke tests for the final release binary.
 *
 * Usage:
 *   node tests/e2e-binary.mjs <path-to-binary>
 *
 * Validates:
 *   1. <binary> --version                       exits 0, stdout contains "oatmeal"
 *   2. <binary> --verbose --command agent-browser --version   forwards to embedded binary
 */

import { execFile } from "node:child_process";
import { resolve } from "node:path";
import { access, constants } from "node:fs/promises";

const binary = process.argv[2];

if (!binary) {
  console.error("Usage: node tests/e2e-binary.mjs <path-to-binary>");
  process.exit(2);
}

const binaryPath = resolve(binary);

try {
  await access(binaryPath, constants.X_OK);
} catch {
  console.error(`Binary not found or not executable: ${binaryPath}`);
  process.exit(2);
}

let passed = 0;
let failed = 0;

function run(args) {
  return new Promise((resolve) => {
    execFile(binaryPath, args, { timeout: 30_000 }, (error, stdout, stderr) => {
      resolve({ code: error ? (error.code ?? 1) : 0, stdout, stderr });
    });
  });
}

async function test(name, args, checks) {
  const label = `${name} (${[binary, ...args].join(" ")})`;
  try {
    const result = await run(args);
    checks(result);
    console.log(`  PASS ${label}`);
    passed++;
  } catch (err) {
    console.error(`  FAIL ${label}`);
    console.error(`    ${err.message}`);
    failed++;
  }
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

console.log(`\nTesting binary: ${binaryPath}\n`);

await test("--version prints wrapper version", ["--version"], ({ code, stdout }) => {
  assert(code === 0, `expected exit code 0, got ${code}`);
  assert(stdout.toLowerCase().includes("oatmeal"), `stdout should contain "oatmeal", got: ${stdout.trim()}`);
});

await test(
  "--verbose --command agent-browser --version forwards to embedded binary",
  ["--verbose", "--command", "agent-browser", "--version"],
  ({ code, stdout }) => {
    assert(code === 0, `expected exit code 0, got ${code}`);
    assert(stdout.includes("agent-browser"), `stdout should contain "agent-browser", got: ${stdout.trim()}`);
  },
);

console.log(`\nResults: ${passed} passed, ${failed} failed\n`);
process.exit(failed > 0 ? 1 : 0);
