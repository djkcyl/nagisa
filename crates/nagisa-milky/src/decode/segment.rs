//! 消息段 decode：Milky `IncomingSegment` → 统一 [`Segment`]。文本/提及/表情/回复/图片/
//! 语音/视频/文件/转发/商城表情/小程序/XML 逐型映射；未识别段 → `Segment::Raw{protocol:
//! Milky}`，绝不丢弃。供消息事件与转发节点共用。
use super::*;

/// 接收段列表 → 统一 `Segment` 列表。
pub fn decode_segments(segments: &[IncomingSegment], peer: Peer) -> Vec<Segment> {
    segments.iter().map(|s| decode_segment(s, peer)).collect()
}

fn recv_media(resource_id: &str, temp_url: &str) -> Media {
    Media::from_recv(ResourceRef {
        id: (!resource_id.is_empty()).then(|| resource_id.to_string()),
        url: (!temp_url.is_empty()).then(|| temp_url.to_string()),
        raw: Value::Null,
    })
}

fn decode_segment(seg: &IncomingSegment, peer: Peer) -> Segment {
    match seg {
        IncomingSegment::Text { text } => Segment::Text(text.clone()),
        IncomingSegment::Mention { user_id, name } => Segment::Mention {
            user: Uin(*user_id),
            name: name.clone(),
        },
        IncomingSegment::MentionAll {} => Segment::MentionAll,
        // Milky 只带 face_id/is_large;OneBot 专属的 super-face/FaceType 扩展
        // (result_id/chain_count/sub_type)在 Milky wire 上无对应字段 → None。
        IncomingSegment::Face { face_id, is_large } => Segment::Face {
            id: face_id.clone(),
            large: *is_large,
            result_id: None,
            chain_count: None,
            sub_type: None,
        },
        IncomingSegment::Reply {
            message_seq,
            sender_id,
            time,
            segments,
            ..
        } => Segment::Reply {
            id: MessageId::from_seq(peer, *message_seq),
            sender: sender_id.map(Uin),
            time: *time,
            quoted: decode_segments(segments, peer),
        },
        IncomingSegment::Image {
            resource_id,
            temp_url,
            width,
            height,
            summary,
            sub_type,
        } => {
            let mut media = recv_media(resource_id, temp_url);
            media.width = (*width > 0).then_some(*width as u32);
            media.height = (*height > 0).then_some(*height as u32);
            media.summary = (!summary.is_empty()).then(|| summary.clone());
            Segment::Image {
                res: media,
                sub_type: image_sub_type(*sub_type),
                hints: nagisa_types::segment::MediaSendHints::default(),
            }
        }
        IncomingSegment::Record {
            resource_id,
            temp_url,
            duration,
        } => {
            let mut media = recv_media(resource_id, temp_url);
            media.duration = (*duration > 0).then_some(*duration as u32);
            Segment::Record { res: media, magic: None, hints: nagisa_types::segment::MediaSendHints::default() }
        }
        IncomingSegment::Video {
            resource_id,
            temp_url,
            width,
            height,
            duration,
        } => {
            let mut media = recv_media(resource_id, temp_url);
            media.width = (*width > 0).then_some(*width as u32);
            media.height = (*height > 0).then_some(*height as u32);
            media.duration = (*duration > 0).then_some(*duration as u32);
            Segment::Video { res: media, hints: nagisa_types::segment::MediaSendHints::default(), thumb: None }
        }
        IncomingSegment::File {
            file_id,
            file_name,
            file_size,
            file_hash,
        } => Segment::File {
            id: file_id.clone(),
            name: file_name.clone(),
            size: (*file_size).max(0) as u64,
            hash: file_hash.clone(),
            url: None,
        },
        IncomingSegment::Forward {
            forward_id,
            title,
            preview,
            summary,
        } => Segment::Forward(Forward::Ref {
            id: forward_id.clone(),
            title: title.clone(),
            preview: preview.clone(),
            summary: summary.clone(),
        }),
        IncomingSegment::MarketFace {
            emoji_package_id,
            emoji_id,
            key,
            summary,
            url,
        } => Segment::MarketFace {
            package_id: *emoji_package_id,
            emoji_id: emoji_id.clone(),
            key: key.clone(),
            summary: (!summary.is_empty()).then(|| summary.clone()),
            url: (!url.is_empty()).then(|| url.clone()),
        },
        IncomingSegment::LightApp {
            app_name,
            json_payload,
        } => Segment::LightApp {
            app_name: (!app_name.is_empty()).then(|| app_name.clone()),
            payload: json_payload.clone(),
        },
        IncomingSegment::Xml {
            service_id,
            xml_payload,
        } => Segment::Xml {
            service_id: Some(*service_id),
            payload: xml_payload.clone(),
        },
        IncomingSegment::Unknown { kind, data } => {
            // 将保留的原始 data 转换为 Map，以保留完整信息。
            let data_map = match data {
                Value::Object(m) => m.clone(),
                other => {
                    let mut m = Map::new();
                    if !other.is_null() {
                        m.insert("value".into(), other.clone());
                    }
                    m
                }
            };
            Segment::Raw {
                protocol: Protocol::Milky,
                kind: kind.clone(),
                data: data_map,
            }
        }
    }
}

fn image_sub_type(st: WireImageSubType) -> ImageSubType {
    match st {
        WireImageSubType::Sticker => ImageSubType::Sticker,
        WireImageSubType::Normal | WireImageSubType::Unknown => ImageSubType::Normal,
    }
}
