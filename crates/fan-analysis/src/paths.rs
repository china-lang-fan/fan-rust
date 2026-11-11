//! file:// URI 与本地路径互转，对应 Go 版 `paths.go`。

/// 把 `file://` URI 转换为本地路径；非 URI 输入原样返回。
pub fn uri_to_path(uri: &str) -> String {
    match uri.strip_prefix("file://") {
        Some(rest) => {
            // 对常见的百分号编码做最小解码（路径中的中文一般不编码，
            // 这里只处理空格等高频场景），保持与编辑器侧可用路径一致。
            decode_path(rest)
        }
        None => uri.to_string(),
    }
}

/// 把本地路径转换为绝对的 `file://` URI。
pub fn path_to_uri(path: &str) -> String {
    let abs = std::path::absolute(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string());
    format!("file://{abs}")
}

fn decode_path(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_与路径互转() {
        assert_eq!(uri_to_path("file:///a/b.凡"), "/a/b.凡");
        assert_eq!(uri_to_path("/a/b.凡"), "/a/b.凡");
        assert!(path_to_uri("b.凡").starts_with("file:///"));
    }

    #[test]
    fn 解码百分号编码() {
        assert_eq!(uri_to_path("file:///a/b%20c.凡"), "/a/b c.凡");
    }
}
