//! "Export List to": TXT, CSV, Excel, HTML, XML, JSON.

use crate::model::Issue;
use anyhow::{bail, Result};
use std::fmt::Write;
use std::path::Path;

pub fn export(issues: &[Issue], path: &Path) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("txt").to_ascii_lowercase();
    if ext == "xlsx" {
        let rows: Vec<Vec<String>> = issues.iter().map(|i| vec![i.category.to_string(), i.location.clone(), i.detail.clone()]).collect();
        return crate::xlsx::write(path, "Scan results", &["Category", "Location", "Detail"], &rows);
    }
    let body = match ext.as_str() {
        "txt" => to_txt(issues),
        "csv" => to_csv(issues),
        "html" | "htm" => to_html(issues),
        "xml" => to_xml(issues),
        "json" => serde_json::to_string_pretty(issues)?,
        other => bail!("unsupported export format '.{other}' (txt, csv, xlsx, html, xml, json)"),
    };
    std::fs::write(path, body)?;
    Ok(())
}

pub fn to_txt(issues: &[Issue]) -> String {
    let mut s = String::new();
    let mut cat = "";
    for i in issues {
        if i.category != cat {
            cat = i.category;
            let n = issues.iter().filter(|x| x.category == cat).count();
            let _ = writeln!(s, "\n{cat} ({n})");
        }
        let _ = writeln!(s, "  {}\n      {}", i.location, i.detail);
    }
    s
}

fn to_csv(issues: &[Issue]) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut s = String::from("Category,Location,Detail\r\n");
    for i in issues {
        let _ = write!(s, "{},{},{}\r\n", q(i.category), q(&i.location), q(&i.detail));
    }
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn to_html(issues: &[Issue]) -> String {
    let mut s = String::from(
        "<!doctype html><meta charset=utf-8><title>CleanCore scan results</title>\
         <style>body{font:14px system-ui;margin:2em}td,th{padding:4px 8px;border-bottom:1px solid #ddd;text-align:left}</style>\
         <table><tr><th>Category</th><th>Location</th><th>Detail</th></tr>",
    );
    for i in issues {
        let _ = write!(s, "<tr><td>{}</td><td>{}</td><td>{}</td></tr>", esc(i.category), esc(&i.location), esc(&i.detail));
    }
    s.push_str("</table>");
    s
}

fn to_xml(issues: &[Issue]) -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<issues>\n");
    for i in issues {
        let _ = writeln!(s, "  <issue category=\"{}\" location=\"{}\" detail=\"{}\"/>", esc(i.category), esc(&i.location), esc(&i.detail));
    }
    s.push_str("</issues>\n");
    s
}
