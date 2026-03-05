import { io } from "socket.io-client";

const [, , baseUrl, eventName, payloadJson = "{}", timeoutMs = "1800"] = process.argv;

if (!baseUrl || !eventName) {
  console.error("Usage: node tests/socket_client.mjs <url> <event> [payload-json] [timeout-ms]");
  process.exit(2);
}

const payload = JSON.parse(payloadJson);
const timeout = Number(timeoutMs);
const events = [];

const socket = io(baseUrl, {
  timeout: 5000,
  transports: ["websocket", "polling"],
});

for (const name of ["health", "version", "stdout", "stderr", "exit", "error"]) {
  socket.on(name, (data) => {
    events.push({ event: name, data });
  });
}

await new Promise((resolve, reject) => {
  socket.on("connect", resolve);
  socket.on("connect_error", reject);
});

socket.emit(eventName, payload);
await new Promise((resolve) => setTimeout(resolve, timeout));

socket.disconnect();
console.log(JSON.stringify(events));
