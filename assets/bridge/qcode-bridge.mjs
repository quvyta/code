// The QCode bridge: a Model Context Protocol server that lets the agent of one QCode tab
// list the other tabs of its project and send a message to one of them.
//
// A harness starts this file with `node` over stdio. It has no dependencies, because it runs in
// every profile image and nothing but Node is promised there. It decides nothing itself: every
// question goes to QCode on the host, through the project's socket in the folder this file lives
// in, and QCode answers with the words the agent is shown. That keeps the rules (the person's
// approval, the network direction, the loop limits) in one place, where the person is.
//
// The wire format is newline-delimited JSON-RPC 2.0, as the stdio transport of MCP defines it.
// Both eras of the protocol are served: the `initialize` handshake of the revisions up to
// 2025-11-25, and the per-request metadata and `server/discover` of 2026-07-28.

import { createConnection } from "node:net";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";

const SOCKET = join(dirname(fileURLToPath(import.meta.url)), "bridge.sock");
const SERVER = { name: "qcode", version: "1" };
const MODERN = "2026-07-28";
const LEGACY = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const VERSION_KEY = "io.modelcontextprotocol/protocolVersion";
// Longer than any approval QCode waits for before it answers on its own.
const HOST_WAIT_MS = 120000;

const INSTRUCTIONS =
  "Other tabs of this QCode project run agents too. list_tabs names them; send_message " +
  "hands one of them a message. The person using QCode approves the first message between two " +
  "tabs, and QCode may refuse a message; the answer always says what happened and why.";

const TOOLS = [
  {
    name: "list_tabs",
    title: "List the project's other agent tabs",
    description:
      "Lists the other tabs open in this QCode project that run an agent: the id to send " +
      "to, the tab's title, its harness, its profile and whether it reaches the network.",
    inputSchema: { type: "object", additionalProperties: false },
  },
  {
    name: "send_message",
    title: "Send a message to another agent tab",
    description:
      "Sends a message to the agent of another tab of this QCode project, by the id " +
      "list_tabs gave. The receiving agent sees who sent it. The answer says whether the message " +
      "was accepted, is waiting for the person's approval, or was refused, and why.",
    inputSchema: {
      type: "object",
      properties: {
        tab: { type: "string", description: "The id of the tab, as list_tabs gives it." },
        text: { type: "string", description: "The message, written for the agent that reads it." },
      },
      required: ["tab", "text"],
      additionalProperties: false,
    },
  },
];

// The tab this server speaks for. QCode starts every harness tab with the variable set, and a
// harness passes its environment on to the servers it starts; one that passes only some of it
// still has the variable in its own process, so the parents are asked in turn.
function token() {
  if (process.env.QCODE_BRIDGE) return process.env.QCODE_BRIDGE;
  let pid = process.ppid;
  for (let depth = 0; pid > 1 && depth < 16; depth += 1) {
    try {
      const environment = readFileSync(`/proc/${pid}/environ`, "latin1").split("\0");
      const found = environment.find((entry) => entry.startsWith("QCODE_BRIDGE="));
      if (found) return found.slice("QCODE_BRIDGE=".length);
      const stat = readFileSync(`/proc/${pid}/stat`, "latin1");
      pid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
    } catch {
      return "";
    }
  }
  return "";
}

const TOKEN = token();

// Asks QCode one question and answers its one-line reply.
function ask(request) {
  return new Promise((resolve) => {
    const socket = createConnection(SOCKET);
    let received = "";
    let settled = false;
    const finish = (answer) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      resolve(answer);
    };
    socket.setTimeout(HOST_WAIT_MS, () => finish(null));
    socket.on("connect", () => socket.write(JSON.stringify({ token: TOKEN, ...request }) + "\n"));
    socket.on("data", (chunk) => {
      received += chunk.toString("utf8");
      const end = received.indexOf("\n");
      if (end >= 0) {
        try {
          finish(JSON.parse(received.slice(0, end)));
        } catch {
          finish(null);
        }
      }
    });
    socket.on("error", () => finish(null));
    socket.on("close", () => finish(null));
  });
}

function text(value, isError) {
  return { content: [{ type: "text", text: value }], isError };
}

const UNREACHABLE =
  "QCode did not answer: it is not running, or this project is not open in it. Nothing was sent.";

async function call(params) {
  const name = params?.name;
  const args = params?.arguments ?? {};
  if (name === "list_tabs") {
    const answer = await ask({ op: "list" });
    if (!answer) return text(UNREACHABLE, true);
    const result = text(answer.text, !answer.ok);
    if (answer.ok) result.structuredContent = { tabs: answer.tabs ?? [] };
    return result;
  }
  if (name === "send_message") {
    if (typeof args.tab !== "string" || typeof args.text !== "string") {
      return text("send_message needs `tab` and `text`, both strings.", true);
    }
    const answer = await ask({ op: "send", tab: args.tab, text: args.text });
    if (!answer) return text(UNREACHABLE, true);
    return text(answer.text, !answer.ok);
  }
  return null;
}

function reply(id, result) {
  return { jsonrpc: "2.0", id, result };
}

function failure(id, code, message, data) {
  return { jsonrpc: "2.0", id, error: data === undefined ? { code, message } : { code, message, data } };
}

// Answers one message, or nothing for a notification.
async function handle(message) {
  if (message === null || typeof message !== "object" || Array.isArray(message)) {
    return failure(null, -32600, "Invalid Request");
  }
  const { id, method, params } = message;
  const isRequest = id !== undefined && id !== null;
  if (typeof method !== "string") return isRequest ? failure(id, -32600, "Invalid Request") : null;
  if (!isRequest) return null;

  // A request that carries its version is of the modern era, and is answered in it.
  const version = params?._meta?.[VERSION_KEY];
  const modern = version !== undefined;
  if (modern && version !== MODERN) {
    return failure(id, -32022, "Unsupported protocol version", { supported: [MODERN], requested: version });
  }
  const complete = (result) => reply(id, modern ? { resultType: "complete", ...result } : result);

  switch (method) {
    case "initialize": {
      const asked = params?.protocolVersion;
      return reply(id, {
        protocolVersion: LEGACY.includes(asked) ? asked : LEGACY[0],
        capabilities: { tools: {} },
        serverInfo: SERVER,
        instructions: INSTRUCTIONS,
      });
    }
    case "server/discover":
      return complete({
        supportedVersions: [MODERN],
        capabilities: { tools: {} },
        _meta: { "io.modelcontextprotocol/serverInfo": SERVER },
        instructions: INSTRUCTIONS,
      });
    case "ping":
      return complete({});
    case "tools/list":
      return complete({ tools: TOOLS });
    case "tools/call": {
      const result = await call(params);
      if (result === null) return failure(id, -32602, `Unknown tool: ${params?.name}`);
      return complete(result);
    }
    default:
      return failure(id, -32601, `Method not found: ${method}`);
  }
}

function send(message) {
  if (message !== null) process.stdout.write(JSON.stringify(message) + "\n");
}

// Questions still waiting for QCode when the harness closes the input are answered before the
// server leaves, since the harness may still be reading.
let pending = 0;
let closed = false;
const leaveWhenDone = () => {
  if (closed && pending === 0) process.exit(0);
};

async function receive(line) {
  let parsed;
  try {
    parsed = JSON.parse(line);
  } catch {
    send(failure(null, -32700, "Parse error"));
    return;
  }
  if (Array.isArray(parsed)) {
    const answers = (await Promise.all(parsed.map(handle))).filter((answer) => answer !== null);
    if (answers.length > 0) send(answers);
    return;
  }
  send(await handle(parsed));
}

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", async (line) => {
  if (line.trim() === "") return;
  pending += 1;
  try {
    await receive(line);
  } finally {
    pending -= 1;
    leaveWhenDone();
  }
});
lines.on("close", () => {
  closed = true;
  leaveWhenDone();
});
