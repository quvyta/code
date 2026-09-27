// opencode's shared server in a QCode profile container, and what keeps it.
//
// opencode's own program is a server and an interface in one process, and one of them per tab
// costs most of a gigabyte. The server part is the same for every tab of a profile, so the
// container runs it once, and each tab runs only the interface, `opencode attach`, in a small
// shell loop (`ui::workspace::shared::tab_program`). This script is the rest:
//
//   node qcode-opencode.mjs ready [<conversation>]
//     Starts the keeper and the server unless they run already, waits until the server answers,
//     and prints the conversation a tab is to show: the one given, while the server still has it,
//     or a new one. QCode runs it before a tab starts, so it knows the tab's conversation from the
//     first moment: that is how the tab is told apart from its neighbours when its agent asks
//     QCode something, and what it opens again next time.
//
//   node qcode-opencode.mjs ensure
//     The same without a conversation, which each tab runs once before its interface starts:
//     after the last tab of a profile has ended the server stops, and a tab started in that
//     moment must not attach to nothing.
//
//   node qcode-opencode.mjs keep
//     The keeper, started by the two above and left running, one per container. It starts the
//     server and watches it and the tabs:
//       - a server that stops is started again, and every interface is ended with a note that
//         tells its tab's loop to attach again to the same conversation: an interface whose
//         server went away takes what is typed and does nothing with it;
//       - a tab that ended, closed by QCode or quit by the person, has what its agent was doing
//         stopped (a server goes on with a conversation whose interface left) and its
//         conversation removed when nothing was said in it;
//       - when no tab has been attached for a while, the server is stopped and the keeper ends.
//
// One keeper and no process per tab of its own: an idle Node costs about 40 MB, and seven of
// them would eat a good part of what sharing the server saves.
//
// What the server starts with comes from the environment QCode gives: its token in QCODE_SERVER
// (a tab's own token, QCODE_BRIDGE, never reaches the server, since QCode ends a closed tab's
// processes by it), the relay's script in QCODE_RELAY when the profile runs on a provider, and
// opencode's configuration in OPENCODE_CONFIG_CONTENT. A keeper found running with another token
// is one an earlier QCode started, whose tabs are gone; it is stopped with its server.
//
// No dependencies, like the bridge's server beside it: nothing but Node is promised in an image.

import { spawn } from "node:child_process";
import { closeSync, mkdirSync, openSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";

// QCode hands both over (`ui::workspace::shared`); these are the same values, for a run without
// them.
const PORT = Number(process.env.QCODE_OPENCODE_PORT || 41418);
const DIR = process.env.QCODE_OPENCODE_DIR || "/work";
const ADDRESS = `http://127.0.0.1:${PORT}`;
// Where the keeper keeps its notes and the server's log, one folder per port. The tab's loop
// reads the notes too, so it knows the folder by the same rule.
const NOTES = process.env.QCODE_OPENCODE_NOTES || `/tmp/qcode-opencode-${PORT}`;
const LOG = `${NOTES}/server.log`;

// Generous, because the first start of a server reads every plugin; finite, because a tab
// waiting forever is a tab stuck.
const START_WAIT_MS = 120000;
// How often the keeper looks, and how many misses in a row mean the server is gone.
const WATCH_MS = 2000;
const MISSES = 2;
// How long the keeper waits for an interface it ended to come back before it takes the tab for
// closed, and how long it keeps a server no tab is attached to.
const RETURN_MS = 30000;
const IDLE_MS = Number(process.env.QCODE_OPENCODE_IDLE_MS || 30000);
// How long a question to the server may take before it counts as unanswered, and how long the
// first conversation may take: a server without the network waits over a minute for its plugins'
// packages before it makes one (opencode 1.18.32, measured).
const ASK_MS = 5000;
const CONVERSATION_WAIT_MS = 300000;

const TOKEN = process.env.QCODE_SERVER || "";
// Marks the keeper and the server by port, so they can be found and told from those of another.
const MARK = "QCODE_SERVES";
const ROLE = "QCODE_OPENCODE_ROLE";

function fail(words) {
  process.stderr.write(`qcode-opencode: ${words}\n`);
  process.exit(1);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const safe = (id) => typeof id === "string" && /^[A-Za-z0-9][A-Za-z0-9_-]*$/.test(id);

// Asks the server. `null` when it does not answer within `wait` or answers with an error.
async function ask(method, path, body, wait = ASK_MS) {
  try {
    const answer = await fetch(`${ADDRESS}${path}`, {
      method,
      headers: { "x-opencode-directory": DIR, "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(wait),
    });
    if (!answer.ok) return null;
    const text = await answer.text();
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  } catch {
    return null;
  }
}

async function healthy() {
  const answer = await ask("GET", "/global/health");
  return answer !== null && typeof answer === "object" && answer.healthy === true;
}

// The processes of this container, other than this one, with their environment and command line.
function processes() {
  const found = [];
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry) || Number(entry) === process.pid) continue;
    try {
      const environment = readFileSync(`/proc/${entry}/environ`, "latin1").split("\0");
      const words = readFileSync(`/proc/${entry}/cmdline`, "latin1").split("\0").filter(Boolean);
      const value = (name) => {
        const item = environment.find((each) => each.startsWith(`${name}=`));
        return item === undefined ? undefined : item.slice(name.length + 1);
      };
      found.push({ pid: Number(entry), value, words });
    } catch {
      // A process that ended while it was being read, or one of somebody else.
    }
  }
  return found;
}

// The keepers of this port.
function keepers() {
  return processes().filter((each) => each.value(MARK) === String(PORT) && each.value(ROLE) === "keeper");
}

// The interfaces attached to this port's server, with the conversation each shows.
function interfaces() {
  return processes()
    .filter((each) => each.words.includes("attach") && each.words.includes(ADDRESS))
    .map((each) => ({ pid: each.pid, session: each.words[each.words.indexOf("--session") + 1] }))
    .filter((each) => safe(each.session));
}

// The process group `pid` leads, or the process alone.
function group(pid) {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, "latin1");
    return Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[2]);
  } catch {
    return 0;
  }
}

function signal(pid, name) {
  const leader = group(pid);
  try {
    if (leader > 1 && leader !== process.pid) process.kill(-leader, name);
    else process.kill(pid, name);
  } catch {
    // Gone already.
  }
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    const stat = readFileSync(`/proc/${pid}/stat`, "latin1");
    return stat.slice(stat.lastIndexOf(")") + 2)[0] !== "Z";
  } catch {
    return false;
  }
}

// Stops the keepers of this port for which `which` holds, with their servers.
async function stopKeepers(which) {
  const doomed = keepers().filter(which);
  for (const keeper of doomed) signal(keeper.pid, "SIGTERM");
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline && doomed.some((keeper) => alive(keeper.pid))) await sleep(100);
  for (const keeper of doomed) if (alive(keeper.pid)) signal(keeper.pid, "SIGKILL");
}

function startKeeper() {
  mkdirSync(NOTES, { recursive: true });
  const environment = { ...process.env, [MARK]: String(PORT), [ROLE]: "keeper" };
  delete environment.QCODE_BRIDGE;
  const log = openSync(LOG, "a");
  const keeper = spawn(process.execPath, [new URL(import.meta.url).pathname, "keep"], {
    cwd: DIR,
    env: environment,
    detached: true,
    stdio: ["ignore", log, log],
  });
  keeper.on("error", () => {});
  keeper.unref();
  closeSync(log);
}

// A tab on its way: noted while `ready` or `ensure` runs, so the keeper does not take the moment
// before the tab's interface is attached, which can be long ([`CONVERSATION_WAIT_MS`]), for a
// server nobody wants.
const WANTED = `${NOTES}/wanted`;

function wanted() {
  try {
    mkdirSync(NOTES, { recursive: true });
    writeFileSync(WANTED, String(Date.now()));
  } catch {
    // The keeper then goes by the interfaces alone.
  }
}

function lastWanted() {
  try {
    return statSync(WANTED).mtimeMs;
  } catch {
    return 0;
  }
}

// The keeper of this token, and its server answering.
async function ensure() {
  if (TOKEN === "") fail("QCODE_SERVER is not set: QCode starts this for an opencode tab.");
  wanted();
  setInterval(wanted, WATCH_MS).unref();
  await stopKeepers((keeper) => keeper.value("QCODE_SERVER") !== TOKEN);
  const deadline = Date.now() + START_WAIT_MS;
  while (Date.now() < deadline) {
    // Two tabs starting at once may both start a keeper; the second finds the first and ends.
    if (keepers().length === 0) startKeeper();
    if (await healthy()) return;
    await sleep(250);
  }
  let log = "";
  try {
    log = readFileSync(LOG, "utf8").split("\n").slice(-20).join("\n");
  } catch {
    // No log: the keeper never started.
  }
  fail(`opencode's server did not answer within ${START_WAIT_MS / 1000} seconds.\n${log}`);
}

// The conversation a tab is to show: `wanted` while the server has it, a new one otherwise.
async function ready(wanted) {
  await ensure();
  if (safe(wanted)) {
    const found = await ask("GET", `/session/${wanted}`, undefined, CONVERSATION_WAIT_MS);
    if (found && typeof found === "object" && found.id === wanted) {
      process.stdout.write(`${wanted}\n`);
      return;
    }
  }
  const made = await ask("POST", "/session", {}, CONVERSATION_WAIT_MS);
  if (!made || typeof made !== "object" || !safe(made.id)) fail("opencode's server made no conversation.");
  process.stdout.write(`${made.id}\n`);
}

// The keeper.
async function keep() {
  // Another keeper of this token started in the same moment keeps; this one ends.
  if (keepers().some((keeper) => keeper.pid < process.pid)) process.exit(0);
  let server = null;
  const startServer = () => {
    const environment = { ...process.env, QCODE_BRIDGE: TOKEN };
    delete environment.QCODE_SERVER;
    delete environment[ROLE];
    const command = ["opencode", "serve", "--port", String(PORT), "--hostname", "127.0.0.1"];
    if (process.env.QCODE_RELAY) command.unshift("node", process.env.QCODE_RELAY);
    server = spawn(command[0], command.slice(1), { cwd: DIR, env: environment, stdio: "inherit" });
    server.on("error", () => {});
  };
  const stopServer = () => {
    // The server and whatever it started (its MCP servers, the relay's harness) are this
    // keeper's process group.
    try {
      process.kill(-process.pid, "SIGTERM");
    } catch {
      // Nothing left.
    }
  };
  for (const name of ["SIGTERM", "SIGHUP", "SIGINT"]) {
    process.on(name, () => {
      stopServer();
      process.exit(0);
    });
  }

  startServer();
  // The conversations of the interfaces seen last time, and those ended here and expected back.
  let seen = new Set();
  const expected = new Map();
  const leaving = new Set();
  let misses = 0;
  let answered = false;
  let started = Date.now();
  let lonely = Date.now();
  for (;;) {
    await sleep(WATCH_MS);
    const up = await healthy();
    misses = up ? 0 : misses + 1;
    answered = answered || up;
    // A server still starting is given its while; one that ended, or stopped answering after it
    // had answered, is started again and every interface sent back to its conversation.
    const exited = server.exitCode !== null || server.signalCode !== null;
    const silent = misses >= MISSES && (answered || Date.now() - started > START_WAIT_MS);
    if (exited || silent) {
      if (!exited) {
        try {
          server.kill("SIGKILL");
        } catch {
          // Gone.
        }
      }
      startServer();
      started = Date.now();
      misses = 0;
      answered = false;
      const until = Date.now() + START_WAIT_MS;
      while (Date.now() < until && !(await healthy())) await sleep(250);
      mkdirSync(NOTES, { recursive: true });
      for (const each of interfaces()) {
        writeFileSync(`${NOTES}/again-${each.session}`, "");
        expected.set(each.session, Date.now());
        // The interface alone: its group is its tab's, whose loop attaches it again.
        try {
          process.kill(each.pid, "SIGTERM");
        } catch {
          // Gone already.
        }
      }
      continue;
    }
    const present = new Set(interfaces().map((each) => each.session));
    for (const session of present) expected.delete(session);
    for (const session of seen) {
      if (!present.has(session) && !expected.has(session)) leaving.add(session);
    }
    for (const [session, since] of expected) {
      if (Date.now() - since > RETURN_MS) {
        expected.delete(session);
        rmSync(`${NOTES}/again-${session}`, { force: true });
        leaving.add(session);
      }
    }
    // A server still loading its plugins answers nothing yet; what a tab left is seen to once it
    // does.
    for (const session of leaving) {
      if (present.has(session) || (await ended(session))) leaving.delete(session);
    }
    seen = present;
    if (present.size > 0 || expected.size > 0 || leaving.size > 0) {
      lonely = Date.now();
    } else if (Date.now() - Math.max(lonely, lastWanted()) > IDLE_MS) {
      stopServer();
      process.exit(0);
    }
  }
}

// What a tab leaves behind when it ends: its agent's work stopped, and its conversation removed
// when nothing was said in it. False while the server does not answer.
async function ended(session) {
  const status = await ask("GET", "/session/status");
  if (status === null || typeof status !== "object") return false;
  if (status[session] && status[session].type !== "idle") await ask("POST", `/session/${session}/abort`);
  const messages = await ask("GET", `/session/${session}/message`);
  if (Array.isArray(messages) && messages.length === 0) await ask("DELETE", `/session/${session}`);
  return true;
}

const [what, argument] = process.argv.slice(2);
if (what === "ready") await ready(argument);
else if (what === "ensure") await ensure();
else if (what === "keep") await keep();
else fail("use `ready [<conversation>]`, `ensure` or `keep`.");
