//! 把统一的 `nagisa` 段编码成 OneBot v11 wire 段数组。
//!
//! [`encode_segments`] 是适配器 `send` 用的入口;没有 OneBot wire 形态的段被丢弃(返回 `None`),
//! 而非发一个无效段。两个 crate 内部辅助函数与动作面共用,使其编码只此一处:`encode_source`
//! (`ResourceSource` → OneBot `file` 字符串:`base64://` / `file://` / URL)与 `encode_forward_node`
//! (一个合并转发 `node` 对象)。
use crate::wire::WireSegment;
use nagisa_types::prelude::*;
use nagisa_types::segment::{ContactKind, Forward, ForwardNode, MusicShare};
use serde_json::{Map, Value};

/// 把一个合并转发节点编码成 OneBot 线格式的 `{type:"node", data:{user_id, nickname,
/// content[, time]}}` 对象。两条转发发送路径（消息段里的 `Segment::Forward(Forward::Nodes)`
/// 与显式 `send_*_forward` 动作）共用此处，避免节点编码两份漂移；外层形态差异（裸数组 vs
/// `{nodes:[..]}`+preview 包装）保留在各自调用处。
pub(crate) fn encode_forward_node(n: &ForwardNode) -> Value {
    let content = encode_segments(&n.content);
    let content_json = serde_json::to_value(&content).unwrap_or(Value::Array(vec![]));
    let mut node_data = obj(vec![
        ("user_id", Value::String(n.user.0.to_string())),
        ("nickname", Value::String(n.name.clone())),
        ("content", content_json),
    ]);
    if let Some(t) = n.time {
        node_data.insert("time".into(), Value::from(t));
    }
    Value::Object(obj(vec![("type", Value::String("node".into())), ("data", Value::Object(node_data))]))
}

/// 把 `ResourceSource` 序列化成 OneBot 的 `file` 字段形式:
/// `base64://…`、`file://…` 或 `http(s)://…` URL。
pub(crate) fn encode_source(src: &ResourceSource) -> String {
    match src {
        ResourceSource::Url(u) => u.clone(),
        ResourceSource::Path(p) => format!("file://{}", p.display()),
        ResourceSource::Bytes(b) => {
            format!("base64://{}", nagisa_core::wire::base64_encode(b))
        }
    }
}

/// 把 `Media` 解析成发送用的 wire `file` 字符串。优先显式的 `source`;否则回退到接收到的
/// `id`/`url`(按引用再发)。
fn media_file(media: &Media) -> Option<String> {
    if let Some(src) = &media.source {
        return Some(encode_source(src));
    }
    if let Some(recv) = &media.recv {
        return recv.id.clone().or_else(|| recv.url.clone());
    }
    None
}

fn obj(pairs: Vec<(&str, Value)>) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// 把 image/record/video 的发送提示写进 data（OneBot 用 0/1 表示 cache/proxy 布尔）。
fn apply_send_hints(data: &mut Map<String, Value>, hints: &nagisa_types::segment::MediaSendHints) {
    if let Some(c) = hints.cache {
        data.insert("cache".into(), Value::from(if c { 1 } else { 0 }));
    }
    if let Some(p) = hints.proxy {
        data.insert("proxy".into(), Value::from(if p { 1 } else { 0 }));
    }
    if let Some(t) = hints.timeout {
        data.insert("timeout".into(), Value::from(t));
    }
}

/// 把一段统一段编码成 OneBot wire 段。没有 OneBot 表示的段经 `Segment::Raw` round-trip 降级。
pub fn encode_segments(segs: &[Segment]) -> Vec<WireSegment> {
    segs.iter().filter_map(encode_segment).collect()
}

fn encode_segment(seg: &Segment) -> Option<WireSegment> {
    let ws = match seg {
        Segment::Text(t) => WireSegment::new("text", obj(vec![("text", Value::String(t.clone()))])),
        Segment::Mention { user, .. } => WireSegment::new("at", obj(vec![("qq", Value::String(user.0.to_string()))])),
        Segment::MentionAll => WireSegment::new("at", obj(vec![("qq", Value::String("all".into()))])),
        Segment::Face { id, large, result_id, chain_count, sub_type } => {
            let mut data = obj(vec![("id", Value::String(id.clone()))]);
            if *large {
                data.insert("large".into(), Value::Bool(true));
            }
            // NapCat 超级表情(连发动画):resultId(string)/chainCount(number)。
            // ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageFaceSchema)。
            if let Some(rid) = result_id {
                data.insert("resultId".into(), Value::String(rid.clone()));
            }
            if let Some(cc) = chain_count {
                data.insert("chainCount".into(), Value::from(*cc));
            }
            // LLOneBot FaceType(表情子类型)。
            // ENDPOINT: LLOneBot/LLOneBot src/onebot11/types.ts (OB11MessageFace)。
            if let Some(st) = sub_type {
                data.insert("sub_type".into(), Value::from(*st));
            }
            WireSegment::new("face", data)
        }
        Segment::Reply { id, .. } => {
            // OneBot reply 按 message_id(合成整型)。
            let v = id.onebot_id.map(|o| Value::String(o.to_string())).unwrap_or(Value::String(id.seq.to_string()));
            let mut data = obj(vec![("id", v)]);
            // NapCat 在 reply 上额外接受 `seq`(message_seq);带真实 seq 时发它,使 NapCat 能精确
            // 定位引用。ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageReplySchema)。
            if id.seq != 0 {
                data.insert("seq".into(), Value::String(id.seq.to_string()));
            }
            WireSegment::new("reply", data)
        }
        Segment::Image { res, sub_type, hints } => {
            let file = media_file(res)?;
            let mut data = obj(vec![("file", Value::String(file))]);
            match sub_type {
                ImageSubType::Sticker => {
                    data.insert("subType".into(), Value::from(1));
                }
                ImageSubType::Flash => {
                    data.insert("type".into(), Value::String("flash".into()));
                }
                ImageSubType::Normal => {}
            }
            if let Some(s) = &res.summary {
                data.insert("summary".into(), Value::String(s.clone()));
            }
            apply_send_hints(&mut data, hints);
            WireSegment::new("image", data)
        }
        Segment::Record { res, magic, hints } => {
            let file = media_file(res)?;
            let mut data = obj(vec![("file", Value::String(file))]);
            if let Some(m) = magic {
                data.insert("magic".into(), Value::from(*m));
            }
            apply_send_hints(&mut data, hints);
            WireSegment::new("record", data)
        }
        Segment::Video { res, hints, thumb } => {
            let file = media_file(res)?;
            let mut data = obj(vec![("file", Value::String(file))]);
            // LLOneBot/go-cqhttp 在视频段上接受 `thumb` 封面。标准 OneBot v11 无此字段,但对支持
            // 的厂商发它无害,且使 thumb 与 decode 对称(LLOneBot thumb/cover/path)。
            // ENDPOINT: LLOneBot/LLOneBot src/onebot11/types.ts (OB11MessageVideo data `thumb`)。
            if let Some(t) = thumb {
                data.insert("thumb".into(), Value::String(encode_source(t)));
            }
            apply_send_hints(&mut data, hints);
            WireSegment::new("video", data)
        }
        Segment::File { id, name, size, .. } => {
            // OneBot 没有标准的内联文件发送段,但 LLOneBot 接受一个带 file/name/file_size(/path) 的
            // `file` 段。已知大小时发 file_size 以便接收方预分配;当我们持有的是 recv id(本地路径
            // 字符串)时,path 由它恢复。
            // ENDPOINT: LLOneBot/LLOneBot src/onebot11/types.ts (OB11MessageFile)。
            let mut data = obj(vec![("file", Value::String(id.clone())), ("name", Value::String(name.clone()))]);
            if *size != 0 {
                data.insert("file_size".into(), Value::String(size.to_string()));
            }
            WireSegment::new("file", data)
        }
        Segment::Forward(fwd) => encode_forward(fwd),
        Segment::MarketFace { emoji_id, key, package_id, summary, .. } => {
            let mut data = obj(vec![
                ("emoji_id", Value::String(emoji_id.clone())),
                ("key", Value::String(key.clone())),
                ("emoji_package_id", Value::from(*package_id)),
            ]);
            // decode 会读 `summary`;encode 时镜像它,使气泡替代文本 round-trip(LLOneBot/Lagrange
            // 把它作为回退说明文字显示)。
            if let Some(s) = summary {
                data.insert("summary".into(), Value::String(s.clone()));
            }
            WireSegment::new("mface", data)
        }
        Segment::LightApp { payload, .. } => {
            WireSegment::new("json", obj(vec![("data", Value::String(payload.clone()))]))
        }
        // 镜像 LightApp→json:`{"type":"xml","data":{"data":<payload>[,"service_id":<id>]}}`。
        Segment::Xml { service_id, payload } => {
            let mut data = obj(vec![("data", Value::String(payload.clone()))]);
            if let Some(id) = service_id {
                data.insert("service_id".into(), Value::from(*id));
            }
            WireSegment::new("xml", data)
        }
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§戳一戳)
        // Lagrange: https://github.com/LagrangeDev/Lagrange.Core/blob/master/Lagrange.OneBot/Message/Entity/PokeSegment.cs
        // Lagrange 的 PokeSegment 把 type/id/strength 作字符串携带;发 type+id,strength/name 存在时
        // 一并发。
        Segment::Poke { kind, id, strength, name } => {
            let mut data = obj(vec![("type", Value::String(kind.to_string())), ("id", Value::String(id.to_string()))]);
            if let Some(s) = strength {
                data.insert("strength".into(), Value::String(s.to_string()));
            }
            if let Some(n) = name {
                data.insert("name".into(), Value::String(n.clone()));
            }
            WireSegment::new("poke", data)
        }
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§推荐好友/§推荐群)
        // data {type:"qq"|"group", id}。Friend→qq、Group→group。
        Segment::Contact { kind, id } => {
            let ty = match kind {
                ContactKind::Friend => "qq",
                ContactKind::Group => "group",
            };
            WireSegment::new(
                "contact",
                obj(vec![("type", Value::String(ty.into())), ("id", Value::String(id.0.to_string()))]),
            )
        }
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§位置)
        // Lagrange: https://github.com/LagrangeDev/Lagrange.Core/blob/master/Lagrange.OneBot/Message/Entity/LocationSegment.cs
        // Lagrange 把 lat/lon 序列化为字符串;title/content 可选。
        Segment::Location { lat, lon, title, content } => {
            let mut data = obj(vec![("lat", Value::String(lat.to_string())), ("lon", Value::String(lon.to_string()))]);
            if let Some(t) = title {
                data.insert("title".into(), Value::String(t.clone()));
            }
            if let Some(c) = content {
                data.insert("content".into(), Value::String(c.clone()));
            }
            WireSegment::new("location", data)
        }
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§音乐分享/§音乐自定义分享)
        // Lagrange: https://github.com/LagrangeDev/Lagrange.Core/blob/master/Lagrange.OneBot/Message/Entity/MusicSegment.cs
        // Platform:{type:ty, id};Custom:{type:"custom", url, audio, title[, content, image]}。
        Segment::Music(share) => match share {
            MusicShare::Platform { ty, id } => WireSegment::new(
                "music",
                obj(vec![("type", Value::String(ty.clone())), ("id", Value::String(id.clone()))]),
            ),
            MusicShare::Custom { url, audio, title, content, image } => {
                let mut data = obj(vec![
                    ("type", Value::String("custom".into())),
                    ("url", Value::String(url.clone())),
                    ("audio", Value::String(audio.clone())),
                    ("title", Value::String(title.clone())),
                ]);
                if let Some(c) = content {
                    data.insert("content".into(), Value::String(c.clone()));
                }
                if let Some(i) = image {
                    data.insert("image".into(), Value::String(i.clone()));
                }
                WireSegment::new("music", data)
            }
        },
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§链接分享)
        // data {url, title[, content, image]}。
        Segment::Share { url, title, content, image } => {
            let mut data = obj(vec![("url", Value::String(url.clone())), ("title", Value::String(title.clone()))]);
            if let Some(c) = content {
                data.insert("content".into(), Value::String(c.clone()));
            }
            if let Some(i) = image {
                data.insert("image".into(), Value::String(i.clone()));
            }
            WireSegment::new("share", data)
        }
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§魔法表情/§窗口抖动/§匿名发消息)
        // 标准 v11 rps/dice 带空 data;持有 NapCat 的 `result` 时把它带出去(round-trip 已掷出的
        // 结果),否则保持空。
        Segment::Rps { result } => {
            let mut data = Map::new();
            if let Some(r) = result {
                data.insert("result".into(), Value::from(*r));
            }
            WireSegment::new("rps", data)
        }
        Segment::Dice { result } => {
            let mut data = Map::new();
            if let Some(r) = result {
                data.insert("result".into(), Value::from(*r));
            }
            WireSegment::new("dice", data)
        }
        Segment::Shake => WireSegment::new("shake", Map::new()),
        Segment::Anonymous { ignore } => {
            let mut data = Map::new();
            if let Some(ig) = ignore {
                data.insert("ignore".into(), Value::Bool(*ig));
            }
            WireSegment::new("anonymous", data)
        }
        // Lagrange 内联键盘 / markdown / 长消息段。
        // wire 形态已对照 Lagrange.OneBot/Message/Entity/*Segment.cs 核实(2026-06-04):
        //   keyboard → data {content:<KeyboardData JSON>};markdown → data {content:<string>};
        //   longmsg  → data {id:<res_id string>}。
        Segment::Keyboard { content } => WireSegment::new("keyboard", obj(vec![("content", content.clone())])),
        Segment::Markdown { content } => {
            WireSegment::new("markdown", obj(vec![("content", Value::String(content.clone()))]))
        }
        Segment::LongMsg { id } => WireSegment::new("longmsg", obj(vec![("id", Value::String(id.clone()))])),
        // LLOneBot 私聊闪传文件卡片。与 decode 对称:
        //   data {title?, file_set_id, scene_type:<number>}。
        // ENDPOINT: LLOneBot/LLOneBot src/onebot11/types.ts (OB11MessageFlashFile)。
        Segment::FlashFile { title, file_set_id, scene_type } => {
            let mut data = obj(vec![
                ("file_set_id", Value::String(file_set_id.clone())),
                ("scene_type", Value::from(*scene_type)),
            ]);
            if let Some(t) = title {
                data.insert("title".into(), Value::String(t.clone()));
            }
            WireSegment::new("flash_file", data)
        }
        // NapCat 私聊小程序卡片:data {data:<miniapp JSON string>}。
        // ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageMiniAppSchema)。
        Segment::MiniApp { payload } => {
            WireSegment::new("miniapp", obj(vec![("data", Value::String(payload.clone()))]))
        }
        // NapCat 私聊在线文件卡片:data {msgId, elementId, fileName, fileSize, isDir}。
        // ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageOnlineFileSchema)。
        Segment::OnlineFile { msg_id, element_id, file_name, file_size, is_dir } => WireSegment::new(
            "onlinefile",
            obj(vec![
                ("msgId", Value::String(msg_id.clone())),
                ("elementId", Value::String(element_id.clone())),
                ("fileName", Value::String(file_name.clone())),
                ("fileSize", Value::String(file_size.clone())),
                ("isDir", Value::Bool(*is_dir)),
            ]),
        ),
        // NapCat 私聊闪传卡片:data {fileSetId}。
        // ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageFlashTransferSchema)。
        Segment::FlashTransfer { file_set_id } => {
            WireSegment::new("flashtransfer", obj(vec![("fileSetId", Value::String(file_set_id.clone()))]))
        }
        // 逃生通道:来自 OneBot 时原样 round-trip 出去。
        Segment::Raw { protocol, kind, data } if *protocol == Protocol::OneBot11 => {
            WireSegment::new(kind.clone(), data.clone())
        }
        Segment::Raw { .. } => return None,
        // `#[non_exhaustive]`——我们没建模的任何未来变体都跳过。
        _ => return None,
    };
    Some(ws)
}

fn encode_forward(fwd: &Forward) -> WireSegment {
    match fwd {
        Forward::Ref { id, .. } => WireSegment::new("forward", obj(vec![("id", Value::String(id.clone()))])),
        Forward::Nodes { nodes, summary, prompt, news, source, .. } => {
            // 节点编码与 `send_*_forward` 动作共用 `encode_forward_node`(裸数组形态在此包成
            // `{nodes:[..]}` + 可选 preview 字段)。
            let node_vals: Vec<Value> = nodes.iter().map(encode_forward_node).collect();
            let mut data = obj(vec![("nodes", Value::Array(node_vals))]);
            // go-cqhttp/NapCat 自定义合并转发的预览字段。只发已设的,使普通(非自定义)转发保持裸
            // {nodes:[…]}。(`title` 没有独立的 gocq wire 字段——卡片标题行由 `source` 携带。)
            if let Some(s) = source {
                data.insert("source".into(), Value::String(s.clone()));
            }
            if let Some(s) = summary {
                data.insert("summary".into(), Value::String(s.clone()));
            }
            if let Some(p) = prompt {
                data.insert("prompt".into(), Value::String(p.clone()));
            }
            if !news.is_empty() {
                let arr: Vec<Value> =
                    news.iter().map(|line| Value::Object(obj(vec![("text", Value::String(line.clone()))]))).collect();
                data.insert("news".into(), Value::Array(arr));
            }
            WireSegment::new("forward", data)
        }
    }
}
