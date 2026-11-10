"""A scripted stand-in for the OpenAI Responses API (no model, no network).

Used by tools/e2e/codex-allowance-probe.ps1 to drive the REAL `codex exec`
binary through its own tool pipeline (exec policy, sandbox, approvals) with
fixed shell commands, without sending any prompt to a real model.

    python -I codex-mock-responses.py <port> <commands.json> <log.jsonl>

Each POST .../responses answers with one function call running the next
command of <commands.json> (a JSON array of strings) through whichever shell
tool the request offers, then a final assistant message once the list is
used up. Every request's tool names and the tool outputs Codex sends back
are appended to <log.jsonl>.
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
COMMANDS = json.load(open(sys.argv[2], encoding="utf-8-sig"))
LOG = sys.argv[3]
STATE = {"next": 0, "n": 0}


def log(entry):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")


def tool_names(body):
    out = []
    for t in body.get("tools") or []:
        out.append(t.get("name") or t.get("type"))
    return out


def call_for(tools, entry):
    """(tool name, JSON arguments) running `entry` with the offered shell
    tool. An entry is a command string, or {"cmd": ..., "workdir": ...} to
    run it in another directory (the tool's own `workdir` argument)."""
    cmd = entry["cmd"] if isinstance(entry, dict) else entry
    wd = entry.get("workdir") if isinstance(entry, dict) else None
    extra = {"workdir": wd} if wd else {}
    if "shell_command" in tools:
        return "shell_command", {"command": cmd, **extra}
    if "exec_command" in tools:
        return "exec_command", {"cmd": cmd, "yield_time_ms": 20000, **extra}
    if "shell" in tools:
        return "shell", {"command": ["powershell.exe", "-NoProfile", "-Command", cmd], **extra}
    raise RuntimeError(f"no shell tool in {tools}")


def sse(events):
    return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        body = json.dumps({"models": [], "data": []}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(n)
        try:
            body = json.loads(raw)
        except Exception:
            body = {}
        if not self.path.rstrip("/").endswith("/responses"):
            self.send_response(404)
            self.end_headers()
            return
        tools = tool_names(body)
        outputs = [
            {"call_id": i.get("call_id"), "output": i.get("output")}
            for i in body.get("input") or []
            if isinstance(i, dict) and i.get("type", "").endswith("call_output")
        ]
        STATE["n"] += 1
        rid = f"resp_{STATE['n']}"
        log({"request": STATE["n"], "path": self.path, "tools": tools, "outputs": outputs[-1:]})
        events = [{"type": "response.created", "response": {"id": rid}}]
        if STATE["next"] < len(COMMANDS):
            cmd = COMMANDS[STATE["next"]]
            STATE["next"] += 1
            name, args = call_for(tools, cmd)
            log({"request": STATE["n"], "call": name, "command": cmd})
            events.append(
                {
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": {
                        "type": "function_call",
                        "id": f"fc_{STATE['n']}",
                        "call_id": f"call_{STATE['n']}",
                        "name": name,
                        "arguments": json.dumps(args),
                    },
                }
            )
        else:
            events.append(
                {
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": {
                        "type": "message",
                        "role": "assistant",
                        "id": f"msg_{STATE['n']}",
                        "content": [{"type": "output_text", "text": "probe finished"}],
                    },
                }
            )
        events.append(
            {
                "type": "response.completed",
                "response": {
                    "id": rid,
                    "usage": {
                        "input_tokens": 0,
                        "input_tokens_details": None,
                        "output_tokens": 0,
                        "output_tokens_details": None,
                        "total_tokens": 0,
                    },
                },
            }
        )
        data = sse(events)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
