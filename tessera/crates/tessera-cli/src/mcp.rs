//! Minimal **Model Context Protocol** (MCP) server over stdio — the *agent* driver surface for a
//! `.tsra` (`docs/spikes/tsra-explorer-wireframes.md` § Surfaces). Synchronous, newline-delimited
//! JSON-RPC 2.0 (no async SDK — matches ADR-0034's sync-first API), exposing the read verbs as tools an
//! agent calls, reusing the same view-model / `nav` logic the CLI and TUI render. `tsra mcp` starts it
//! (reads requests on stdin, writes responses on stdout; diagnostics go to stderr via `tracing`).
//!
//! First cut = read-only tools (`tsra_tree` · `tsra_ls` · `tsra_stats` · `tsra_verify`). The protocol
//! surface is `initialize` → `tools/list` → `tools/call`; notifications get no response. Compile-checked
//! here — smoke-test against a real MCP client before relying on it.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{json, Value};
use tessera_core::{Error, Result};
use tessera_io::Reader;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the stdio MCP server loop until EOF on stdin.
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| Error::Invalid(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue; // ignore malformed frames
        };
        // A request has an `id`; a notification does not and gets no response.
        let Some(id) = req.get("id").cloned() else {
            continue;
        };
        let method = req
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let response = match dispatch(method, params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(msg) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": msg } })
            }
        };
        writeln!(stdout, "{response}").map_err(|e| Error::Invalid(e.to_string()))?;
        stdout.flush().ok();
    }
    Ok(())
}

/// Dispatch one JSON-RPC method to its result (or a human error string).
fn dispatch(method: &str, params: Value) -> std::result::Result<Value, String> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "tessera", "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_specs() })),
        "tools/call" => call_tool(&params),
        other => Err(format!("unknown method '{other}'")),
    }
}

/// The tool catalogue (JSON-Schema per tool) returned by `tools/list`.
fn tool_specs() -> Value {
    let file = || json!({ "type": "string", "description": "path to the .tsra file" });
    json!([
        {
            "name": "tsra_tree",
            "description": "Render a .tsra as a navigable hierarchy tree (product/schema/blocks/sources).",
            "inputSchema": { "type": "object", "properties": { "file": file() }, "required": ["file"] }
        },
        {
            "name": "tsra_ls",
            "description": "List one node's children: no path = top level; 'meta'; '<block>'; or 'sources'.",
            "inputSchema": { "type": "object",
                "properties": { "file": file(), "path": { "type": "string" } }, "required": ["file"] }
        },
        {
            "name": "tsra_stats",
            "description": "Numeric overview of an array block: shape/dtype/chunks/codec/min/max/mean/std.",
            "inputSchema": { "type": "object",
                "properties": { "file": file(), "block": { "type": "string" } }, "required": ["file", "block"] }
        },
        {
            "name": "tsra_verify",
            "description": "Verify a .tsra's integrity: magic + manifest seal + every block digest.",
            "inputSchema": { "type": "object", "properties": { "file": file() }, "required": ["file"] }
        }
    ])
}

/// Run one `tools/call`, returning the MCP `content` payload (a single text block).
fn call_tool(params: &Value) -> std::result::Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("missing tool name")?;
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let text = run_tool(name, &args).map_err(|e| e.to_string())?;
    Ok(json!({ "content": [ { "type": "text", "text": text } ] }))
}

/// Execute a tool by name — the read verbs reused from `crate::nav`, rendered to a text buffer.
fn run_tool(name: &str, args: &Value) -> Result<String> {
    let arg = |k: &str| args.get(k).and_then(Value::as_str);
    let file = arg("file").ok_or_else(|| Error::Invalid("missing 'file' argument".into()))?;
    let file = Path::new(file);

    if name == "tsra_verify" {
        let mut r = Reader::open(file)?;
        let n = r.manifest().blocks.len();
        for block in r.block_names() {
            r.read_block(&block)?; // payload bytes vs recorded digest
        }
        return Ok(format!("OK  {} verified ({n} blocks)", file.display()));
    }

    let mut buf: Vec<u8> = Vec::new();
    match name {
        "tsra_tree" => crate::nav::tree(file, false, &mut buf)?,
        "tsra_ls" => crate::nav::ls(file, arg("path"), false, &mut buf)?,
        "tsra_stats" => {
            let block =
                arg("block").ok_or_else(|| Error::Invalid("missing 'block' argument".into()))?;
            crate::nav::stats(file, block, &mut buf)?;
        }
        other => return Err(Error::Invalid(format!("unknown tool '{other}'"))),
    }
    String::from_utf8(buf).map_err(|e| Error::Invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_advertises_the_tools_capability() {
        let r = dispatch("initialize", Value::Null).unwrap();
        assert_eq!(r["protocolVersion"], PROTOCOL_VERSION);
        assert!(r["capabilities"]["tools"].is_object());
        assert_eq!(r["serverInfo"]["name"], "tessera");
    }

    #[test]
    fn tools_list_returns_the_read_tools_with_object_schemas() {
        let r = dispatch("tools/list", Value::Null).unwrap();
        let tools = r["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"tsra_tree") && names.contains(&"tsra_verify"));
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn unknown_method_and_missing_file_are_errors() {
        assert!(dispatch("nope", Value::Null).is_err());
        // `tools/call` with no 'file' argument surfaces a clear error, not a panic.
        assert!(dispatch(
            "tools/call",
            json!({ "name": "tsra_tree", "arguments": {} })
        )
        .is_err());
    }
}
