//! 统一错误类型。

/// RunPHP 核心错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON 序列化错误: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP 请求错误: {0}")]
    Http(#[from] reqwest::Error),

    #[error("配置错误: {0}")]
    Config(String),

    #[error("运行时错误: {0}")]
    Runtime(String),

    #[error("Caddy/FrankenPHP 错误: {0}")]
    Caddy(String),

    #[error("解压错误: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("{0}")]
    Other(String),
}
