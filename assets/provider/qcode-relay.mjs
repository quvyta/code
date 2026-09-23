// The QCode provider relay: a loopback HTTP server that lets a harness inside this container
// speak to a provider it has been pointed at, without ever holding the provider's key.
//
// A harness is given this server's address (127.0.0.1 and the port below) the way it would be
// given the provider's own. Every request is carried, unchanged but for its own `authorization`,
// `x-api-key` and `api-key` headers, to QCode on the host over the socket beside this file. QCode decides
// whether the request may go anywhere at all, adds the real key to the copy it sends the
// provider, and streams the answer back here. The key never reaches this process, this container,
// or anything either of them could write down.
//
// No dependency but Node itself, because this file runs in every profile image and nothing more
// is promised there. It has no logic about providers: it does not know what a provider is, what
// shape it speaks, or whether the request it is carrying will be allowed. All of that is QCode's,
// on the other end of the socket, where the person is.

import { spawn } from "node:child_process";
import { createConnection } from "node:net";
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SOCKET = join(dirname(fileURLToPath(import.meta.url)), "relay.sock");
// Must match `provider::relay::PORT`.
const PORT = 41417;
// Must match `bridge::TOKEN_VARIABLE`: a tab has one identity, and the bridge already gives it
// one nobody outside this process can guess, so the relay reuses it rather than minting a second.
const TOKEN_VARIABLE = "QCODE_BRIDGE";
// Headers a harness's own request carries that must never reach QCode as if they were meant for
// it: the key that matters here is QCode's own, added once the request reaches the host.
const STRIPPED = new Set(["authorization", "x-api-key", "api-key"]);

// The tab this server speaks for, found the same way the bridge finds it: its own environment
// first, then its parents' in turn, because a harness that passes on only part of its environment
// still carries the variable in its own process.
function token() {
  if (process.env[TOKEN_VARIABLE]) return process.env[TOKEN_VARIABLE];
  let pid = process.ppid;
  for (let depth = 0; pid > 1 && depth < 16; depth += 1) {
    try {
      const environment = readFileSync(`/proc/${pid}/environ`, "latin1").split("\0");
      const found = environment.find((entry) => entry.startsWith(`${TOKEN_VARIABLE}=`));
      if (found) return found.slice(TOKEN_VARIABLE.length + 1);
      const stat = readFileSync(`/proc/${pid}/stat`, "latin1");
      pid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
    } catch {
      return "";
    }
  }
  return "";
}

const TOKEN = token();

// The headers a request is carried with: whatever the harness sent, minus the ones it must never
// carry across, lower-cased the way Node already gives them.
function carriedHeaders(headers) {
  const carried = {};
  for (const [name, value] of Object.entries(headers)) {
    if (STRIPPED.has(name.toLowerCase()) || value === undefined) continue;
    carried[name] = Array.isArray(value) ? value.join(", ") : value;
  }
  return carried;
}

const server = createServer((req, res) => {
  const socket = createConnection(SOCKET);
  let settled = false;
  let headBuffer = Buffer.alloc(0);
  let headParsed = false;

  const fail = (status, message) => {
    if (settled) return;
    settled = true;
    socket.destroy();
    if (!res.headersSent) res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: message }));
  };

  socket.on("connect", () => {
    const head = JSON.stringify({ token: TOKEN, method: req.method, path: req.url, headers: carriedHeaders(req.headers) });
    socket.write(`${head}\n`);
    req.pipe(socket);
  });

  socket.on("data", (chunk) => {
    if (headParsed) {
      res.write(chunk);
      return;
    }
    headBuffer = Buffer.concat([headBuffer, chunk]);
    const end = headBuffer.indexOf("\n");
    if (end < 0) return;
    headParsed = true;
    let parsed;
    try {
      parsed = JSON.parse(headBuffer.subarray(0, end).toString("utf8"));
    } catch {
      fail(502, "QCode's answer was not understood.");
      return;
    }
    settled = true;
    res.writeHead(parsed.status ?? 502, parsed.headers ?? {});
    const rest = headBuffer.subarray(end + 1);
    if (rest.length > 0) res.write(rest);
  });

  socket.on("end", () => {
    if (settled) res.end();
    else fail(502, "QCode closed the connection without answering.");
  });
  socket.on("close", () => {
    if (settled) res.end();
    else fail(502, "QCode did not answer: it is not running, or this project is not open in it.");
  });
  socket.on("error", () => fail(502, "QCode did not answer: it is not running, or this project is not open in it."));
  req.on("error", () => socket.destroy());
});

// The harness itself, when this process was given one to run. Starting it here rather than
// beside it is the only way to know the relay is listening before the harness's first request:
// a harness started in parallel can ask before the server is up and take the refusal for a
// provider that does not work. The relay then lives exactly as long as the harness does.
const harness = process.argv.slice(2);

server.listen(PORT, "127.0.0.1", () => {
  if (harness.length === 0) return;
  const child = spawn(harness[0], harness.slice(1), { stdio: "inherit" });
  child.on("exit", (code, signal) => {
    server.close();
    process.exit(signal ? 128 : (code ?? 0));
  });
  child.on("error", (error) => {
    process.stderr.write(`${harness[0]}: ${error.message}\n`);
    server.close();
    process.exit(127);
  });
});
