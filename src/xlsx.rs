//! Excel export (.xlsx): one sheet of text, with a bold, frozen header row and column widths
//! that fit the content. An .xlsx file is a zip of a few XML parts; they are stored
//! uncompressed, which every spreadsheet program reads, so no zip library is needed.

use anyhow::{Context, Result};
use std::path::Path;

/// Write `head` and `rows` to `path` as a workbook with one sheet named `sheet`.
pub fn write(path: &Path, sheet: &str, head: &[&str], rows: &[Vec<String>]) -> Result<()> {
    std::fs::write(path, workbook(sheet, head, rows)).with_context(|| format!("writing {}", path.display()))
}

/// The .xlsx file's bytes.
pub fn workbook(sheet: &str, head: &[&str], rows: &[Vec<String>]) -> Vec<u8> {
    const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
    const DOC: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
    let types = format!(
        r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#
    );
    let root_rels = format!(r#"{XML}<Relationships xmlns="{RELS}"><Relationship Id="rId1" Type="{DOC}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#);
    let book = format!(r#"{XML}<workbook xmlns="{MAIN}" xmlns:r="{DOC}"><sheets><sheet name="{}" sheetId="1" r:id="rId1"/></sheets></workbook>"#, esc(&sheet_name(sheet)));
    let book_rels = format!(
        r#"{XML}<Relationships xmlns="{RELS}"><Relationship Id="rId1" Type="{DOC}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{DOC}/styles" Target="styles.xml"/></Relationships>"#
    );
    // Style 0: plain text. Style 1: bold, for the header row.
    let styles = format!(
        r#"{XML}<styleSheet xmlns="{MAIN}"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#
    );

    let mut s = format!(
        r#"{XML}<worksheet xmlns="{MAIN}"><sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews><cols>"#
    );
    for (i, h) in head.iter().enumerate() {
        let longest = rows.iter().filter_map(|r| r.get(i)).map(|c| c.chars().count()).chain(Some(h.chars().count())).max().unwrap_or(0);
        s += &format!(r#"<col min="{n}" max="{n}" width="{}" customWidth="1"/>"#, (longest + 2).clamp(8, 80), n = i + 1);
    }
    s += "</cols><sheetData>";
    let head: Vec<String> = head.iter().map(|h| h.to_string()).collect();
    for (r, row) in std::iter::once(&head).chain(rows).enumerate() {
        s += &format!(r#"<row r="{}">"#, r + 1);
        for (c, text) in row.iter().enumerate() {
            let style = if r == 0 { r#" s="1""# } else { "" };
            s += &format!(r#"<c r="{}{}" t="inlineStr"{style}><is><t xml:space="preserve">{}</t></is></c>"#, column(c), r + 1, esc(&cell(text)));
        }
        s += "</row>";
    }
    s += "</sheetData></worksheet>";

    zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", book.as_bytes()),
        ("xl/_rels/workbook.xml.rels", book_rels.as_bytes()),
        ("xl/styles.xml", styles.as_bytes()),
        ("xl/worksheets/sheet1.xml", s.as_bytes()),
    ])
}

/// Column letters: 0 → A, 25 → Z, 26 → AA.
fn column(mut i: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (i % 26) as u8);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

/// Excel's limits: a sheet name has at most 31 characters and none of `[]:*?/\`.
fn sheet_name(s: &str) -> String {
    let n: String = s.chars().filter(|c| !"[]:*?/\\".contains(*c)).take(31).collect();
    if n.trim().is_empty() {
        "Sheet1".into()
    } else {
        n
    }
}

/// A cell holds at most 32,767 characters, and XML has no place for most control characters.
fn cell(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r')).take(32_767).collect()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A zip of `files`, stored (no compression), with UTF-8 names.
fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32fast::hash(data);
        let (size, name_len) = (data.len() as u32, name.len() as u16);
        // Local file header: version 2.0, UTF-8 names, stored, no date.
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(0x0800u16.to_le_bytes());
        out.extend([0u8; 6]);
        out.extend(crc.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(name_len.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name.as_bytes());
        out.extend(*data);
        // Its central directory entry.
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(0x0800u16.to_le_bytes());
        central.extend([0u8; 6]);
        central.extend(crc.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend(name_len.to_le_bytes());
        central.extend([0u8; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let (start, len, count) = (out.len() as u32, central.len() as u32, files.len() as u16);
    out.extend(central);
    // End of central directory.
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(count.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(len.to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_names() {
        assert_eq!([column(0), column(25), column(26), column(51), column(701), column(702)], ["A", "Z", "AA", "AZ", "ZZ", "AAA"]);
        assert_eq!(sheet_name("Scan results: HKLM\\Software [1]"), "Scan results HKLMSoftware 1");
        assert_eq!(sheet_name("???"), "Sheet1");
        assert_eq!(cell("a\u{1}b\tc"), "ab\tc");
    }

    /// The zip opens with a standard reader (`unzip`, when present) and holds the parts.
    #[test]
    fn writes_a_readable_zip() {
        let rows = vec![vec!["Bad & <odd> \"text\"".to_string(), "C:\\Program Files\\Ünïcode".to_string()]];
        let bytes = workbook("Programs", &["Name", "Location"], &rows);
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        let dir = std::env::temp_dir().join(format!("cleancore-xlsx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.xlsx");
        std::fs::write(&file, &bytes).unwrap();
        if let Ok(o) = std::process::Command::new("unzip").arg("-p").arg(&file).arg("xl/worksheets/sheet1.xml").output() {
            if o.status.success() {
                let xml = String::from_utf8(o.stdout).unwrap();
                assert!(xml.contains("Bad &amp; &lt;odd&gt; &quot;text&quot;"));
                assert!(xml.contains("Ünïcode"));
                assert!(xml.contains(r#"<c r="B2" t="inlineStr">"#));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
