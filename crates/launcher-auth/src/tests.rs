use crate::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) async fn mock_server(
    replies: Vec<(u16, serde_json::Value)>,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in replies {
            let (mut stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut data = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let read = stream.read(&mut chunk).await.unwrap();
                assert_ne!(read, 0);
                data.extend_from_slice(&chunk[..read]);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..end]);
                    let len = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if data.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            requests.push(String::from_utf8(data).unwrap());
            let body = if status == 204 {
                String::new()
            } else {
                body.to_string()
            };
            let response = format!(
                "HTTP/1.1 {status} Mock\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\nlocation: http://127.0.0.1:1/credential-leak\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    (base, handle)
}
#[test]
fn validates_profile_and_secret_contract() {
    assert!(GameProfile::new("not-uuid", "Player").is_err());
    assert!(GameProfile::new("00000000000000000000000000000001", "bad name").is_err());
    assert_eq!(Secret::new("secret").expose(), "secret");
}
