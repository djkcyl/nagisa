//! 媒体资源的两侧形态：发送侧的来源 [`ResourceSource`]、接收侧的引用 [`ResourceRef`]，以及把
//! 两者收进一处的载体 [`Media`]。收发字段天然不对称（发送给字节/路径/URL，接收拿 id/URL），故
//! [`Media`] 用 `source` / `recv` 两个 `Option` 各表一侧。[`Segment`](crate::segment::Segment)
//! 的图片/语音/视频段都内嵌 [`Media`]。
use bytes::Bytes;
use serde_json::Value;
use std::path::PathBuf;

/// 发送侧资源来源。适配器各自序列化为 `base64://` / `file://` / `http(s)://`。
#[derive(Clone, Debug)]
pub enum ResourceSource {
    Bytes(Bytes),
    Path(PathBuf),
    Url(String),
}

/// 接收侧资源引用：真实 URL 可能需懒加载
/// （Milky 文件需另调 `get_*_file_download_url`）。
#[derive(Clone, Debug, Default)]
pub struct ResourceRef {
    pub id: Option<String>,
    pub url: Option<String>,
    pub raw: Value,
}

/// 媒体段载体：发送时持 `source`，接收时持 `recv`。
#[derive(Clone, Debug, Default)]
pub struct Media {
    pub source: Option<ResourceSource>,
    pub recv: Option<ResourceRef>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<u32>,
    pub summary: Option<String>,
}

impl ResourceSource {
    pub fn bytes(b: impl Into<Bytes>) -> Self {
        ResourceSource::Bytes(b.into())
    }
    pub fn url(u: impl Into<String>) -> Self {
        ResourceSource::Url(u.into())
    }
    pub fn path(p: impl Into<PathBuf>) -> Self {
        ResourceSource::Path(p.into())
    }
}

impl Media {
    pub fn from_source(source: ResourceSource) -> Self {
        Media { source: Some(source), ..Default::default() }
    }
    pub fn from_recv(recv: ResourceRef) -> Self {
        Media { recv: Some(recv), ..Default::default() }
    }
}
