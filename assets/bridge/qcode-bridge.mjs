// The QCode bridge: a Model Context Protocol server that lets the agent of one QCode tab learn
// which tab it is, list the other tabs of its workspace, send a message to one or all of them,
// and take the messages waiting for its own tab.
//
// A harness starts this file with `node` over stdio. It has no dependencies, because it runs in
// every profile image and nothing but Node is promised there. It decides nothing itself: every
// question goes to QCode on the host, through the workspace's socket in the folder this file lives
// in, and QCode answers with the words the agent is shown. That keeps the rules (the person's
// approval, the network direction, the loop limits) in one place, where the person is.
//
// The wire format is newline-delimited JSON-RPC 2.0, as the stdio transport of MCP defines it.
// Both eras of the protocol are served: the `initialize` handshake of the revisions up to
// 2025-11-25, and the per-request metadata and `server/discover` of 2026-07-28.
//
// The same tools reach opencode's shared server as a plugin (`qcode-opencode-plugin.mjs`), which
// imports what it needs from here; this file only serves stdio when it is the program started.

import { createConnection } from "node:net";
import { readFileSync, realpathSync } from "node:fs";
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

export const INSTRUCTIONS =
  "Other tabs of this QCode workspace run agents too. list_tabs says which tab you are (`you`, " +
  "and its first line) and names the others; only list_tabs tells you your own id. send_message " +
  "hands one of them, or `all` of them, a message of a kind: `info` (no answer needed, the " +
  "default), `question` (you wait for the answer) or `report` (a task: you wait for a report of " +
  "the result). A message arrives under two lines naming its sender's tab id, its kind and how to " +
  "answer; answer a question, or report a task's result, with send_message to that id and kind " +
  "`info`. QCode may refuse a message, or wait for the person to allow it; the answer always says " +
  "what happened and why. A message is written into the other tab's prompt once that tab is quiet, " +
  "so it may still be waiting after send_message answers: list_tabs says how many messages each " +
  "tab still holds and what is stopping them. A window tab has no prompt: messages to it wait " +
  "until its agent calls check_inbox, and an agent in a window calls check_inbox at the start and " +
  "the end of every task.";

export const TOOLS = [
  {
    name: "list_tabs",
    title: "Say which tab you are and list the other agent tabs",
    description:
      "Says which tab you are (`you`: your id, title, coding tool, profile and workspace; the " +
      "first line of the text says it too) and lists the other tabs of this QCode workspace that " +
      "run an agent: the id to send to, the tab's title, its coding tool, its profile, whether it " +
      "reaches the network, how many messages are still waiting to be written into it (`waiting`) " +
      "and what is stopping them (`trouble`, null when nothing is). Ids count within this " +
      "workspace. Call it again to see whether a message you sent has arrived.",
    inputSchema: { type: "object", additionalProperties: false },
  },
  {
    name: "send_message",
    title: "Send a message to another agent tab",
    description:
      "Sends a message to the agent of another tab of this QCode workspace, by the id list_tabs " +
      "gave, or to every other agent tab with `all`. The message is written into that tab's " +
      "prompt under two lines naming this tab as its sender, the kind and how to answer, once the " +
      "tab is quiet: nobody typing in it and its tool done writing. The answer says whether the " +
      "message went in, is still waiting to go in, is waiting for the person's approval, or was " +
      "refused, and why; for `all`, tab by tab. Do not take a message that is still waiting for " +
      "delivered work; list_tabs says whether it has arrived.",
    inputSchema: {
      type: "object",
      properties: {
        tab: { type: "string", description: "The id of the tab, as list_tabs gives it, or `all` for every other agent tab." },
        text: { type: "string", description: "The message, written for the agent that reads it." },
        kind: {
          type: "string",
          enum: ["info", "question", "report"],
          description:
            "What you ask of the receiver: `info` for their information, no answer needed (the " +
            "default); `question` when you wait for their answer; `report` for a task whose result " +
            "they report when they finish. An answer to a question, and a report, are `info`.",
        },
      },
      required: ["tab", "text"],
      additionalProperties: false,
    },
  },
  {
    name: "check_inbox",
    title: "Take the messages waiting for this tab",
    description:
      "Returns every message other tabs of this QCode workspace sent to this tab that has not " +
      "reached you yet, each with the id of the tab that sent it and its kind, and takes them out of QCode: they are " +
      "yours now. A tab in a window (Antigravity IDE) has no prompt, so this is the only way its " +
      "messages reach it: call it at the start of every task and when you finish one. Answer a " +
      "message with send_message to the tab that sent it.",
    inputSchema: { type: "object", additionalProperties: false },
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

// Asks QCode one question and answers its one-line reply. `extra` is what a question carries
// beside the token: the conversation, when the tab is told apart by it (opencode's shared
// server speaks for every tab of its profile with one token).
function ask(request, extra = {}) {
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
    socket.on("connect", () => socket.write(JSON.stringify({ token: TOKEN, ...extra, ...request }) + "\n"));
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
  "QCode did not answer: it is not running, or this workspace is not open in it. Nothing was sent.";

export async function call(params, extra = {}) {
  const name = params?.name;
  const args = params?.arguments ?? {};
  if (name === "list_tabs") {
    const answer = await ask({ op: "list" }, extra);
    if (!answer) return text(UNREACHABLE, true);
    const result = text(answer.text, !answer.ok);
    if (answer.ok) result.structuredContent = { you: answer.you ?? null, tabs: answer.tabs ?? [] };
    return result;
  }
  if (name === "send_message") {
    if (typeof args.tab !== "string" || typeof args.text !== "string") {
      return text("send_message needs `tab` and `text`, both strings.", true);
    }
    const kind = args.kind ?? "info";
    if (!["info", "question", "report"].includes(kind)) {
      return text("`kind` is one of info, question and report. Nothing was sent.", true);
    }
    const answer = await ask({ op: "send", tab: args.tab, text: args.text, kind }, extra);
    if (!answer) return text(UNREACHABLE, true);
    const result = text(answer.text, !answer.ok);
    if (answer.sent) result.structuredContent = { sent: answer.sent };
    return result;
  }
  if (name === "check_inbox") {
    const answer = await ask({ op: "inbox" }, extra);
    if (!answer) return text(UNREACHABLE, true);
    const result = text(answer.text, !answer.ok);
    if (answer.ok) result.structuredContent = { messages: answer.messages ?? [] };
    return result;
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

// Whether this file is the program that was started, rather than a module another one imported.
function started() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (started()) serve();

function serve() {
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
}
