use crate::ApiError;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

pub(super) async fn drain_progress(
    mut reader: impl AsyncRead + Unpin,
    maximum: usize,
    idle: Duration,
) -> Result<(), ApiError> {
    let mut buffer = vec![0u8; 8192];
    let mut count = 0usize;
    loop {
        let read = tokio::time::timeout(idle, reader.read(&mut buffer))
            .await
            .map_err(|_| {
                ApiError::new(
                    "SOURCE_DOWNLOAD_STALLED",
                    "下载连续 30 秒没有进度，请检查网络或更换下载线路。",
                )
            })?
            .map_err(|_| ApiError::new("SOURCE_GIT_FAILED", "下载应用失败。"))?;
        if read == 0 {
            return Ok(());
        }
        count = count
            .checked_add(read)
            .filter(|count| *count <= maximum)
            .ok_or_else(|| ApiError::new("SOURCE_LIMIT_EXCEEDED", "下载进度输出超过预算。"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn a_stalled_stream_fails_and_real_progress_resets_the_idle_budget() {
        let (_writer, reader) = tokio::io::duplex(64);
        assert_eq!(
            drain_progress(reader, 64, Duration::from_millis(30))
                .await
                .unwrap_err()
                .code,
            "SOURCE_DOWNLOAD_STALLED"
        );
        let (mut writer, reader) = tokio::io::duplex(64);
        let output = tokio::spawn(async move {
            for _ in 0..4 {
                writer.write_all(b"progress").await.unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        drain_progress(reader, 64, Duration::from_millis(50))
            .await
            .unwrap();
        output.await.unwrap();
    }
}
