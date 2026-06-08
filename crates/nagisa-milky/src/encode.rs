//! 统一 [`Segment`] → Milky [`OutgoingSegment`] 段编码（出站）。
//!
//! [`encode`] 把统一段列表翻译成 Milky wire 段数组（供 `actions` 模块发消息）；Milky 出站只有
//! 10 种段，无对应 wire 段的统一段静默跳过（绝不 panic）。媒体资源经 [`source_to_uri`] 收敛为
//! 单个 `uri`（`base64://` / `file://` / `http(s)://`，对齐 Lagrange `ResourceResolver` 认可的
//! 三格式）；base64 用 [`nagisa_core::wire::base64_encode`]（与 OneBot 适配器共用、无外部 crate）。
use nagisa_core::wire::base64_encode;
use nagisa_types::prelude::*;
use nagisa_types::segment::{Forward, ForwardNode};

use crate::wire::{OutgoingForwardedMessage, OutgoingSegment, WireOutImageSubType};

/// `ResourceSource` → 单个 Milky URI。
pub fn source_to_uri(source: &ResourceSource) -> String {
    match source {
        ResourceSource::Url(u) => u.clone(),
        ResourceSource::Bytes(b) => format!("base64://{}", base64_encode(b)),
        ResourceSource::Path(p) => {
            // Lagrange 剥 `file://` 前 7 字符；标准化为 `file://{absolute path}`。
            format!("file://{}", p.display())
        }
    }
}

/// 取媒体段的 uri：优先 send 侧 `source`，否则回退到接收 url（转发场景）。
fn media_uri(media: &Media) -> String {
    if let Some(src) = &media.source {
        source_to_uri(src)
    } else if let Some(recv) = &media.recv {
        recv.url.clone().unwrap_or_default()
    } else {
        String::new()
    }
}

fn image_sub_type(st: ImageSubType) -> WireOutImageSubType {
    match st {
        ImageSubType::Normal => WireOutImageSubType::Normal,
        ImageSubType::Sticker => WireOutImageSubType::Sticker,
        // Milky 无「闪照」线类型;降级为普通图发送(对端不支持阅后即焚)。
        ImageSubType::Flash => WireOutImageSubType::Normal,
    }
}

/// 把一个统一 `Segment` push 进 outgoing 列表。未知/不可发段静默跳过（已尽力降级）。
fn push_segment(out: &mut Vec<OutgoingSegment>, seg: &Segment) {
    match seg {
        Segment::Text(t) => out.push(OutgoingSegment::Text { text: t.clone() }),
        Segment::Mention { user, .. } => out.push(OutgoingSegment::Mention { user_id: user.0 }),
        Segment::MentionAll => out.push(OutgoingSegment::MentionAll {}),
        // Milky 出站 face 只有 face_id/is_large;super-face/FaceType 等是 OneBot 专属,
        // 在 Milky wire 上无对应(丢弃,不 panic)。
        Segment::Face { id, large, .. } => out.push(OutgoingSegment::Face {
            face_id: id.clone(),
            is_large: *large,
        }),
        Segment::Reply { id, .. } => out.push(OutgoingSegment::Reply {
            message_seq: id.seq,
        }),
        Segment::Image { res, sub_type, .. } => out.push(OutgoingSegment::Image {
            uri: media_uri(res),
            sub_type: image_sub_type(*sub_type),
            summary: res.summary.clone(),
        }),
        Segment::Record { res, .. } => out.push(OutgoingSegment::Record { uri: media_uri(res) }),
        Segment::Video { res, thumb, .. } => out.push(OutgoingSegment::Video {
            uri: media_uri(res),
            thumb_uri: thumb.as_ref().map(source_to_uri),
        }),
        Segment::Forward(fwd) => {
            if let Some(seg) = encode_forward(fwd) {
                out.push(seg);
            }
        }
        Segment::LightApp { payload, .. } => out.push(OutgoingSegment::LightApp {
            json_payload: payload.clone(),
        }),
        // File / MarketFace / Xml / Share / Keyboard / Markdown / LongMsg / Raw 等：
        // Milky 出站段只有 10 种（OutgoingSegment），这些统一 Segment 在发送侧无对应 wire 段，
        // 静默跳过（已尽力降级，绝不 panic）。
        _ => {}
    }
}

/// 合并转发 → outgoing forward 段（仅内联 `Nodes` 可发；`Ref` 无法发送，跳过）。
fn encode_forward(fwd: &Forward) -> Option<OutgoingSegment> {
    match fwd {
        // Milky forward 有 title/summary/prompt;gocq 专属的 `news`/`source` 预览字段在
        // Milky wire 上无对应(忽略,不 panic)。
        Forward::Nodes {
            nodes,
            title,
            summary,
            prompt,
            ..
        } => {
            let messages = nodes.iter().map(encode_forward_node).collect();
            Some(OutgoingSegment::Forward {
                messages,
                title: title.clone(),
                preview: None,
                summary: summary.clone(),
                prompt: prompt.clone(),
            })
        }
        Forward::Ref { .. } => None,
    }
}

fn encode_forward_node(node: &ForwardNode) -> OutgoingForwardedMessage {
    OutgoingForwardedMessage {
        user_id: node.user.0,
        sender_name: node.name.clone(),
        segments: encode(&node.content),
    }
}

/// `&[Segment]` → Milky `OutgoingSegment` 列表。
pub fn encode(message: &[Segment]) -> Vec<OutgoingSegment> {
    let mut out = Vec::with_capacity(message.len());
    for seg in message {
        push_segment(&mut out, seg);
    }
    out
}
