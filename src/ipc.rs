//! Read-only agent access. The unlocked app answers requests on a private Unix
//! socket; the `journal` command and the MCP server are thin clients of it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use chrono::{Local, NaiveDate};
use serde_json::{json, Value};

use crate::vault::{day_markdown, group_by_day, Entry, Vault};

#[derive(Default)]
pub struct Shared {
    pub vault: Option<Vault>,
    pub entries: Vec<Entry>,
}
pub type SharedRef = Arc<Mutex<Shared>>;

pub fn socket_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("journal-{}", std::env::var("USER").unwrap_or_default())));
    let _ = crate::vault::private_dir(&base);
    base.join(std::env::var("JOURNAL_SOCKET").unwrap_or_else(|_| "journal.sock".into()))
}

/// Entry text with the markdown markers removed, for matching and snippets.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let line = ["## ", "# ", "> "].iter().find_map(|p| line.strip_prefix(p)).unwrap_or(line);
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.extend(chars.next()),
                '*' | '`' | '~' => {}
                _ => out.push(c),
            }
        }
    }
    out
}

pub fn snippet(text: &str, query_lower: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let pos = lower.find(query_lower)?;
    if lower.len() != text.len() {
        return Some(text.chars().take(90).collect::<String>().replace('\n', " "));
    }
    let mut start = pos.saturating_sub(40);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (pos + query_lower.len() + 60).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let mut s = text[start..end].replace('\n', " ");
    if start > 0 {
        s.insert(0, '…');
    }
    if end < text.len() {
        s.push('…');
    }
    Some(s)
}

/// Newest first. Matches entry text and the date itself (so "2026-11" finds a month).
pub fn search<'a>(entries: &'a [Entry], query: &str, limit: usize) -> Vec<(&'a Entry, String)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    entries
        .iter()
        .rev()
        .filter_map(|e| {
            let text = plain(&e.text);
            snippet(&text, &q).or_else(|| {
                let date = format!("{} {}", e.date(), e.date().format("%A %B %-d %Y")).to_lowercase();
                date.contains(&q).then(|| text.chars().take(90).collect::<String>().replace('\n', " "))
            })
            .map(|s| (e, s))
        })
        .take(limit)
        .collect()
}

fn parse_date(v: &Value, key: &str) -> Result<Option<NaiveDate>> {
    match v.get(key).and_then(Value::as_str) {
        None | Some("") => Ok(None),
        Some(s) => Ok(Some(
            NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| anyhow!("{key} must be YYYY-MM-DD"))?,
        )),
    }
}

fn handle(shared: &SharedRef, req: &Value, note: &Sender<String>) -> Result<Value> {
    let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or("");
    let (vault, entries) = {
        let s = shared.lock().unwrap();
        (s.vault.clone(), s.entries.clone())
    };
    if cmd == "status" {
        return Ok(json!({"unlocked": vault.is_some(), "entries": entries.len()}));
    }
    let Some(vault) = vault else { bail!("The journal is locked. Unlock it in the app first.") };
    match cmd {
        "search" => {
            let query = req.get("query").and_then(Value::as_str).unwrap_or("");
            let limit = req.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            let hits: Vec<Value> = search(&entries, query, limit)
                .into_iter()
                .map(|(e, s)| json!({"date": e.date().to_string(), "time": e.time_label(), "snippet": s}))
                .collect();
            let _ = note.send(format!("An agent searched for “{query}”"));
            Ok(json!({"results": hits}))
        }
        "read" => {
            let today = Local::now().date_naive();
            let since = parse_date(req, "since")?;
            let until = parse_date(req, "until")?;
            let (since, until) = match (since, until) {
                (None, None) => (today, today),
                (s, u) => (s.unwrap_or(NaiveDate::MIN), u.unwrap_or(today)),
            };
            let days = group_by_day(&entries);
            let parts: Vec<String> =
                days.range(since..=until).map(|(d, es)| day_markdown(*d, es)).collect();
            let _ = note.send(format!("An agent read {} day(s)", parts.len()));
            Ok(json!({"days": parts.len(), "markdown": parts.join("\n\n")}))
        }
        "days" => {
            let days: Vec<Value> = group_by_day(&entries)
                .iter()
                .map(|(d, es)| json!({"date": d.to_string(), "entries": es.len()}))
                .collect();
            Ok(json!({"days": days}))
        }
        "export" => {
            let dir = req.get("dir").and_then(Value::as_str).ok_or_else(|| anyhow!("missing dir"))?;
            let n = vault.export(&entries, std::path::Path::new(dir))?;
            let _ = note.send(format!("Exported {n} day(s) to {dir}"));
            Ok(json!({"days": n, "dir": dir}))
        }
        _ => bail!("unknown command"),
    }
}

pub fn serve(shared: SharedRef, note: Sender<String>) -> Result<()> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line)
                .map_err(|e| anyhow!("bad request: {e}"))
                .and_then(|req| handle(&shared, &req, &note))
            {
                Ok(mut v) => {
                    v["ok"] = json!(true);
                    v
                }
                Err(e) => json!({"ok": false, "error": e.to_string()}),
            };
            let _ = writeln!(&stream, "{reply}");
        }
    });
    Ok(())
}

pub fn request(req: &Value) -> Result<Value> {
    let mut stream = UnixStream::connect(socket_path())
        .map_err(|_| anyhow!("Journal is not running. Open the app and unlock it first."))?;
    writeln!(stream, "{req}")?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let v: Value = serde_json::from_str(&line)?;
    if v["ok"] != json!(true) {
        bail!("{}", v["error"].as_str().unwrap_or("request failed"));
    }
    Ok(v)
}

const USAGE: &str = "journal                      open the app
journal status               is the journal running and unlocked?
journal search <words>       find entries (newest first)
journal read [--since YYYY-MM-DD] [--until YYYY-MM-DD] [--all]
                             print entries as markdown (default: today)
journal days                 list days that have entries
journal export <folder>      write plain markdown and media to a folder
journal mcp                  run as an MCP server for agents (stdio)

Everything except opening the app needs the app running and unlocked.";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

pub fn cli(args: &[String]) -> Result<()> {
    match args[0].as_str() {
        "status" => {
            let v = request(&json!({"cmd": "status"}))?;
            if v["unlocked"] == json!(true) {
                println!("unlocked, {} entries", v["entries"]);
            } else {
                println!("locked");
            }
        }
        "search" => {
            let v = request(&json!({"cmd": "search", "query": args[1..].join(" "), "limit": 50}))?;
            for r in v["results"].as_array().into_iter().flatten() {
                println!("{} {}  {}", r["date"].as_str().unwrap_or(""), r["time"].as_str().unwrap_or(""), r["snippet"].as_str().unwrap_or(""));
            }
        }
        "read" => {
            let since = if args.iter().any(|a| a == "--all") { Some("0001-01-01".to_string()) } else { flag(args, "--since") };
            let v = request(&json!({"cmd": "read", "since": since, "until": flag(args, "--until")}))?;
            println!("{}", v["markdown"].as_str().unwrap_or(""));
        }
        "days" => {
            let v = request(&json!({"cmd": "days"}))?;
            for d in v["days"].as_array().into_iter().flatten() {
                println!("{}  {}", d["date"].as_str().unwrap_or(""), d["entries"]);
            }
        }
        "export" => {
            let dir = args.get(1).ok_or_else(|| anyhow!("usage: journal export <folder>"))?;
            let abs = std::path::absolute(dir)?;
            let v = request(&json!({"cmd": "export", "dir": abs}))?;
            println!("Exported {} day(s) to {}", v["days"], abs.display());
        }
        "mcp" => mcp()?,
        _ => println!("{USAGE}"),
    }
    Ok(())
}

fn mcp_tools() -> Value {
    json!([
        {
            "name": "journal_search",
            "description": "Search the user's private journal. Returns matching entries (date, time, snippet), newest first. Read-only; works only while the journal app is unlocked.",
            "inputSchema": {"type": "object", "properties": {
                "query": {"type": "string", "description": "Words to look for, or part of a date such as 2026-11"},
                "limit": {"type": "integer", "description": "Maximum results (default 20)"}
            }, "required": ["query"]}
        },
        {
            "name": "journal_read",
            "description": "Read the user's journal entries as markdown for a date range (inclusive). With no dates, returns today. Read-only; works only while the journal app is unlocked.",
            "inputSchema": {"type": "object", "properties": {
                "since": {"type": "string", "description": "First day, YYYY-MM-DD"},
                "until": {"type": "string", "description": "Last day, YYYY-MM-DD (default today)"}
            }}
        },
        {
            "name": "journal_days",
            "description": "List the dates that have journal entries, with the number of entries on each.",
            "inputSchema": {"type": "object", "properties": {}}
        }
    ])
}

fn mcp_call(params: &Value) -> Result<String> {
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let mut req = args.clone();
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    req["cmd"] = json!(match name {
        "journal_search" => "search",
        "journal_read" => "read",
        "journal_days" => "days",
        _ => bail!("unknown tool"),
    });
    let v = request(&req)?;
    Ok(match name {
        "journal_read" => {
            let md = v["markdown"].as_str().unwrap_or("");
            if md.is_empty() { "No entries in that range.".to_string() } else { md.to_string() }
        }
        "journal_search" => serde_json::to_string_pretty(&v["results"])?,
        _ => serde_json::to_string_pretty(&v["days"])?,
    })
}

fn mcp() -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue };
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result = match msg["method"].as_str().unwrap_or("") {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2024-11-05")),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "journal", "version": env!("CARGO_PKG_VERSION")}
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": mcp_tools()})),
            "tools/call" => Ok(match mcp_call(&params) {
                Ok(text) => json!({"content": [{"type": "text", "text": text}]}),
                Err(e) => json!({"content": [{"type": "text", "text": e.to_string()}], "isError": true}),
            }),
            _ => Err(json!({"code": -32601, "message": "method not found"})),
        };
        let reply = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        };
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}
