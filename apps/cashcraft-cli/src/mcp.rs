//! MCP (Model Context Protocol) server on stdin/stdout: JSON-RPC 2.0, one message per line. Its
//! tools are the operations of the command line. Always in English.
//!
//! Example of configuration for an MCP client:
//! { "mcpServers": { "cashcraft": { "command": "cashcraft-cli", "args": ["mcp", "--automation-read-root", "…", "--automation-write-root", "…"] } } }

use anyhow::Result;
use cashcraft_engine::db::Db;
use cashcraft_engine::i18n::{Lang, with_lang};
use cashcraft_engine::ops;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

const PROTOCOL: &str = "2025-06-18";

pub fn serve(db: &Db, roots: &ops::Roots) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("invalid JSON: {e}")}});
                writeln!(out, "{err}")?;
                out.flush()?;
                continue;
            }
        };
        let replies: Vec<Value> = match msg {
            Value::Array(batch) => batch.iter().filter_map(|m| handle(db, m, roots)).collect(),
            m => handle(db, &m, roots).into_iter().collect(),
        };
        for r in replies {
            writeln!(out, "{r}")?;
        }
        out.flush()?;
    }
    Ok(())
}

/// Handles a message; notifications (without an id) get no reply.
pub fn handle(db: &Db, msg: &Value, roots: &ops::Roots) -> Option<Value> {
    with_lang(Lang::En, || reply(db, msg, roots))
}

fn reply(db: &Db, msg: &Value, roots: &ops::Roots) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let id = id?;
    let result: std::result::Result<Value, (i64, String)> = match method {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or(PROTOCOL).to_string();
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "cashcraft", "title": "CashCraft", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "CashCraft manages personal finances: accounts, transactions, categories, budgets, rules and import of Italian bank files, of OFX statements and of credit card statements in PDF. Amounts are in euros; negative = money out. Accounts and categories are given by name or id."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => {
            let tools: Vec<Value> = ops::tools()
                .into_iter()
                .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": t.schema}))
                .collect();
            Ok(json!({"tools": tools}))
        }
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !ops::tools().iter().any(|t| t.name == name) {
                Err((-32602, format!("unknown tool \"{name}\"")))
            } else {
                Ok(match ops::call(db, name, &args, Some(roots)) {
                    Ok(v) => json!({
                        "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                        "structuredContent": v,
                        "isError": false
                    }),
                    Err(e) => json!({
                        "content": [{"type": "text", "text": format!("Error: {e:#}")}],
                        "isError": true
                    }),
                })
            }
        }
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        other => Err((-32601, format!("method \"{other}\" is not supported"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_get_english_in_every_language() {
        let dir = std::env::temp_dir();
        let roots = ops::Roots::new(&dir, &dir).unwrap();
        let ask = |l, method: &str, params: Value| {
            with_lang(l, || {
                let db = with_lang(Lang::It, Db::open_in_memory).unwrap();
                let msg = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
                handle(&db, &msg, &roots).unwrap()
            })
        };
        let calls = [
            ("initialize", json!({})),
            ("tools/list", json!({})),
            ("nonsense", json!({})),
            ("tools/call", json!({"name": "nonsense"})),
            ("tools/call", json!({"name": "find", "arguments": {}})),
            ("tools/call", json!({"name": "delete_account", "arguments": {"account": "Nessuno"}})),
            ("tools/call", json!({"name": "create_account", "arguments": {"name": "A", "opening_balance": "1,234"}})),
            ("tools/call", json!({"name": "create_account", "arguments": {"name": "A", "opening_balance": "x"}})),
        ];
        for (method, params) in calls {
            assert_eq!(ask(Lang::It, method, params.clone()), ask(Lang::En, method, params));
        }
        let error = ask(Lang::It, "tools/call", json!({"name": "delete_account", "arguments": {"account": "x"}}));
        assert_eq!(error["result"]["content"][0]["text"], "Error: account \"x\" not found");
    }
}
