//! 进度同步：与服务端 /api/progress 双向同步（书/章/页）
//!
//! 服务端是设备侧真值（自研固件按键发 page_turn，服务端折算）；
//! 阅读器是主机侧真值。启动时服务端状态优先，之后每次存进度同时上送。

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Entry {
    book: String,
    chapter: usize,
    page: usize,
}

pub struct Sync {
    base: String,
    client: reqwest::Client,
}

impl Sync {
    pub fn new(base: String) -> Self {
        Sync {
            base,
            client: reqwest::Client::new(),
        }
    }

    /// 拉取某本书在服务端的进度（无记录返回 None）
    pub async fn pull(&self, book: &str) -> Result<Option<(usize, usize)>> {
        let entries: Vec<Entry> = self
            .client
            .get(format!("{}/api/progress", self.base))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .with_context(|| "连接服务端失败")?
            .error_for_status()
            .with_context(|| "服务端返回错误")?
            .json()
            .await
            .with_context(|| "进度响应解析失败")?;
        Ok(entries
            .iter()
            .find(|e| e.book == book)
            .map(|e| (e.chapter, e.page)))
    }

    /// 上送绝对进度（书/章/页）
    pub async fn push(&self, book: &str, chapter: usize, page: usize) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}/api/progress", self.base))
            .timeout(std::time::Duration::from_secs(10))
            .json(&Entry {
                book: book.into(),
                chapter,
                page,
            })
            .send()
            .await
            .with_context(|| "连接服务端失败")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("HTTP {status}: {body}");
        }
        Ok(())
    }

    /// 查询服务端某书的页面缓存状态（章节页数列表 + 已缓存页数）
    pub async fn display_status(&self, book: &str) -> Result<DisplayStatus> {
        self.client
            .get(format!("{}/api/display/status", self.base))
            .query(&[("book", book)])
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .with_context(|| "连接服务端失败")?
            .error_for_status()
            .with_context(|| "服务端返回错误")?
            .json()
            .await
            .with_context(|| "缓存状态响应解析失败")
    }

    /// 登记书目结构（每章页数），服务端据此做跨章翻页折算
    pub async fn display_book(&self, book: &str, chapters: &[usize]) -> Result<()> {
        self.post_json(
            "/api/display/book",
            &serde_json::json!({ "book": book, "chapters": chapters }),
            30,
        )
        .await
    }

    /// 上传某章的页位图（data 为 base64 的 1bpp 裸位图）
    pub async fn display_pages(
        &self,
        book: &str,
        chapter: usize,
        pages: &[(usize, String)],
    ) -> Result<()> {
        let payload = serde_json::json!({
            "book": book,
            "chapter": chapter,
            "pages": pages
                .iter()
                .map(|(p, data)| serde_json::json!({ "page": p, "data": data }))
                .collect::<Vec<_>>(),
        });
        self.post_json("/api/display/pages", &payload, 120).await
    }

    /// 上传待机页位图（书名+操作说明，400×300 1bpp base64）
    pub async fn display_standby(&self, book: &str, data_b64: &str) -> Result<()> {
        self.post_json(
            "/api/display/standby",
            &serde_json::json!({ "book": book, "data": data_b64 }),
            60,
        )
        .await
    }

    /// 清空某书的页面缓存（换书或渲染参数变化时重传）
    pub async fn display_clear(&self, book: &str) -> Result<()> {
        self.post_json("/api/display/clear", &serde_json::json!({ "book": book }), 30)
            .await
    }

    async fn post_json(&self, path: &str, payload: &serde_json::Value, timeout_s: u64) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}{path}", self.base))
            .timeout(std::time::Duration::from_secs(timeout_s))
            .json(payload)
            .send()
            .await
            .with_context(|| "连接服务端失败")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("HTTP {status}: {body}");
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct DisplayStatus {
    pub chapters: Vec<usize>,
    pub cached_pages: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// 进程内 mock 服务端：GET /api/progress 回固定数组，POST 记录 body
    async fn spawn_mock() -> (String, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let posted: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let posted_cloned = posted.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let posted = posted_cloned.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let body_start = buf[..n]
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|p| p + 4)
                        .unwrap_or(n);
                    let is_post = buf.starts_with(b"POST");
                    let resp_body: &[u8] = if is_post {
                        *posted.lock().unwrap() = buf[body_start..n].to_vec();
                        br#"{"ok": true}"#
                    } else {
                        br#"[{"book":"/books/x.txt","chapter":3,"page":7}]"#
                    };
                    let head = if is_post {
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                    } else {
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                    };
                    let resp = format!(
                        "{head}Content-Length: {}\r\nConnection: close\r\n\r\n",
                        resp_body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.write_all(resp_body).await;
                });
            }
        });
        (format!("http://{addr}"), posted)
    }

    #[tokio::test]
    async fn pull_and_push_roundtrip() {
        let (base, posted) = spawn_mock().await;
        let sync = Sync::new(base);

        // pull：命中 book 返回 (chapter, page)
        let state = sync.pull("/books/x.txt").await.unwrap();
        assert_eq!(state, Some((3, 7)));
        // pull：未命中的 book 返回 None
        assert_eq!(sync.pull("/books/other.txt").await.unwrap(), None);

        // push：POST body 是服务端约定的 {book, chapter, page}
        sync.push("/books/y.txt", 12, 34).await.unwrap();
        let sent = String::from_utf8(posted.lock().unwrap().clone()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert_eq!(v["book"], "/books/y.txt");
        assert_eq!(v["chapter"], 12);
        assert_eq!(v["page"], 34);
    }
}
