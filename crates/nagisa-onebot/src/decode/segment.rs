//! wire 段 / CQ 字符串解码成统一的 `Segment`(绝不丢弃、绝不 panic)。
use super::*;

/// 解码一个 wire 段数组。未知段降级为 `Segment::Raw`。`peer` 是这些段所属的会话,会被串进
/// 嵌套的 `reply` 段,使其恢复真实对端(对照 Milky 的 `decode_segments`)。
pub fn decode_segments(segs: &[WireSegment], peer: Peer) -> Vec<Segment> {
    segs.iter().map(|s| decode_segment(s, peer)).collect()
}

/// 把动作响应的 `message`/`content` 字段(OneBot 数组格式,或 CQ 字符串)解码成统一的
/// `Message`。供 `get_forward_msg` 的节点内容与 `get_essence_msg_list` 的消息正文使用。`peer`
/// 是这段内容所属的会话(串进嵌套的 `reply` 段)。绝不 panic:解不了的都降级为空消息。
pub fn decode_message_value(v: &Value, peer: Peer) -> Message {
    match v {
        Value::Array(_) => serde_json::from_value::<Vec<WireSegment>>(v.clone())
            .map(|segs| decode_segments(&segs, peer))
            .unwrap_or_default(),
        Value::String(s) => decode_cq_string(s, peer),
        _ => Vec::new(),
    }
}

fn raw_segment(seg: &WireSegment) -> Segment {
    Segment::Raw {
        protocol: PROTO,
        kind: seg.kind.clone(),
        data: seg.data.clone(),
    }
}

fn decode_segment(seg: &WireSegment, peer: Peer) -> Segment {
    match seg.kind.as_str() {
        "text" => Segment::Text(seg.str_field("text").unwrap_or_default()),
        "at" => match seg.str_field("qq").as_deref() {
            Some("all") | Some("0") => Segment::MentionAll,
            Some(s) => match s.parse::<i64>() {
                Ok(uin) => Segment::Mention {
                    user: Uin(uin),
                    // name 原样保留（不在此处归一化 @）：是否带前导 @ 取决于厂商（Lagrange 原样
                    // 透传 QQ 的 "@昵称"，NapCat 不发 name，LLOneBot 已 strip）,故归一化交给知道
                    // 厂商的适配器层按 `Vendor` 条件处理（见 `adapter::normalize_at_names`）。
                    name: seg.str_field("name"),
                },
                Err(_) => raw_segment(seg),
            },
            None => raw_segment(seg),
        },
        // NapCat 超级表情:resultId(string)/chainCount(number);LLOneBot:sub_type(FaceType)。
        // 标准 v11 / 其他厂商没有 → None(解码保持不会失败)。
        "face" => Segment::Face {
            id: seg.str_field("id").unwrap_or_default(),
            large: seg.bool_field("large").unwrap_or(false),
            result_id: seg.str_field("resultId"),
            chain_count: seg.i64_field("chainCount").map(|v| v as i32),
            sub_type: seg.i64_field("sub_type").map(|v| v as i32),
        },
        "image" => Segment::Image {
            res: media_from_recv(seg),
            // 闪照优先:OneBot/go-cqhttp 用 data.type = "flash";否则按 subType 区分大表情。
            sub_type: if seg.str_field("type").as_deref() == Some("flash") {
                ImageSubType::Flash
            } else {
                match seg.i64_field("subType") {
                    Some(0) | None => ImageSubType::Normal,
                    Some(_) => ImageSubType::Sticker,
                }
            },
            hints: hints_from_recv(seg),
        },
        "record" => decode_record(seg),
        // LLOneBot/go-cqhttp 带 `thumb`/`cover` URL(视频封面)+ 本地 `path`。把 thumb typed 浮现
        // 出来(以前恒为 None);`path` 保留在 media recv.raw 里,由 `media_from_recv` 作 source 回退。
        // ENDPOINT: LLOneBot/LLOneBot src/onebot11/types.ts (OB11MessageVideo)。
        "video" => Segment::Video {
            res: media_from_recv(seg),
            hints: hints_from_recv(seg),
            thumb: seg
                .str_field("thumb")
                .or_else(|| seg.str_field("cover"))
                .filter(|s| !s.is_empty())
                .map(ResourceSource::url),
        },
        "reply" => decode_reply(seg, peer),
        "forward" => Segment::Forward(Forward::Ref {
            id: seg.str_field("id").unwrap_or_default(),
            title: None,
            preview: Vec::new(),
            summary: None,
        }),
        "file" => Segment::File {
            // Lagrange 发 file_id / file_name / file_hash;回退到老/其他实现用的旧字段名,最后
            // 回退到 LLOneBot 的本地 `path`,使其能作为 `file` round-trip 出去。
            id: seg
                .str_field("file_id")
                .or_else(|| seg.str_field("file"))
                .or_else(|| seg.str_field("id"))
                .or_else(|| seg.str_field("path"))
                .unwrap_or_default(),
            name: seg.str_field("file_name").or_else(|| seg.str_field("name")).unwrap_or_default(),
            size: seg.i64_field("file_size").or_else(|| seg.i64_field("size")).unwrap_or(0) as u64,
            hash: seg.str_field("file_hash"),
            url: seg.str_field("url"),
        },
        // `{"type":"json","data":{"data":"{...}"}}` → LightApp(与 encode 对称)。
        "json" => Segment::LightApp {
            app_name: None,
            payload: seg.str_field("data").unwrap_or_default(),
        },
        // `{"type":"xml","data":{"data":"<xml/>"[,"service_id":35]}}` → Xml(与 encode 对称)。
        "xml" => Segment::Xml {
            service_id: seg.i64_field("service_id").map(|v| v as i32),
            payload: seg.str_field("data").unwrap_or_default(),
        },
        // Lagrange 商城表情段。
        "mface" => Segment::MarketFace {
            package_id: seg.i64_field("emoji_package_id").unwrap_or(0) as i32,
            emoji_id: seg.str_field("emoji_id").unwrap_or_default(),
            key: seg.str_field("key").unwrap_or_default(),
            summary: seg.str_field("summary"),
            url: seg.str_field("url"),
        },
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§戳一戳)
        // Lagrange: https://github.com/LagrangeDev/Lagrange.Core/blob/master/Lagrange.OneBot/Message/Entity/PokeSegment.cs
        // data {type,id[,strength,name]}——Lagrange 把这些序列化为字符串。
        "poke" => Segment::Poke {
            kind: seg.i64_field("type").unwrap_or(0) as i32,
            id: seg.i64_field("id").unwrap_or(-1) as i32,
            strength: seg.i64_field("strength").map(|v| v as i32),
            name: seg.str_field("name"),
        },
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§推荐好友/§推荐群)
        // data {type:"qq"|"group", id}——"qq"→Friend、"group"→Group;未知 type 保持 Raw。
        "contact" => match seg.str_field("type").as_deref() {
            Some("qq") => Segment::Contact {
                kind: ContactKind::Friend,
                id: Uin(seg.i64_field("id").unwrap_or(0)),
            },
            Some("group") => Segment::Contact {
                kind: ContactKind::Group,
                id: Uin(seg.i64_field("id").unwrap_or(0)),
            },
            _ => raw_segment(seg),
        },
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§位置)
        // Lagrange: https://github.com/LagrangeDev/Lagrange.Core/blob/master/Lagrange.OneBot/Message/Entity/LocationSegment.cs
        // data {lat,lon[,title,content]}——Lagrange 把 lat/lon 序列化为字符串。
        "location" => Segment::Location {
            lat: seg.str_field("lat").and_then(|s| s.parse().ok()).unwrap_or(0.0),
            lon: seg.str_field("lon").and_then(|s| s.parse().ok()).unwrap_or(0.0),
            title: seg.str_field("title").filter(|s| !s.is_empty()),
            content: seg.str_field("content").filter(|s| !s.is_empty()),
        },
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§链接分享)
        // data {url, title[, content, image]}。url/title 必填;content/image 可选。
        "share" => Segment::Share {
            url: seg.str_field("url").unwrap_or_default(),
            title: seg.str_field("title").unwrap_or_default(),
            content: seg.str_field("content").filter(|s| !s.is_empty()),
            image: seg.str_field("image").filter(|s| !s.is_empty()),
        },
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§魔法表情)
        // 标准 v11 的 rps / dice 带空 data;NapCat 加了数字 `result`(已掷出的结果),我们保留
        // (没有 → None)。
        "rps" => Segment::Rps { result: seg.i64_field("result").map(|v| v as i32) },
        "dice" => Segment::Dice { result: seg.i64_field("result").map(|v| v as i32) },
        // shake / anonymous 按规范是只发的;入站也兼容(绝不丢)。
        "shake" => Segment::Shake,
        "anonymous" => Segment::Anonymous { ignore: seg.bool_field("ignore") },
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§合并转发节点)
        // 内联自定义节点:data {user_id, nickname, content[, time]} → 单节点 Forward。
        // (与 encode 侧对称,encode 已发 type:"node"。)
        "node" => {
            // go-cqhttp 自定义节点用 `uin`/`name`;标准用 `user_id`/`nickname`。
            let user = Uin(seg.i64_field("user_id").or_else(|| seg.i64_field("uin")).unwrap_or(0));
            let name = seg
                .str_field("nickname")
                .or_else(|| seg.str_field("name"))
                .unwrap_or_default();
            let content = seg
                .data
                .get("content")
                .map(|v| crate::decode::decode_message_value(v, peer))
                .unwrap_or_default();
            let time = seg.i64_field("time");
            Segment::Forward(Forward::Nodes {
                nodes: vec![ForwardNode { user, name, content, time }],
                // go-cqhttp 自定义合并转发的预览字段(在节点顶层)。
                title: None,
                summary: seg.str_field("summary"),
                prompt: seg.str_field("prompt"),
                news: news_lines(seg.data.get("news")),
                source: seg.str_field("source"),
            })
        }
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§音乐分享/§音乐自定义分享)
        // Lagrange: https://github.com/LagrangeDev/Lagrange.Core/blob/master/Lagrange.OneBot/Message/Entity/MusicSegment.cs
        // type=="custom" → Custom {url,audio,title[,content,image]};否则 Platform {ty:type, id}。
        "music" => match seg.str_field("type").as_deref() {
            Some("custom") => Segment::Music(MusicShare::Custom {
                url: seg.str_field("url").unwrap_or_default(),
                audio: seg.str_field("audio").unwrap_or_default(),
                title: seg.str_field("title").unwrap_or_default(),
                content: seg.str_field("content").filter(|s| !s.is_empty()),
                image: seg.str_field("image").filter(|s| !s.is_empty()),
            }),
            _ => Segment::Music(MusicShare::Platform {
                ty: seg.str_field("type").unwrap_or_default(),
                id: seg.str_field("id").unwrap_or_default(),
            }),
        },
        // Lagrange 内联键盘 / markdown / 长消息段(2026-06-04 已对照
        // Lagrange.OneBot/Message/Entity/*Segment.cs 核实)。
        // keyboard.content 是 JSON 对象(KeyboardData)——原样读取。
        "keyboard" => Segment::Keyboard {
            content: seg.data.get("content").cloned().unwrap_or(Value::Null),
        },
        "markdown" => Segment::Markdown {
            content: seg.str_field("content").unwrap_or_default(),
        },
        "longmsg" => Segment::LongMsg {
            id: seg.str_field("id").unwrap_or_default(),
        },
        // LLOneBot 私聊闪传文件卡片。与 encode 对称:
        //   data {title?, file_set_id, scene_type:<number>}。`title` 在 wire 上可能缺失
        //   (标题属性并非总能提取)→ None。
        // ENDPOINT: LLOneBot/LLOneBot src/onebot11/transform/message/incoming.ts。
        "flash_file" => Segment::FlashFile {
            title: seg.str_field("title"),
            file_set_id: seg.str_field("file_set_id").unwrap_or_default(),
            scene_type: seg.i64_field("scene_type").unwrap_or(0) as i32,
        },
        // NapCat 私聊小程序卡片:data {data:<miniapp JSON string>}。
        // ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageMiniAppSchema)。
        "miniapp" => Segment::MiniApp {
            payload: seg.str_field("data").unwrap_or_default(),
        },
        // NapCat 私聊在线文件卡片:data {msgId, elementId, fileName, fileSize, isDir}。
        // ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageOnlineFileSchema)。
        "onlinefile" => Segment::OnlineFile {
            msg_id: seg.str_field("msgId").unwrap_or_default(),
            element_id: seg.str_field("elementId").unwrap_or_default(),
            file_name: seg.str_field("fileName").unwrap_or_default(),
            file_size: seg.str_field("fileSize").unwrap_or_default(),
            is_dir: seg.bool_field("isDir").unwrap_or(false),
        },
        // NapCat 私聊闪传卡片:data {fileSetId}。
        // ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageFlashTransferSchema)。
        "flashtransfer" => Segment::FlashTransfer {
            file_set_id: seg.str_field("fileSetId").unwrap_or_default(),
        },
        _ => raw_segment(seg),
    }
}

/// Lagrange 的 bug:`video` 可能以 `record` 段到达。这里没法真正区分,故尊重声明的 `record`
/// 类型——在意此 bug 的调用方可检查 `Segment::Record` 的 media url。一个 url 明显像视频的
/// `record` 仍当 record 处理(解码器不能猜错);常见情况就是音频。
fn decode_record(seg: &WireSegment) -> Segment {
    Segment::Record {
        res: media_from_recv(seg),
        // OFFICIAL: segment.md §语音 — magic=1 表示变声。
        magic: seg.i64_field("magic").map(|v| v as i32),
        hints: hints_from_recv(seg),
    }
}

/// 接收侧 hints 恒为默认（cache/proxy/timeout 仅发送侧有意义）。但 decode 仍读取，
/// 以便 re-send 时 round-trip（never drop）。
fn hints_from_recv(seg: &WireSegment) -> nagisa_types::segment::MediaSendHints {
    nagisa_types::segment::MediaSendHints {
        cache: seg.bool_field("cache"),
        proxy: seg.bool_field("proxy"),
        timeout: seg.i64_field("timeout").map(|v| v as i32),
    }
}

fn decode_reply(seg: &WireSegment, peer: Peer) -> Segment {
    let onebot_id = seg.i64_field("id").map(|v| v as i32);
    // `peer` 是外层消息事件真实的会话对端,由 `decode_message` 串进来(与 Milky 对齐)。OneBot 的
    // `reply` wire 段只带被回复的 `id`,故 seq 保持 0;对端从上下文恢复,而非留作 `friend(0)` 兜底。
    let id = MessageId {
        peer,
        seq: 0,
        onebot_id,
    };
    Segment::Reply {
        id,
        sender: seg.i64_field("user_id").map(Uin),
        time: None,
        quoted: Vec::new(),
    }
}

/// 把 go-cqhttp 自定义转发的 `news` 字段解析成预览行。wire 形态是 `{"text": "..."}` 对象数组
/// (每个是一行预览);也兼容裸字符串。缺失/异常形态 → 空(绝不 panic)。
fn news_lines(v: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = v else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| match item {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => o.get("text").and_then(Value::as_str).map(str::to_string),
            _ => None,
        })
        .collect()
}

fn media_from_recv(seg: &WireSegment) -> Media {
    let recv = ResourceRef {
        // LLOneBot 在 record/video/file 段上加了本地 `path`;在 wire 的 `file`/`filename` 之后回退到
        // 它作为 recv id(一个可再发的本地路径)。完整 data map(含 `path`)原样保留在 `raw` 里。
        id: seg
            .str_field("file")
            .or_else(|| seg.str_field("filename"))
            .or_else(|| seg.str_field("path")),
        url: seg.str_field("url"),
        raw: Value::Object(seg.data.clone()),
    };
    let mut media = Media::from_recv(recv);
    media.summary = seg.str_field("summary");
    media
}

/// 把 CQ 字符串(`message_format: "string"`)解析成段。这是一个尽力而为的解析器:文本 +
/// `[CQ:type,k=v,...]` 记号,带标准的 `&amp;`/`&#91;`/`&#93;`/`&#44;` 实体转义。
pub fn decode_cq_string(s: &str, peer: Peer) -> Vec<Segment> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut text_start = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' && s[i..].starts_with("[CQ:") {
            if i > text_start {
                let t = cq_unescape(&s[text_start..i]);
                if !t.is_empty() {
                    out.push(Segment::Text(t));
                }
            }
            if let Some(end) = s[i..].find(']') {
                let body = &s[i + 4..i + end];
                out.push(parse_cq_segment(body, peer));
                i += end + 1;
                text_start = i;
                continue;
            }
        }
        i += 1;
    }
    if text_start < bytes.len() {
        let t = cq_unescape(&s[text_start..]);
        if !t.is_empty() {
            out.push(Segment::Text(t));
        }
    }
    out
}

fn parse_cq_segment(body: &str, peer: Peer) -> Segment {
    let mut parts = body.split(',');
    let kind = parts.next().unwrap_or("").to_string();
    let mut data = Map::new();
    for kv in parts {
        if let Some((k, v)) = kv.split_once('=') {
            data.insert(k.to_string(), Value::String(cq_unescape(v)));
        }
    }
    decode_segment(&WireSegment::new(kind, data), peer)
}

fn cq_unescape(s: &str) -> String {
    s.replace("&#44;", ",")
        .replace("&#91;", "[")
        .replace("&#93;", "]")
        .replace("&amp;", "&")
}
