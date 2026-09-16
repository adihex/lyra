//! `lyra-mcp`: stdio JSON-RPC MCP bridge onto the control socket (§2).
//! Thin by design: speaks MCP on stdin/stdout, NDJSON to the socket.
//! Configure agents with `{"command":"lyra-mcp"}` (or `lyra mcp` later).
//!
//! Hand-rolled JSON-RPC loop — no extra deps beyond serde_json.

use lyra_ipc::{Client, ClientError};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

const MCP_VERSION: &str = "2024-11-05";

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
}

fn obj_schema(required: &[&str], props: Value) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

fn tools() -> Vec<Tool> {
    let no_args = obj_schema(&[], json!({}));
    vec![
        Tool { name: "now_playing", description: "Current track, position, player state", schema: no_args.clone() },
        Tool { name: "player_state", description: "Full player snapshot", schema: no_args.clone() },
        Tool { name: "play", description: "Start/resume playback", schema: no_args.clone() },
        Tool { name: "pause", description: "Pause playback", schema: no_args.clone() },
        Tool { name: "next", description: "Next track", schema: no_args.clone() },
        Tool { name: "previous", description: "Previous track", schema: no_args.clone() },
        Tool { name: "stop", description: "Stop playback", schema: no_args.clone() },
        Tool {
            name: "play_track",
            description: "Play a library track by id",
            schema: obj_schema(&["track_id"], json!({"track_id": {"type": "string"}})),
        },
        Tool {
            name: "play_query",
            description: "Search the library and play the top match (composite: search+play)",
            schema: obj_schema(&["q"], json!({"q": {"type": "string"}})),
        },
        Tool {
            name: "search_library",
            description: "FTS library search",
            schema: obj_schema(&["q"], json!({"q": {"type": "string"}, "type": {"type": "string"}, "limit": {"type": "integer"}})),
        },
        Tool {
            name: "seek",
            description: "Absolute seek in seconds (idempotent)",
            schema: obj_schema(&["position_s"], json!({"position_s": {"type": "number"}})),
        },
        Tool {
            name: "set_volume",
            description: "Absolute volume 0.0-1.0 (idempotent)",
            schema: obj_schema(&["volume"], json!({"volume": {"type": "number"}})),
        },
        Tool {
            name: "queue_add",
            description: "Enqueue by track id or search query; supports if_playlist_revision + idempotency_key",
            schema: obj_schema(&[], json!({"track_id": {"type": "string"}, "query": {"type": "string"}, "position": {"type": "integer"}, "if_playlist_revision": {"type": "integer"}, "idempotency_key": {"type": "string"}})),
        },
        Tool { name: "queue_list", description: "List the queue", schema: no_args.clone() },
        Tool {
            name: "queue_remove",
            description: "Remove queue entry by index",
            schema: obj_schema(&["index"], json!({"index": {"type": "integer"}})),
        },
        Tool { name: "queue_clear", description: "Clear the queue", schema: no_args.clone() },
        Tool { name: "eq_get", description: "Current EQ bands + preamp", schema: no_args.clone() },
        Tool {
            name: "eq_set",
            description: "Set EQ: full band array or single {band, gain_db}",
            schema: obj_schema(&[], json!({"bands": {"type": "array"}, "band": {"type": "integer"}, "gain_db": {"type": "number"}, "preamp": {"type": "number"}})),
        },
        Tool { name: "library_stats", description: "Library counts, last scan", schema: no_args.clone() },
        Tool {
            name: "library_scan",
            description: "Start a library rescan job; poll with job_get",
            schema: obj_schema(&[], json!({"path": {"type": "string"}})),
        },
        Tool {
            name: "job_get",
            description: "Poll an async job (library_scan, torrent_add)",
            schema: obj_schema(&["id"], json!({"id": {"type": "string"}})),
        },
        Tool {
            name: "job_cancel",
            description: "Cancel a running job",
            schema: obj_schema(&["id"], json!({"id": {"type": "string"}})),
        },
        Tool { name: "viz_state", description: "Live spectrum bands for ambient tooling", schema: no_args.clone() },
    ]
}

fn submit(c: &mut Client, operation: &str, params: Value) -> Result<Value, ClientError> {
    let r = c.call("operation.submit", json!({"operation": operation, "params": params}))?;
    Ok(json!({"job": r.job, "snapshot": r.snapshot}))
}

fn call_tool(name: &str, args: &Value) -> Result<Value, ClientError> {
    let mut c = Client::discover_and_connect()?;
    let get_str = |k: &str| args.get(k).and_then(|v| v.as_str());
    match name {
        "now_playing" => {
            let snap = c.state()?;
            Ok(json!({
                "track": snap["track"], "state": snap["state"],
                "position": snap["position"], "duration": snap["duration"],
            }))
        }
        "player_state" => c.state(),
        "play" => submit(&mut c, "play", json!({})),
        "pause" => submit(&mut c, "pause", json!({})),
        "next" => submit(&mut c, "next", json!({})),
        "previous" => submit(&mut c, "prev", json!({})),
        "stop" => submit(&mut c, "stop", json!({})),
        "play_track" => {
            let id = get_str("track_id").ok_or_else(|| ClientError::Protocol("play_track needs track_id".into()))?;
            submit(&mut c, "track.play", json!({"track_id": id}))
        }
        // Composite: agents want "play X" to just work.
        "play_query" => {
            let q = get_str("q").ok_or_else(|| ClientError::Protocol("play_query needs q".into()))?;
            let r = submit(&mut c, "library.search", json!({"q": q, "limit": 1}))?;
            let first = r
                .pointer("/job/result/results/0")
                .cloned()
                .unwrap_or(Value::Null);
            let id = first.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
                ClientError::Server {
                    code: lyra_ipc::ErrorCode::NotFound,
                    message: format!("no match for {q}"),
                }
            })?;
            submit(&mut c, "track.play", json!({"track_id": id}))
        }
        "search_library" => {
            let q = get_str("q").ok_or_else(|| ClientError::Protocol("search_library needs q".into()))?;
            let mut p = json!({"q": q});
            if let Some(t) = get_str("type") {
                p["type"] = t.into();
            }
            if let Some(l) = args.get("limit") {
                p["limit"] = l.clone();
            }
            Ok(submit(&mut c, "library.search", p)?.pointer("/job/result").cloned().unwrap_or(Value::Null))
        }
        "seek" => {
            let pos = args.get("position_s").and_then(Value::as_f64).ok_or_else(|| {
                ClientError::Protocol("seek needs position_s".into())
            })?;
            submit(&mut c, "seek.absolute", json!({"position_s": pos}))
        }
        "set_volume" => {
            let v = args.get("volume").and_then(Value::as_f64).ok_or_else(|| {
                ClientError::Protocol("set_volume needs volume".into())
            })?;
            submit(&mut c, "volume.set", json!({"volume": v}))
        }
        "queue_add" => {
            let mut p = json!({});
            for k in ["track_id", "query", "position", "idempotency_key"] {
                if let Some(v) = args.get(k) {
                    p[k] = v.clone();
                }
            }
            let mut req = json!({"operation": "queue.enqueue", "params": p});
            if let Some(rev) = args.get("if_playlist_revision") {
                req["if_playlist_revision"] = rev.clone();
            }
            let r = c.call("operation.submit", req)?;
            Ok(json!({"job": r.job, "snapshot": r.snapshot}))
        }
        "queue_list" => Ok(submit(&mut c, "queue.list", json!({}))?.pointer("/job/result").cloned().unwrap_or(Value::Null)),
        "queue_remove" => {
            let i = args.get("index").and_then(Value::as_i64).ok_or_else(|| {
                ClientError::Protocol("queue_remove needs index".into())
            })?;
            submit(&mut c, "queue.remove", json!({"index": i}))
        }
        "queue_clear" => submit(&mut c, "queue.clear", json!({})),
        "eq_get" => Ok(submit(&mut c, "eq.get", json!({}))?.pointer("/job/result").cloned().unwrap_or(Value::Null)),
        "eq_set" => {
            if let (Some(b), Some(g)) = (args.get("band"), args.get("gain_db")) {
                submit(&mut c, "eq.band.set", json!({"band": b, "gain_db": g}))
            } else {
                let mut p = json!({});
                if let Some(b) = args.get("bands") {
                    p["bands"] = b.clone();
                }
                if let Some(pr) = args.get("preamp") {
                    p["preamp"] = pr.clone();
                }
                submit(&mut c, "eq.set", p)
            }
        }
        "library_stats" => Ok(submit(&mut c, "library.stats", json!({}))?.pointer("/job/result").cloned().unwrap_or(Value::Null)),
        "library_scan" => {
            let mut p = json!({});
            if let Some(path) = get_str("path") {
                p["path"] = path.into();
            }
            submit(&mut c, "library.scan", p)
        }
        "job_get" => {
            let id = get_str("id").ok_or_else(|| ClientError::Protocol("job_get needs id".into()))?;
            let r = c.call("job.get", json!({"id": id}))?;
            Ok(r.job.unwrap_or(Value::Null))
        }
        "job_cancel" => {
            let id = get_str("id").ok_or_else(|| ClientError::Protocol("job_cancel needs id".into()))?;
            let r = c.call("job.cancel", json!({"id": id}))?;
            Ok(r.job.unwrap_or(Value::Null))
        }
        "viz_state" => {
            let r = c.call("spectrum.get", json!({}))?;
            Ok(r.result.unwrap_or(Value::Null))
        }
        other => Err(ClientError::Protocol(format!("unknown tool: {other}"))),
    }
}

fn text_result(v: Value) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}]})
}

fn text_error(msg: String) -> Value {
    json!({"content": [{"type": "text", "text": msg}], "isError": true})
}

fn handle(method: &str, params: &Value, id: &Value) -> Option<Value> {
    let ok = |result: Value| {
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    };
    let err = |code: i64, message: String| {
        Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
    };
    match method {
        "initialize" => ok(json!({
            "protocolVersion": MCP_VERSION,
            "capabilities": {"tools": {}, "resources": {}},
            "serverInfo": {"name": "lyra-mcp", "version": env!("CARGO_PKG_VERSION")},
        })),
        "notifications/initialized" => None,
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tools().iter().map(|t| {
            json!({"name": t.name, "description": t.description, "inputSchema": t.schema})
        }).collect::<Vec<_>>()})),
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if tools().iter().all(|t| t.name != name) {
                return err(-32602, format!("unknown tool: {name}"));
            }
            match call_tool(name, &args) {
                Ok(v) => ok(text_result(v)),
                Err(e) => ok(text_error(e.to_string())),
            }
        }
        "resources/list" => ok(json!({"resources": [{
            "uri": "lyra://state", "name": "player_state",
            "description": "Current player snapshot", "mimeType": "application/json",
        }]})),
        "resources/read" => {
            let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
            if uri != "lyra://state" {
                return err(-32602, format!("unknown resource: {uri}"));
            }
            match Client::discover_and_connect().and_then(|mut c| c.state()) {
                Ok(snap) => ok(json!({"contents": [{
                    "uri": uri, "mimeType": "application/json",
                    "text": serde_json::to_string(&snap).unwrap_or_default(),
                }]})),
                Err(e) => ok(text_error(e.to_string())),
            }
        }
        _ => err(-32601, format!("method not found: {method}")),
    }
}

pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                let _ = writeln!(
                    out,
                    "{}",
                    json!({"jsonrpc": "2.0", "id": Value::Null,
                           "error": {"code": -32700, "message": format!("parse error: {e}")}})
                );
                let _ = out.flush();
                continue;
            }
        };
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        if let Some(resp) = handle(method, &params, &id) {
            let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap_or_default());
            let _ = out.flush();
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_initialize_and_tools_list() {
        let id = json!(1);
        let resp = handle("initialize", &json!({}), &id).unwrap();
        assert_eq!(resp["result"]["protocolVersion"], MCP_VERSION);
        let resp = handle("tools/list", &json!({}), &id).unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for want in ["now_playing", "play", "play_query", "seek", "set_volume", "library_scan", "job_get"] {
            assert!(names.contains(&want), "missing tool {want}");
        }
        // Unknown method → JSON-RPC error, unknown tool → MCP isError (not crash).
        let resp = handle("nope/method", &json!({}), &id).unwrap();
        assert_eq!(resp["error"]["code"], -32601);
        let resp = handle("tools/call", &json!({"name": "nope", "arguments": {}}), &id).unwrap();
        assert_eq!(resp["error"]["code"], -32602);
    }
}
