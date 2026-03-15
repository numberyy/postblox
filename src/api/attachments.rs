use std::fmt::Write;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{get_inbox_for_org, get_message_for_inbox, AppState};
use crate::models::Attachment;

const MAX_ARCHIVE_SIZE: u64 = 100 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 1000;
const MAX_CSV_ROWS: usize = 50;
const MAX_TEXT_PREVIEW_BYTES: usize = 64 * 1024;
const MAX_CSV_COLS: usize = 10;

pub async fn list(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path((inbox_id, message_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Attachment>>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;
    get_message_for_inbox(&state.pool, message_id, inbox_id).await?;

    let attachments = crate::db::attachments::list_by_message(&state.pool, message_id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(Json(attachments))
}

pub async fn download(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path((inbox_id, message_id, attachment_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;
    get_message_for_inbox(&state.pool, message_id, inbox_id).await?;
    let attachment = get_validated_attachment(&state, attachment_id, message_id).await?;

    let data = read_data(&state, &attachment).await?;

    let disposition = format!(
        "{}; filename=\"{}\"",
        attachment.disposition,
        attachment.filename.replace('"', "\\\"")
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &attachment.content_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CONTENT_LENGTH, data.len())
        .body(Body::from(data))
        .map_err(|e| ApiError::Internal(format!("failed to build attachment response: {e}")))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path((inbox_id, message_id, attachment_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;
    get_message_for_inbox(&state.pool, message_id, inbox_id).await?;
    let attachment = get_validated_attachment(&state, attachment_id, message_id).await?;

    // DB first (reversible if file delete fails), then file
    crate::db::attachments::delete(&state.pool, attachment_id)
        .await
        .map_err(ApiError::from_sqlx)?;

    if let Err(e) =
        crate::storage::delete_attachment(&state.attachment_storage_path, &attachment.storage_key)
            .await
    {
        tracing::warn!(storage_key = %attachment.storage_key, "failed to delete attachment file: {e}");
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn preview(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path((inbox_id, message_id, attachment_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;
    get_message_for_inbox(&state.pool, message_id, inbox_id).await?;
    let attachment = get_validated_attachment(&state, attachment_id, message_id).await?;

    let download_url =
        format!("/api/v1/inboxes/{inbox_id}/messages/{message_id}/attachments/{attachment_id}");

    match classify_preview(&attachment.content_type) {
        PreviewKind::Image => Ok(redirect_response(&download_url)),
        PreviewKind::Pdf => Ok(html_response(render_pdf_html(
            &download_url,
            &attachment.filename,
        ))),
        PreviewKind::Csv => {
            let data = read_data(&state, &attachment).await?;
            Ok(html_response(render_csv_html(&data, &attachment.filename)?))
        }
        PreviewKind::Text => {
            let mut data = read_data(&state, &attachment).await?;
            let truncated = data.len() > MAX_TEXT_PREVIEW_BYTES;
            if truncated {
                data.truncate(MAX_TEXT_PREVIEW_BYTES);
            }
            let filename = attachment.filename.clone();
            let syntect = state.syntect.clone();
            let html = tokio::task::spawn_blocking(move || {
                render_text_html(&data, &filename, truncated, &syntect)
            })
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
            Ok(html_response(html))
        }
        PreviewKind::Unsupported => Ok(unsupported_response(
            "preview not available for this file type",
        )),
    }
}

pub async fn contents(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path((inbox_id, message_id, attachment_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;
    get_message_for_inbox(&state.pool, message_id, inbox_id).await?;
    let attachment = get_validated_attachment(&state, attachment_id, message_id).await?;

    let kind = match classify_archive(&attachment.content_type, &attachment.filename) {
        Some(k) => k,
        None => return Ok(unsupported_response("not an archive file")),
    };

    if attachment.size_bytes as u64 > MAX_ARCHIVE_SIZE {
        return Err(ApiError::BadRequest(format!(
            "archive too large: {} bytes exceeds {MAX_ARCHIVE_SIZE} byte limit",
            attachment.size_bytes,
        )));
    }

    let data = read_data(&state, &attachment).await?;
    let entries = list_archive_entries(kind, &data)
        .map_err(|e| ApiError::Internal(format!("failed to read archive: {e}")))?;

    let body = serde_json::to_vec(&serde_json::json!({ "files": entries }))
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .map_err(|e| ApiError::Internal(e.to_string()))
}

async fn get_validated_attachment(
    state: &AppState,
    attachment_id: Uuid,
    message_id: Uuid,
) -> Result<Attachment, ApiError> {
    let attachment = crate::db::attachments::get_by_id(&state.pool, attachment_id)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;
    if attachment.message_id != message_id {
        return Err(ApiError::NotFound);
    }
    Ok(attachment)
}

async fn read_data(state: &AppState, attachment: &Attachment) -> Result<Vec<u8>, ApiError> {
    crate::storage::read_attachment(&state.attachment_storage_path, &attachment.storage_key)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to read attachment: {e}")))
}

fn redirect_response(url: &str) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, url)
        .body(Body::empty())
        .expect("static redirect builder")
}

fn html_response(body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(body))
        .expect("static html builder")
}

fn unsupported_response(msg: &str) -> Response {
    let body = serde_json::json!({ "error": msg });
    Response::builder()
        .status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_string(&body).expect("json serialize"),
        ))
        .expect("static unsupported builder")
}

#[derive(Debug, PartialEq)]
enum PreviewKind {
    Image,
    Pdf,
    Csv,
    Text,
    Unsupported,
}

fn classify_preview(content_type: &str) -> PreviewKind {
    if content_type.starts_with("image/") {
        PreviewKind::Image
    } else if content_type == "application/pdf" {
        PreviewKind::Pdf
    } else if content_type == "text/csv" {
        PreviewKind::Csv
    } else if content_type.starts_with("text/")
        || content_type == "application/json"
        || content_type == "application/xml"
    {
        PreviewKind::Text
    } else {
        PreviewKind::Unsupported
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn render_pdf_html(download_url: &str, filename: &str) -> String {
    let escaped_url = html_escape(download_url);
    format!(
        "<!DOCTYPE html>\
<html><head><title>{} \u{2014} Preview</title>\
<style>body{{margin:0;height:100vh}}object{{width:100%;height:100%}}</style>\
</head><body>\
<object data=\"{escaped_url}\" type=\"application/pdf\" width=\"100%\" height=\"100%\">\
<p>PDF preview not supported. <a href=\"{escaped_url}\">Download</a></p>\
</object></body></html>",
        html_escape(filename),
    )
}

fn render_csv_html(data: &[u8], filename: &str) -> Result<String, ApiError> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(data);

    let mut html = format!(
        "<!DOCTYPE html>\
<html><head><title>{} \u{2014} Preview</title>\
<style>\
table{{border-collapse:collapse;font-family:monospace;font-size:13px}}\
td,th{{border:1px solid #ddd;padding:4px 8px;max-width:300px;overflow:hidden;\
text-overflow:ellipsis;white-space:nowrap}}\
th{{background:#f5f5f5;position:sticky;top:0}}\
</style></head><body><table>",
        html_escape(filename)
    );

    if let Ok(headers) = reader.headers() {
        html.push_str("<thead><tr>");
        for (i, h) in headers.iter().enumerate() {
            if i >= MAX_CSV_COLS {
                let _ = write!(html, "<th>+{} more</th>", headers.len() - MAX_CSV_COLS);
                break;
            }
            let _ = write!(html, "<th>{}</th>", html_escape(h));
        }
        html.push_str("</tr></thead>");
    }

    html.push_str("<tbody>");
    let mut row_count = 0;
    for result in reader.records().take(MAX_CSV_ROWS) {
        let record = result.map_err(|e| ApiError::BadRequest(format!("CSV parse error: {e}")))?;
        html.push_str("<tr>");
        for (i, field) in record.iter().enumerate() {
            if i >= MAX_CSV_COLS {
                let _ = write!(html, "<td>+{} more</td>", record.len() - MAX_CSV_COLS);
                break;
            }
            let _ = write!(html, "<td>{}</td>", html_escape(field));
        }
        html.push_str("</tr>");
        row_count += 1;
    }
    html.push_str("</tbody></table>");

    if row_count >= MAX_CSV_ROWS {
        let _ = write!(html, "<p><em>Showing first {MAX_CSV_ROWS} rows</em></p>");
    }
    html.push_str("</body></html>");
    Ok(html)
}

fn render_text_html(
    data: &[u8],
    filename: &str,
    truncated: bool,
    res: &super::SyntectResources,
) -> String {
    let text = String::from_utf8_lossy(data);
    let syntax = res
        .syntax_set
        .find_syntax_for_file(filename)
        .ok()
        .flatten()
        .unwrap_or_else(|| res.syntax_set.find_syntax_plain_text());
    let theme = res
        .theme_set
        .themes
        .get("base16-ocean.dark")
        .or_else(|| res.theme_set.themes.values().next())
        .expect("syntect ships with default themes");

    let highlighted =
        syntect::html::highlighted_html_for_string(&text, &res.syntax_set, syntax, theme)
            .unwrap_or_else(|_| format!("<pre>{}</pre>", html_escape(&text)));

    let notice = if truncated {
        "<p><em>Showing first 64 KB of file</em></p>"
    } else {
        ""
    };

    format!(
        "<!DOCTYPE html>\
<html><head><title>{} \u{2014} Preview</title>\
<style>body{{margin:0;padding:1em;font-family:monospace}}pre{{overflow-x:auto}}</style>\
</head><body>{}{}</body></html>",
        html_escape(filename),
        highlighted,
        notice,
    )
}

#[derive(Debug, PartialEq)]
enum ArchiveKind {
    Zip,
    TarGz,
    Tar,
}

fn classify_archive(content_type: &str, filename: &str) -> Option<ArchiveKind> {
    match content_type {
        "application/zip" | "application/x-zip-compressed" => Some(ArchiveKind::Zip),
        "application/gzip" | "application/x-gzip" | "application/x-compressed-tar" => {
            if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
                Some(ArchiveKind::TarGz)
            } else {
                None
            }
        }
        "application/x-tar" => Some(ArchiveKind::Tar),
        _ => {
            let lower = filename.to_lowercase();
            if lower.ends_with(".zip") {
                Some(ArchiveKind::Zip)
            } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
                Some(ArchiveKind::TarGz)
            } else if lower.ends_with(".tar") {
                Some(ArchiveKind::Tar)
            } else {
                None
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct ArchiveEntry {
    name: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    compressed_size: Option<u64>,
}

fn sanitize_entry_name(name: &str) -> String {
    name.replace('\\', "/")
        .split('/')
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .collect::<Vec<_>>()
        .join("/")
}

fn list_archive_entries(kind: ArchiveKind, data: &[u8]) -> Result<Vec<ArchiveEntry>, String> {
    match kind {
        ArchiveKind::Zip => list_zip_entries(data),
        ArchiveKind::TarGz => list_tar_gz_entries(data),
        ArchiveKind::Tar => list_tar_entries(data),
    }
}

fn list_zip_entries(data: &[u8]) -> Result<Vec<ArchiveEntry>, String> {
    let reader = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("invalid zip: {e}"))?;
    let count = archive.len().min(MAX_ARCHIVE_ENTRIES);
    let mut entries = Vec::with_capacity(count);

    for i in 0..count {
        let file = archive
            .by_index(i)
            .map_err(|e| format!("zip entry error: {e}"))?;
        entries.push(ArchiveEntry {
            name: sanitize_entry_name(file.name()),
            size: file.size(),
            compressed_size: Some(file.compressed_size()),
        });
    }
    Ok(entries)
}

fn list_tar_entries(data: &[u8]) -> Result<Vec<ArchiveEntry>, String> {
    collect_tar_entries(tar::Archive::new(std::io::Cursor::new(data)), "tar")
}

fn list_tar_gz_entries(data: &[u8]) -> Result<Vec<ArchiveEntry>, String> {
    let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(data));
    collect_tar_entries(tar::Archive::new(gz), "tar.gz")
}

fn collect_tar_entries<R: std::io::Read>(
    mut archive: tar::Archive<R>,
    label: &str,
) -> Result<Vec<ArchiveEntry>, String> {
    let mut entries = Vec::new();
    for entry in archive
        .entries()
        .map_err(|e| format!("invalid {label}: {e}"))?
    {
        if entries.len() >= MAX_ARCHIVE_ENTRIES {
            break;
        }
        let entry = entry.map_err(|e| format!("{label} entry error: {e}"))?;
        let name = entry
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| format!("{label} path error: {e}"))?;
        entries.push(ArchiveEntry {
            name: sanitize_entry_name(&name),
            size: entry.size(),
            compressed_size: None,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_syntect() -> crate::api::SyntectResources {
        crate::api::SyntectResources {
            syntax_set: syntect::parsing::SyntaxSet::load_defaults_newlines(),
            theme_set: syntect::highlighting::ThemeSet::load_defaults(),
        }
    }

    #[test]
    fn test_content_disposition_escapes_quotes() {
        let filename = r#"file"name.txt"#;
        let disposition = format!("attachment; filename=\"{}\"", filename.replace('"', "\\\""));
        assert_eq!(disposition, r#"attachment; filename="file\"name.txt""#);
    }

    #[test]
    fn test_content_disposition_normal_filename() {
        let filename = "report.pdf";
        let disposition = format!("attachment; filename=\"{}\"", filename.replace('"', "\\\""));
        assert_eq!(disposition, "attachment; filename=\"report.pdf\"");
    }

    // --- Preview classification ---

    #[test]
    fn test_preview_classify_image_types() {
        assert_eq!(classify_preview("image/png"), PreviewKind::Image);
        assert_eq!(classify_preview("image/jpeg"), PreviewKind::Image);
        assert_eq!(classify_preview("image/gif"), PreviewKind::Image);
        assert_eq!(classify_preview("image/webp"), PreviewKind::Image);
        assert_eq!(classify_preview("image/svg+xml"), PreviewKind::Image);
    }

    #[test]
    fn test_preview_classify_pdf() {
        assert_eq!(classify_preview("application/pdf"), PreviewKind::Pdf);
    }

    #[test]
    fn test_preview_classify_csv() {
        assert_eq!(classify_preview("text/csv"), PreviewKind::Csv);
    }

    #[test]
    fn test_preview_classify_text_types() {
        assert_eq!(classify_preview("text/plain"), PreviewKind::Text);
        assert_eq!(classify_preview("text/html"), PreviewKind::Text);
        assert_eq!(classify_preview("text/markdown"), PreviewKind::Text);
        assert_eq!(classify_preview("application/json"), PreviewKind::Text);
        assert_eq!(classify_preview("application/xml"), PreviewKind::Text);
    }

    #[test]
    fn test_preview_classify_unsupported() {
        assert_eq!(
            classify_preview("application/octet-stream"),
            PreviewKind::Unsupported
        );
        assert_eq!(
            classify_preview("application/zip"),
            PreviewKind::Unsupported
        );
        assert_eq!(classify_preview("audio/mpeg"), PreviewKind::Unsupported);
        assert_eq!(classify_preview("video/mp4"), PreviewKind::Unsupported);
    }

    // --- Preview rendering ---

    #[test]
    fn test_preview_csv_renders_table() {
        let csv_data = b"name,age,city\nAlice,30,NYC\nBob,25,LA\n";
        let html = render_csv_html(csv_data, "data.csv").unwrap();
        assert!(html.contains("<th>name</th>"));
        assert!(html.contains("<th>age</th>"));
        assert!(html.contains("<td>Alice</td>"));
        assert!(html.contains("<td>30</td>"));
        assert!(html.contains("<td>Bob</td>"));
        assert!(html.contains("data.csv"));
    }

    #[test]
    fn test_preview_csv_caps_rows() {
        let mut csv_data = String::from("x\n");
        for i in 0..100 {
            csv_data.push_str(&format!("{i}\n"));
        }
        let html = render_csv_html(csv_data.as_bytes(), "big.csv").unwrap();
        assert!(html.contains("Showing first 50 rows"));
        let row_count = html.matches("<tr>").count();
        assert_eq!(row_count, 51); // 1 header + 50 data rows
    }

    #[test]
    fn test_preview_csv_caps_columns() {
        let header_line = (0..15).map(|i| format!("col{i}")).collect::<Vec<_>>();
        let data_line = (0..15).map(|i| format!("v{i}")).collect::<Vec<_>>();
        let csv_data = format!("{}\n{}\n", header_line.join(","), data_line.join(","));

        let html = render_csv_html(csv_data.as_bytes(), "wide.csv").unwrap();
        assert!(html.contains("+5 more"));
        assert!(html.contains("<th>col0</th>"));
        assert!(html.contains("<th>col9</th>"));
        assert!(!html.contains("<th>col10</th>"));
    }

    #[test]
    fn test_preview_csv_html_escapes_xss() {
        let csv_data = b"name\n<script>alert(1)</script>\n";
        let html = render_csv_html(csv_data, "xss.csv").unwrap();
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn test_preview_text_renders_html() {
        let code = b"fn main() {\n    println!(\"hello\");\n}";
        let res = test_syntect();
        let html = render_text_html(code, "main.rs", false, &res);
        assert!(html.contains("main.rs"));
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(!html.contains("64 KB"));
    }

    #[test]
    fn test_preview_text_handles_invalid_utf8() {
        let res = test_syntect();
        let data = vec![0xFF, 0xFE, 0x68, 0x65, 0x6C, 0x6C, 0x6F];
        let html = render_text_html(&data, "binary.txt", false, &res);
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn test_preview_pdf_renders_object_tag() {
        let html = render_pdf_html("/download/test.pdf", "report.pdf");
        assert!(html.contains("application/pdf"));
        assert!(html.contains("/download/test.pdf"));
        assert!(html.contains("report.pdf"));
    }

    #[test]
    fn test_preview_pdf_html_escapes_url() {
        let html = render_pdf_html("/download?file=a&b=c", "test.pdf");
        assert!(html.contains("file=a&amp;b=c"));
    }

    // --- Archive classification ---

    #[test]
    fn test_archive_classify_zip() {
        assert_eq!(
            classify_archive("application/zip", "test.zip"),
            Some(ArchiveKind::Zip)
        );
        assert_eq!(
            classify_archive("application/x-zip-compressed", "test.zip"),
            Some(ArchiveKind::Zip)
        );
    }

    #[test]
    fn test_archive_classify_tar_gz() {
        assert_eq!(
            classify_archive("application/gzip", "test.tar.gz"),
            Some(ArchiveKind::TarGz)
        );
        assert_eq!(
            classify_archive("application/x-gzip", "data.tgz"),
            Some(ArchiveKind::TarGz)
        );
        assert_eq!(
            classify_archive("application/x-compressed-tar", "archive.tar.gz"),
            Some(ArchiveKind::TarGz)
        );
    }

    #[test]
    fn test_archive_classify_tar() {
        assert_eq!(
            classify_archive("application/x-tar", "test.tar"),
            Some(ArchiveKind::Tar)
        );
    }

    #[test]
    fn test_archive_classify_by_extension_fallback() {
        assert_eq!(
            classify_archive("application/octet-stream", "backup.zip"),
            Some(ArchiveKind::Zip)
        );
        assert_eq!(
            classify_archive("application/octet-stream", "backup.tar.gz"),
            Some(ArchiveKind::TarGz)
        );
        assert_eq!(
            classify_archive("application/octet-stream", "backup.tar"),
            Some(ArchiveKind::Tar)
        );
    }

    #[test]
    fn test_archive_classify_non_archive() {
        assert_eq!(classify_archive("text/plain", "readme.txt"), None);
        assert_eq!(classify_archive("image/png", "photo.png"), None);
        assert_eq!(
            classify_archive("application/octet-stream", "data.bin"),
            None
        );
    }

    #[test]
    fn test_archive_gzip_without_tar_extension_rejected() {
        assert_eq!(classify_archive("application/gzip", "file.gz"), None);
    }

    // --- Archive listing ---

    #[test]
    fn test_archive_zip_listing() {
        let buf = create_test_zip(&[("hello.txt", b"world"), ("sub/data.csv", b"a,b,c")]);
        let entries = list_zip_entries(&buf).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "hello.txt");
        assert_eq!(entries[0].size, 5);
        assert!(entries[0].compressed_size.is_some());
        assert_eq!(entries[1].name, "sub/data.csv");
    }

    #[test]
    fn test_archive_tar_gz_listing() {
        let buf = create_test_tar_gz(&[("readme.md", b"# Hello"), ("src/main.rs", b"fn main(){}")]);
        let entries = list_tar_gz_entries(&buf).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "readme.md");
        assert_eq!(entries[0].size, 7);
        assert!(entries[0].compressed_size.is_none());
        assert_eq!(entries[1].name, "src/main.rs");
    }

    #[test]
    fn test_archive_tar_listing() {
        let buf = create_test_tar(&[("file.txt", b"content")]);
        let entries = list_tar_entries(&buf).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "file.txt");
        assert_eq!(entries[0].size, 7);
    }

    #[test]
    fn test_archive_zip_entry_cap() {
        let files: Vec<(String, Vec<u8>)> = (0..1100)
            .map(|i| (format!("file_{i}.txt"), vec![b'x']))
            .collect();
        let file_refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_slice()))
            .collect();
        let buf = create_test_zip(&file_refs);
        let entries = list_zip_entries(&buf).unwrap();
        assert_eq!(entries.len(), MAX_ARCHIVE_ENTRIES);
    }

    #[test]
    fn test_archive_invalid_zip_returns_error() {
        let result = list_zip_entries(b"not a zip file");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid zip"));
    }

    #[test]
    fn test_archive_invalid_tar_gz_returns_error() {
        let result = list_tar_gz_entries(b"not a tar.gz file");
        assert!(result.is_err());
    }

    // --- Path traversal ---

    #[test]
    fn test_sanitize_entry_name_strips_traversal() {
        assert_eq!(sanitize_entry_name("../etc/passwd"), "etc/passwd");
        assert_eq!(sanitize_entry_name("../../secret"), "secret");
        assert_eq!(sanitize_entry_name("./file.txt"), "file.txt");
    }

    #[test]
    fn test_sanitize_entry_name_strips_absolute_paths() {
        assert_eq!(sanitize_entry_name("/etc/passwd"), "etc/passwd");
        assert_eq!(
            sanitize_entry_name("///root/.ssh/id_rsa"),
            "root/.ssh/id_rsa"
        );
    }

    #[test]
    fn test_sanitize_entry_name_normalizes_backslashes() {
        assert_eq!(sanitize_entry_name("dir\\file.txt"), "dir/file.txt");
        assert_eq!(sanitize_entry_name("..\\..\\secret"), "secret");
    }

    #[test]
    fn test_sanitize_entry_name_preserves_valid_paths() {
        assert_eq!(sanitize_entry_name("src/main.rs"), "src/main.rs");
        assert_eq!(
            sanitize_entry_name("deeply/nested/path/file.txt"),
            "deeply/nested/path/file.txt"
        );
    }

    // --- HTML escape ---

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("x=\"y\""), "x=&quot;y&quot;");
        assert_eq!(html_escape("normal text"), "normal text");
    }

    // --- Archive serialization ---

    #[test]
    fn test_archive_entry_serialization() {
        let entry = ArchiveEntry {
            name: "test.txt".to_string(),
            size: 100,
            compressed_size: Some(50),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["name"], "test.txt");
        assert_eq!(json["size"], 100);
        assert_eq!(json["compressed_size"], 50);

        let entry_no_compressed = ArchiveEntry {
            name: "tar_file.txt".to_string(),
            size: 200,
            compressed_size: None,
        };
        let json2 = serde_json::to_value(&entry_no_compressed).unwrap();
        assert_eq!(json2["name"], "tar_file.txt");
        assert!(json2.get("compressed_size").is_none());
    }

    // --- Test helpers ---

    fn create_test_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn create_test_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, *name, *data).unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn create_test_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let tar_data = create_test_tar(files);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_data).unwrap();
        encoder.finish().unwrap()
    }
}
