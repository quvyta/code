# A model that answers every harness the same way, on the container's own loopback: its first
# answer runs a shell command, its second writes a file, its third says it is done. The shell
# command and the file are what `permission_live.rs` looks for; a harness that stopped to ask the
# person before either never gets to the next answer.
#
# It speaks the four shapes the harnesses ask in: Anthropic messages, OpenAI chat completions,
# OpenAI responses and Gemini's generateContent, each streamed or not as asked. Which answer is due
# is read from the conversation the harness sends back, by counting the tool results in it, so the
# server keeps no state and a harness that asks twice (a retry, a title, a side question) gets the
# same answer for the same conversation. A request without tools is answered with plain text.
#
# Every request's path and the tool names it offered are written to /tmp/fake-model.log, and every
# request's body, one a line, to /tmp/fake-model.bodies, where a test finds what a harness sent.

import http.server
import json
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 41417
COMMAND = "touch /tmp/outside-the-workspace /work/asked-nothing"
# The file the second answer writes; a second argument names another, such as a source file a
# language server of the harness is to wake up for, which then gets the words as a comment.
FILE = sys.argv[2] if len(sys.argv) > 2 else "/work/edited-without-asking.txt"
TEXT = ("// " if FILE.endswith(".rs") else "") + "written without asking\n"
WRITE_COMMAND = "printf 'written without asking\\n' > " + FILE

SHELL = ["Bash", "bash", "run_shell_command", "Shell", "shell", "exec_command", "shell_command"]
WRITE = ["Write", "write", "write_file", "WriteFile", "create_file"]


def log(line):
    with open("/tmp/fake-model.log", "a") as out:
        out.write(line + "\n")


def fill(schema, kind, command=None):
    """Arguments for a tool whose parameters are `schema`, for a `kind` of 'shell' or 'write'."""
    props = (schema or {}).get("properties") or {}
    args = {}
    for name, prop in props.items():
        low = name.lower()
        typ = prop.get("type")
        if kind == "shell" and low in ("command", "cmd"):
            line = command or COMMAND
            args[name] = ["bash", "-lc", line] if typ == "array" else line
        elif kind == "write" and ("path" in low or low in ("file", "filename")):
            args[name] = FILE
        elif kind == "write" and low in ("content", "contents", "text", "file_text"):
            args[name] = TEXT
    for name in (schema or {}).get("required") or []:
        if name in args:
            continue
        typ = (props.get(name) or {}).get("type")
        if typ == "integer" or typ == "number":
            args[name] = 60000
        elif typ == "boolean":
            args[name] = False
        elif typ == "array":
            args[name] = []
        else:
            args[name] = "the step the test asks for"
    return args


def example(schema):
    """A value of JSON `schema` with every required property present."""
    typ = str(schema.get("type", "object")).lower()
    if typ == "object":
        props = schema.get("properties") or {}
        return {name: example(props.get(name) or {}) for name in schema.get("required") or props}
    if typ == "array":
        return []
    if typ in ("integer", "number"):
        return 10
    if typ == "boolean":
        return False
    if schema.get("enum"):
        return schema["enum"][0]
    return "done"


PATCH = "*** Begin Patch\n*** Add File: " + FILE + "\n+" + TEXT.strip() + "\n*** End Patch\n"


def choose(tools, done):
    """The call due after `done` tool results, from `tools` as (name, schema, custom) triples, or
    None when the answer is text."""
    names = {name: (schema, custom) for name, schema, custom in tools}
    if done == 0:
        for name in SHELL:
            if name in names:
                return name, fill(names[name][0], "shell"), names[name][1]
    if done == 1:
        for name in WRITE:
            if name in names:
                return name, fill(names[name][0], "write"), names[name][1]
        if "apply_patch" in names:
            schema, custom = names["apply_patch"]
            if custom:
                return "apply_patch", PATCH, True
            key = next(iter((schema or {}).get("properties") or {"input": 0}))
            return "apply_patch", {key: PATCH}, False
        # A harness that offers no tool of its own for writing files writes them from its shell.
        for name in SHELL:
            if name in names:
                return name, fill(names[name][0], "shell", WRITE_COMMAND), names[name][1]
    return None


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def send(self, body, content_type="application/json"):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(200)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def sse(self, events):
        body = "".join(
            ("event: " + name + "\n" if name else "") + "data: " + json.dumps(data) + "\n\n" for name, data in events
        )
        self.send(body, "text/event-stream")

    def do_GET(self):
        log("GET " + self.path)
        if "models" in self.path:
            self.send(json.dumps({"object": "list", "data": [{"id": "m", "object": "model"}], "models": []}))
        else:
            self.send("{}")

    def do_HEAD(self):
        self.send_response(200)
        self.send_header("content-length", "0")
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length)
        with open("/tmp/fake-model.bodies", "ab") as out:
            out.write(raw.replace(b"\n", b" ") + b"\n")
        try:
            req = json.loads(raw or b"{}")
        except ValueError:
            req = {}
        path = self.path.split("?")[0]
        if path.endswith("/messages/count_tokens"):
            log("POST " + path)
            return self.send(json.dumps({"input_tokens": 10}))
        if path.endswith("/messages"):
            return self.anthropic(req)
        if path.endswith("/chat/completions"):
            return self.chat(req)
        if path.endswith("/responses"):
            return self.openai_responses(req)
        if ":generateContent" in path or ":streamGenerateContent" in path:
            return self.gemini(req, ":streamGenerateContent" in path)
        if ":countTokens" in path:
            return self.send(json.dumps({"totalTokens": 10}))
        log("POST " + path + " (unknown)")
        self.send("{}")

    # Anthropic messages.
    def anthropic(self, req):
        tools = [(t.get("name"), t.get("input_schema"), False) for t in req.get("tools") or []]
        done = sum(
            1
            for m in req.get("messages") or []
            if isinstance(m.get("content"), list)
            for c in m["content"]
            if c.get("type") == "tool_result"
        )
        call = choose(tools, done) if tools else None
        log("POST /messages tools=%s done=%d call=%s" % ([t[0] for t in tools], done, call and call[0]))
        model = req.get("model", "m")
        usage = {"input_tokens": 10, "output_tokens": 10}
        if call:
            block = {"type": "tool_use", "id": "toolu_%d" % done, "name": call[0], "input": call[1]}
            stop = "tool_use"
        else:
            block = {"type": "text", "text": "done"}
            stop = "end_turn"
        if not req.get("stream"):
            return self.send(json.dumps({
                "id": "msg_%d" % done, "type": "message", "role": "assistant", "model": model,
                "content": [block], "stop_reason": stop, "stop_sequence": None, "usage": usage,
            }))
        start = dict(block, input={}) if call else dict(block, text="")
        delta = (
            {"type": "input_json_delta", "partial_json": json.dumps(call[1])}
            if call
            else {"type": "text_delta", "text": "done"}
        )
        self.sse([
            ("message_start", {"type": "message_start", "message": {
                "id": "msg_%d" % done, "type": "message", "role": "assistant", "model": model, "content": [],
                "stop_reason": None, "stop_sequence": None, "usage": usage}}),
            ("content_block_start", {"type": "content_block_start", "index": 0, "content_block": start}),
            ("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": delta}),
            ("content_block_stop", {"type": "content_block_stop", "index": 0}),
            ("message_delta", {"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": None},
                               "usage": {"output_tokens": 10}}),
            ("message_stop", {"type": "message_stop"}),
        ])

    # OpenAI chat completions.
    def chat(self, req):
        tools = [
            ((t.get("function") or {}).get("name"), (t.get("function") or {}).get("parameters"), False)
            for t in req.get("tools") or []
        ]
        done = sum(1 for m in req.get("messages") or [] if m.get("role") == "tool")
        call = choose(tools, done) if tools else None
        log("POST /chat/completions tools=%s done=%d call=%s" % ([t[0] for t in tools], done, call and call[0]))
        model = req.get("model", "m")
        usage = {"prompt_tokens": 10, "completion_tokens": 10, "total_tokens": 20}
        if call:
            tc = {"index": 0, "id": "call_%d" % done, "type": "function",
                  "function": {"name": call[0], "arguments": json.dumps(call[1])}}
            message = {"role": "assistant", "content": None, "tool_calls": [dict(tc)]}
            message["tool_calls"][0].pop("index")
            finish = "tool_calls"
        else:
            tc = None
            message = {"role": "assistant", "content": "done"}
            finish = "stop"
        if not req.get("stream"):
            return self.send(json.dumps({
                "id": "chatcmpl-%d" % done, "object": "chat.completion", "created": 1, "model": model,
                "choices": [{"index": 0, "message": message, "finish_reason": finish}], "usage": usage,
            }))
        base = {"id": "chatcmpl-%d" % done, "object": "chat.completion.chunk", "created": 1, "model": model}
        first = {"role": "assistant", "content": None, "tool_calls": [tc]} if tc else {"role": "assistant", "content": "done"}
        body = "data: " + json.dumps(dict(base, choices=[{"index": 0, "delta": first, "finish_reason": None}])) + "\n\n"
        body += "data: " + json.dumps(dict(base, choices=[{"index": 0, "delta": {}, "finish_reason": finish}], usage=usage)) + "\n\n"
        body += "data: [DONE]\n\n"
        self.send(body, "text/event-stream")

    # OpenAI responses.
    def openai_responses(self, req):
        tools = []
        for t in req.get("tools") or []:
            if t.get("type") == "function":
                tools.append((t.get("name"), t.get("parameters"), False))
            elif t.get("type") == "custom":
                tools.append((t.get("name"), None, True))
        items = req.get("input") if isinstance(req.get("input"), list) else []
        done = sum(1 for i in items if i.get("type") in ("function_call_output", "custom_tool_call_output"))
        call = choose(tools, done) if tools else None
        log("POST /responses tools=%s done=%d call=%s" % ([t[0] for t in tools], done, call and call[0]))
        if call and call[2]:
            item = {"type": "custom_tool_call", "id": "ctc_%d" % done, "call_id": "call_%d" % done,
                    "name": call[0], "input": call[1], "status": "completed"}
        elif call:
            item = {"type": "function_call", "id": "fc_%d" % done, "call_id": "call_%d" % done,
                    "name": call[0], "arguments": json.dumps(call[1]), "status": "completed"}
        else:
            item = {"type": "message", "id": "msg_%d" % done, "role": "assistant", "status": "completed",
                    "content": [{"type": "output_text", "text": "done", "annotations": []}]}
        response = {"id": "resp_%d" % done, "object": "response", "created_at": 1, "status": "completed",
                    "model": req.get("model", "m"), "output": [item],
                    "usage": {"input_tokens": 10, "output_tokens": 10, "total_tokens": 20,
                              "input_tokens_details": {"cached_tokens": 0},
                              "output_tokens_details": {"reasoning_tokens": 0}}}
        if not req.get("stream"):
            return self.send(json.dumps(response))
        pending = dict(response, status="in_progress", output=[])
        self.sse([
            ("response.created", {"type": "response.created", "sequence_number": 0, "response": pending}),
            ("response.output_item.added", {"type": "response.output_item.added", "sequence_number": 1,
                                            "output_index": 0, "item": item}),
            ("response.output_item.done", {"type": "response.output_item.done", "sequence_number": 2,
                                           "output_index": 0, "item": item}),
            ("response.completed", {"type": "response.completed", "sequence_number": 3, "response": response}),
        ])

    # Gemini generateContent.
    def gemini(self, req, stream):
        tools = [
            (f.get("name"), f.get("parametersJsonSchema") or f.get("parameters"), False)
            for t in req.get("tools") or []
            for f in t.get("functionDeclarations") or []
        ]
        done = sum(1 for c in req.get("contents") or [] for p in c.get("parts") or [] if "functionResponse" in p)
        call = choose(tools, done) if tools else None
        log("POST gemini tools=%s done=%d call=%s" % ([t[0] for t in tools], done, call and call[0]))
        config = req.get("generationConfig") or {}
        if call:
            part = {"functionCall": {"name": call[0], "args": call[1]}}
        elif config.get("responseMimeType") == "application/json":
            # A side question the harness asks for itself (Gemini CLI's router asks how hard the
            # task is before it picks a model), answered in the shape it asked for.
            schema = config.get("responseJsonSchema") or config.get("responseSchema") or {}
            part = {"text": json.dumps(example(schema))}
        else:
            part = {"text": "done"}
        answer = {"candidates": [{"content": {"role": "model", "parts": [part]}, "finishReason": "STOP", "index": 0}],
                  "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 10, "totalTokenCount": 20},
                  "modelVersion": "m"}
        if stream:
            return self.send("data: " + json.dumps(answer) + "\r\n\r\n", "text/event-stream")
        self.send(json.dumps(answer))


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True


Server(("127.0.0.1", PORT), Handler).serve_forever()
