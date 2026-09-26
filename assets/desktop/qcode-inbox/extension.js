// QCode's inbox for the agent of this window. A window has no prompt QCode could type a message
// into, so the messages other tabs send it wait in QCode until the agent takes them with the
// `check_inbox` tool. This extension is what makes the agent do so: every few seconds it asks QCode,
// through the workspace's socket, how many messages wait for this tab, and when there are more than
// it last said, it sends the agent panel one prompt telling the agent to take them. The messages
// themselves stay in QCode until the agent takes them, so a prompt that reaches no agent (nobody
// signed in yet) loses nothing.
//
// The tab's token is the one QCode wrote into this window's MCP settings for the bridge's server,
// which it writes anew for every tab it opens, so it is read again on every look.

const fs = require("fs");
const net = require("net");
const os = require("os");
const path = require("path");
const vscode = require("vscode");

const SOCKET = "/run/qcode-mcp/bridge.sock";
const SETTINGS = path.join(os.homedir(), ".gemini", "config", "mcp_config.json");
const LOCK = path.join(os.tmpdir(), "qcode-inbox.lock");
const EVERY_MS = 3000;
const WAIT_MS = 10000;
const SEND = "antigravity.sendPromptToAgentPanel";
// How long messages may go on waiting after the agent was told before it is told again. The panel
// may have been closed when the prompt came (it is opened by the same command, and a prompt sent
// before it is listening is lost), or the agent may have finished its turn without looking.
const AGAIN_MS = Number(process.env.QCODE_INBOX_AGAIN_MS) || 120000;

// The token of this window's tab, or "" when QCode has written none.
function token() {
  try {
    const settings = JSON.parse(fs.readFileSync(SETTINGS, "utf8"));
    const value = settings?.mcpServers?.qcode?.env?.QCODE_BRIDGE;
    return typeof value === "string" ? value : "";
  } catch {
    return "";
  }
}

// One question to QCode, answered with its reply or null.
function ask(request) {
  return new Promise((resolve) => {
    const socket = net.createConnection(SOCKET);
    let received = "";
    let settled = false;
    const finish = (answer) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      resolve(answer);
    };
    socket.setTimeout(WAIT_MS, () => finish(null));
    socket.on("connect", () => socket.write(JSON.stringify(request) + "\n"));
    socket.on("data", (chunk) => {
      received += chunk.toString("utf8");
      const end = received.indexOf("\n");
      if (end < 0) return;
      try {
        finish(JSON.parse(received.slice(0, end)));
      } catch {
        finish(null);
      }
    });
    socket.on("error", () => finish(null));
    socket.on("close", () => finish(null));
  });
}

// Whether this extension host is the one that looks. A window can start more than one host, and
// each would otherwise send the same prompt; the first to hold the lock looks, and a lock whose
// holder is gone is taken over.
function holdsLock() {
  try {
    fs.writeFileSync(LOCK, String(process.pid), { flag: "wx" });
    return true;
  } catch {
    try {
      const holder = Number(fs.readFileSync(LOCK, "utf8"));
      if (holder === process.pid) return true;
      if (holder > 0 && fs.existsSync(`/proc/${holder}`)) return false;
      fs.writeFileSync(LOCK, String(process.pid));
      return true;
    } catch {
      return false;
    }
  }
}

function prompt(count, senders) {
  const what = count === 1 ? "A message" : `${count} messages`;
  return (
    `Through QCode: ${what} from ${count === 1 ? "another tab" : "other tabs"} of this workspace (${senders}) ` +
    `${count === 1 ? "waits" : "wait"} for you. Call the check_inbox tool of the qcode MCP server, ` +
    "do what the messages ask, and answer each one with send_message to the tab that sent it."
  );
}

exports.activate = function (context) {
  let told = 0;
  let toldAt = 0;
  let busy = false;
  const look = async () => {
    if (busy || !holdsLock()) return;
    busy = true;
    try {
      const own = token();
      if (!own) return;
      const answer = await ask({ token: own, op: "peek" });
      if (!answer || !answer.ok || !Array.isArray(answer.messages)) return;
      const count = answer.messages.length;
      if (count > told || (count > 0 && Date.now() - toldAt >= AGAIN_MS)) {
        const senders = [...new Set(answer.messages.map((message) => message.from))].join(", ");
        await vscode.commands.executeCommand(SEND, prompt(count, senders));
        toldAt = Date.now();
      }
      told = count;
    } catch {
      // A look that fails is tried again at the next; nothing is taken from QCode by looking.
    } finally {
      busy = false;
    }
  };
  const timer = setInterval(look, EVERY_MS);
  context.subscriptions.push({ dispose: () => clearInterval(timer) });
};

exports.deactivate = function () {};
