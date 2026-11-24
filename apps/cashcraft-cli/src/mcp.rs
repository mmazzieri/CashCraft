//! Server MCP (Model Context Protocol) su stdin/stdout: JSON-RPC 2.0,
//! un messaggio per riga. Espone le stesse operazioni della CLI come "tools".
//!
//! Esempio di configurazione per un client MCP:
//! { "mcpServers": { "cashcraft": { "command": "cashcraft-cli", "args": ["mcp", "--automation-read-root", "…", "--automation-write-root", "…"] } } }

use anyhow::Result;
use cashcraft_engine::db::Db;
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
                let err = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("JSON non valido: {e}")}});
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

/// Gestisce un messaggio; le notifiche (senza id) non hanno risposta.
pub fn handle(db: &Db, msg: &Value, roots: &ops::Roots) -> Option<Value> {
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
                "instructions": "CashCraft gestisce le finanze personali: conti, movimenti, categorie, budget, regole e import dei CSV delle banche italiane. Gli importi sono in euro; negativo = uscita. Conti e categorie si indicano per nome o id."
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
                Err((-32602, format!("tool \"{name}\" sconosciuto")))
            } else {
                Ok(match ops::call(db, name, &args, Some(roots)) {
                    Ok(v) => json!({
                        "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                        "structuredContent": v,
                        "isError": false
                    }),
                    Err(e) => json!({
                        "content": [{"type": "text", "text": format!("Errore: {e:#}")}],
                        "isError": true
                    }),
                })
            }
        }
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        other => Err((-32601, format!("metodo \"{other}\" non supportato"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    })
}
