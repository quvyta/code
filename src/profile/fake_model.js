// A model that answers every harness the same way, on the container's own loopback: its first
// answer runs a shell command, its second writes a file, its third says it is done. The shell
// command and the file are what `permission_live.rs` looks for; a harness that stopped to ask the
// person before either never gets to the next answer.
//
// It speaks the four shapes the harnesses ask in: Anthropic messages, OpenAI chat completions,
// OpenAI responses and Gemini's generateContent, each streamed or not as asked. Which answer is due
// is read from the conversation the harness sends back, by counting the tool results in it, so the
// server keeps no state and a harness that asks twice (a retry, a title, a side question) gets the
// same answer for the same conversation. A request without tools is answered with plain text.
//
// Every request's path and the tool names it offered are written to /tmp/fake-model.log, and every
// request's body, one a line, to /tmp/fake-model.bodies, where a test finds what a harness sent.
//
// This is the same model as `fake_model.py`, under Node rather than Python: the base image has
// Node in it and no Python, and the point of these tests is what a harness does under `base`.

const fs = require("node:fs");
const http = require("node:http");

const PORT = Number(process.argv[2] || 41417);
const COMMAND = "touch /tmp/outside-the-workspace /work/asked-nothing";
// The file the second answer writes; a second argument names another, such as a source file a
// language server of the harness is to wake up for, which then gets the words as a comment.
const FILE = process.argv[3] || "/work/edited-without-asking.txt";
const TEXT = (FILE.endsWith(".rs") ? "// " : "") + "written without asking\n";
const WRITE_COMMAND = "printf 'written without asking\\n' > " + FILE;

const SHELL = ["Bash", "bash", "run_shell_command", "Shell", "shell", "exec_command", "shell_command"];
const WRITE = ["Write", "write", "write_file", "WriteFile", "create_file"];
const PATCH = "*** Begin Patch\n*** Add File: " + FILE + "\n+" + TEXT.trim() + "\n*** End Patch\n";

const log = (line) => fs.appendFileSync("/tmp/fake-model.log", line + "\n");

// Arguments for a tool whose parameters are `schema`, for a `kind` of "shell" or "write".
const fill = (schema, kind, command) => {
  const props = (schema || {}).properties || {};
  const args = {};
  for (const [name, prop] of Object.entries(props)) {
    const low = name.toLowerCase();
    const type = prop.type;
    if (kind === "shell" && (low === "command" || low === "cmd")) {
      const line = command || COMMAND;
      args[name] = type === "array" ? ["bash", "-lc", line] : line;
    } else if (kind === "write" && (low.includes("path") || low === "file" || low === "filename")) {
      args[name] = FILE;
    } else if (kind === "write" && ["content", "contents", "text", "file_text"].includes(low)) {
      args[name] = TEXT;
    }
  }
  for (const name of (schema || {}).required || []) {
    if (name in args) continue;
    const type = (props[name] || {}).type;
    if (type === "integer" || type === "number") args[name] = 60000;
    else if (type === "boolean") args[name] = false;
    else if (type === "array") args[name] = [];
    else args[name] = "the step the test asks for";
  }
  return args;
};

// A value of JSON `schema` with every required property present.
const example = (schema) => {
  const type = String(schema.type || "object").toLowerCase();
  if (type === "object") {
    const props = schema.properties || {};
    const wanted = schema.required || Object.keys(props);
    return Object.fromEntries(wanted.map((name) => [name, example(props[name] || {})]));
  }
  if (type === "array") return [];
  if (type === "integer" || type === "number") return 10;
  if (type === "boolean") return false;
  if (schema.enum) return schema.enum[0];
  return "done";
};

// The call due after `done` tool results, from `tools` as [name, schema, custom] triples, or null
// when the answer is text.
const choose = (tools, done) => {
  const names = new Map(tools.map(([name, schema, custom]) => [name, [schema, custom]]));
  const call = (name, args, custom) => [name, args, custom];
  if (done === 0) {
    for (const name of SHELL) {
      if (names.has(name)) return call(name, fill(names.get(name)[0], "shell"), names.get(name)[1]);
    }
  }
  if (done === 1) {
    for (const name of WRITE) {
      if (names.has(name)) return call(name, fill(names.get(name)[0], "write"), names.get(name)[1]);
    }
    if (names.has("apply_patch")) {
      const [schema, custom] = names.get("apply_patch");
      if (custom) return call("apply_patch", PATCH, true);
      const key = Object.keys((schema || {}).properties || { input: 0 })[0];
      return call("apply_patch", { [key]: PATCH }, false);
    }
    // A harness that offers no tool of its own for writing files writes them from its shell.
    for (const name of SHELL) {
      if (names.has(name)) return call(name, fill(names.get(name)[0], "shell", WRITE_COMMAND), names.get(name)[1]);
    }
  }
  return null;
};

const send = (res, body, type) => {
  const data = Buffer.from(typeof body === "string" ? body : JSON.stringify(body));
  res.writeHead(200, { "content-type": type || "application/json", "content-length": data.length });
  res.end(data);
};

const sse = (res, events) => {
  const body = events.map(([name, data]) => (name ? "event: " + name + "\n" : "") + "data: " + JSON.stringify(data) + "\n\n").join("");
  send(res, body, "text/event-stream");
};

const said = (path, tools, done, call) => {
  log("POST " + path + " tools=" + JSON.stringify(tools) + " done=" + done + " call=" + (call ? call[0] : ""));
};

// Anthropic messages.
const anthropic = (res, req) => {
  const tools = (req.tools || []).map((tool) => [tool.name, tool.input_schema, false]);
  const done = (req.messages || [])
    .filter((message) => Array.isArray(message.content))
    .flatMap((message) => message.content)
    .filter((part) => part.type === "tool_result").length;
  const call = tools.length ? choose(tools, done) : null;
  said("/messages", tools.map((tool) => tool[0]), done, call);
  const model = req.model || "m";
  const usage = { input_tokens: 10, output_tokens: 10 };
  const block = call
    ? { type: "tool_use", id: "toolu_" + done, name: call[0], input: call[1] }
    : { type: "text", text: "done" };
  const stop = call ? "tool_use" : "end_turn";
  if (!req.stream) {
    return send(res, {
      id: "msg_" + done, type: "message", role: "assistant", model,
      content: [block], stop_reason: stop, stop_sequence: null, usage,
    });
  }
  const start = call ? { ...block, input: {} } : { ...block, text: "" };
  const delta = call
    ? { type: "input_json_delta", partial_json: JSON.stringify(call[1]) }
    : { type: "text_delta", text: "done" };
  sse(res, [
    ["message_start", { type: "message_start", message: {
      id: "msg_" + done, type: "message", role: "assistant", model, content: [],
      stop_reason: null, stop_sequence: null, usage } }],
    ["content_block_start", { type: "content_block_start", index: 0, content_block: start }],
    ["content_block_delta", { type: "content_block_delta", index: 0, delta }],
    ["content_block_stop", { type: "content_block_stop", index: 0 }],
    ["message_delta", { type: "message_delta", delta: { stop_reason: stop, stop_sequence: null },
                         usage: { output_tokens: 10 } }],
    ["message_stop", { type: "message_stop" }],
  ]);
};

// OpenAI chat completions.
const chat = (res, req) => {
  const tools = (req.tools || []).map((tool) => [(tool.function || {}).name, (tool.function || {}).parameters, false]);
  const done = (req.messages || []).filter((message) => message.role === "tool").length;
  const call = tools.length ? choose(tools, done) : null;
  said("/chat/completions", tools.map((tool) => tool[0]), done, call);
  const model = req.model || "m";
  const usage = { prompt_tokens: 10, completion_tokens: 10, total_tokens: 20 };
  const tc = call ? { id: "call_" + done, type: "function", function: { name: call[0], arguments: JSON.stringify(call[1]) } } : null;
  const message = tc
    ? { role: "assistant", content: null, tool_calls: [tc] }
    : { role: "assistant", content: "done" };
  const finish = tc ? "tool_calls" : "stop";
  if (!req.stream) {
    return send(res, {
      id: "chatcmpl-" + done, object: "chat.completion", created: 1, model,
      choices: [{ index: 0, message, finish_reason: finish }], usage,
    });
  }
  const base = { id: "chatcmpl-" + done, object: "chat.completion.chunk", created: 1, model };
  const first = tc ? { role: "assistant", content: null, tool_calls: [tc] } : { role: "assistant", content: "done" };
  let body = "data: " + JSON.stringify({ ...base, choices: [{ index: 0, delta: first, finish_reason: null }] }) + "\n\n";
  body += "data: " + JSON.stringify({ ...base, choices: [{ index: 0, delta: {}, finish_reason: finish }], usage }) + "\n\n";
  body += "data: [DONE]\n\n";
  send(res, body, "text/event-stream");
};

// OpenAI responses.
const responses = (res, req) => {
  const tools = [];
  for (const tool of req.tools || []) {
    if (tool.type === "function") tools.push([tool.name, tool.parameters, false]);
    else if (tool.type === "custom") tools.push([tool.name, null, true]);
  }
  const items = Array.isArray(req.input) ? req.input : [];
  const done = items.filter((item) => item.type === "function_call_output" || item.type === "custom_tool_call_output").length;
  const call = tools.length ? choose(tools, done) : null;
  said("/responses", tools.map((tool) => tool[0]), done, call);
  let item;
  if (call && call[2]) {
    item = { type: "custom_tool_call", id: "ctc_" + done, call_id: "call_" + done,
             name: call[0], input: call[1], status: "completed" };
  } else if (call) {
    item = { type: "function_call", id: "fc_" + done, call_id: "call_" + done,
             name: call[0], arguments: JSON.stringify(call[1]), status: "completed" };
  } else {
    item = { type: "message", id: "msg_" + done, role: "assistant", status: "completed",
             content: [{ type: "output_text", text: "done", annotations: [] }] };
  }
  const response = { id: "resp_" + done, object: "response", created_at: 1, status: "completed",
    model: req.model || "m", output: [item],
    usage: { input_tokens: 10, output_tokens: 10, total_tokens: 20,
             input_tokens_details: { cached_tokens: 0 },
             output_tokens_details: { reasoning_tokens: 0 } } };
  if (!req.stream) return send(res, response);
  const pending = { ...response, status: "in_progress", output: [] };
  sse(res, [
    ["response.created", { type: "response.created", sequence_number: 0, response: pending }],
    ["response.output_item.added", { type: "response.output_item.added", sequence_number: 1,
                                     output_index: 0, item }],
    ["response.output_item.done", { type: "response.output_item.done", sequence_number: 2,
                                    output_index: 0, item }],
    ["response.completed", { type: "response.completed", sequence_number: 3, response }],
  ]);
};

// Gemini generateContent.
const gemini = (res, req, stream) => {
  const tools = (req.tools || [])
    .flatMap((tool) => tool.functionDeclarations || [])
    .map((fn) => [fn.name, fn.parametersJsonSchema || fn.parameters, false]);
  const done = (req.contents || []).flatMap((content) => content.parts || []).filter((part) => part.functionResponse).length;
  const call = tools.length ? choose(tools, done) : null;
  said("gemini", tools.map((tool) => tool[0]), done, call);
  const config = req.generationConfig || {};
  let part;
  if (call) {
    part = { functionCall: { name: call[0], args: call[1] } };
  } else if (config.responseMimeType === "application/json") {
    // A side question the harness asks for itself (Gemini CLI's router asks how hard the task is
    // before it picks a model), answered in the shape it asked for.
    const schema = config.responseJsonSchema || config.responseSchema || {};
    part = { text: JSON.stringify(example(schema)) };
  } else {
    part = { text: "done" };
  }
  const answer = { candidates: [{ content: { role: "model", parts: [part] }, finishReason: "STOP", index: 0 }],
                   usageMetadata: { promptTokenCount: 10, candidatesTokenCount: 10, totalTokenCount: 20 },
                   modelVersion: "m" };
  if (stream) return send(res, "data: " + JSON.stringify(answer) + "\r\n\r\n", "text/event-stream");
  send(res, answer);
};

http
  .createServer((req, res) => {
    if (req.method === "GET") {
      log("GET " + req.url);
      if (req.url.includes("models")) return send(res, { object: "list", data: [{ id: "m", object: "model" }], models: [] });
      return send(res, {});
    }
    if (req.method === "HEAD") {
      res.writeHead(200, { "content-length": 0 });
      return res.end();
    }
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => {
      const raw = Buffer.concat(chunks);
      fs.appendFileSync("/tmp/fake-model.bodies", raw.toString("utf8").replace(/\n/g, " ") + "\n");
      let body = {};
      try {
        body = JSON.parse(raw.toString("utf8") || "{}");
      } catch (error) {
        body = {};
      }
      const path = req.url.split("?")[0];
      if (path.endsWith("/messages/count_tokens")) {
        log("POST " + path);
        return send(res, { input_tokens: 10 });
      }
      if (path.endsWith("/messages")) return anthropic(res, body);
      if (path.endsWith("/chat/completions")) return chat(res, body);
      if (path.endsWith("/responses")) return responses(res, body);
      if (path.includes(":generateContent") || path.includes(":streamGenerateContent")) {
        return gemini(res, body, path.includes(":streamGenerateContent"));
      }
      if (path.includes(":countTokens")) return send(res, { totalTokens: 10 });
      log("POST " + path + " (unknown)");
      send(res, {});
    });
  })
  .listen(PORT, "127.0.0.1");
