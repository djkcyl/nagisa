//! Notice 事件解码(`group_*`/`reaction`/`notify` 子类型 → 统一的 `Notice`)。
use super::*;

pub(super) fn decode_notice(ev: RawEventJson) -> Event {
    let nt = ev.notice_type.clone().unwrap_or_default();
    let group = Uin(ev.group_id.unwrap_or(0));
    let user = Uin(ev.user_id.unwrap_or(0));
    let operator = Uin(ev.operator_id.unwrap_or(0));
    let raw = event_raw_value(&ev);

    let notice = match nt.as_str() {
        "group_recall" => Notice::Recall {
            peer: Peer::group(group.0),
            id: MessageId {
                peer: Peer::group(group.0),
                seq: 0,
                onebot_id: ev.message_id,
            },
            sender: user,
            operator,
            suffix: ev.extra.get("tip").and_then(|v| v.as_str()).map(String::from),
        },
        "friend_recall" => Notice::Recall {
            peer: Peer::friend(user.0),
            id: MessageId {
                peer: Peer::friend(user.0),
                seq: 0,
                onebot_id: ev.message_id,
            },
            sender: user,
            operator: user,
            suffix: ev.extra.get("tip").and_then(|v| v.as_str()).map(String::from),
        },
        "group_increase" => Notice::MemberIncrease {
            group,
            user,
            operator: ev.operator_id.map(Uin),
            invitor: if ev.sub_type.as_deref() == Some("invite") {
                ev.operator_id.map(Uin)
            } else {
                None
            },
        },
        "group_decrease" => Notice::MemberDecrease {
            group,
            user,
            operator: ev.operator_id.map(Uin),
            // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/notice.md (§群成员减少)
            // sub_type: `leave`(主动退群) / `kick`(成员被踢) / `kick_me`(登录号被踢)。
            // NapCat 扩展 `disband`(群解散时逐成员下发):
            //   ENDPOINT NapCat OB11GroupDecreaseEvent.ts。
            reason: match ev.sub_type.as_deref() {
                Some("leave") => MemberDecreaseReason::Leave,
                Some("kick") => MemberDecreaseReason::Kick,
                Some("kick_me") => MemberDecreaseReason::KickMe,
                Some("disband") => MemberDecreaseReason::Disband,
                _ => MemberDecreaseReason::Unknown,
            },
        },
        "group_admin" => Notice::AdminChange {
            group,
            user,
            operator: None,
            is_set: ev.sub_type.as_deref() == Some("set"),
        },
        "group_ban" => {
            // OneBot 重载:user_id == 0 表示全群禁言。
            if ev.user_id.unwrap_or(0) == 0 {
                Notice::WholeMute {
                    group,
                    operator,
                    // 看 sub_type ban / lift_ban,或 duration 的正负。
                    is_mute: ev.sub_type.as_deref() != Some("lift_ban")
                        && ev.duration.unwrap_or(0) != 0,
                }
            } else {
                Notice::Mute {
                    group,
                    user,
                    operator,
                    duration: ev.duration.unwrap_or(0) as i32,
                }
            }
        }
        "group_name_change" => Notice::GroupNameChange {
            group,
            new_name: ev.name.clone().unwrap_or_default(),
            operator,
        },
        "group_upload" => {
            let f = ev.file.clone().unwrap_or_default();
            Notice::GroupFileUpload {
                group,
                user,
                file: FileMeta {
                    id: f.id.unwrap_or_default(),
                    name: f.name.unwrap_or_default(),
                    size: f.size.unwrap_or(0),
                    hash: None,
                    busid: f.busid,
                    // group_upload notice 的 file 只带 id/name/size/busid。
                    uploader: None,
                    upload_time: None,
                    dead_time: None,
                    download_times: None,
                    parent_folder_id: None,
                },
            }
        }
        "reaction" => {
            let face_id = ev.code.clone().unwrap_or_default();
            Notice::Reaction {
                group,
                user: Uin(ev.operator_id.unwrap_or(ev.user_id.unwrap_or(0))),
                seq: ev.message_id.map(|m| m as i64).unwrap_or(0),
                likes: vec![EmojiLike { face_id: face_id.clone(), count: ev.count }],
                face_id,
                kind: ReactionKind::Face,
                is_add: ev.sub_type.as_deref() != Some("remove"),
                // Lagrange 原始 sub_type(`add`/`remove`)：typed 保留，is_add 由其派生。
                sub_type: ev.sub_type.clone(),
                count: ev.count,
            }
        }
        "essence" => Notice::EssenceChange {
            group,
            seq: ev.message_id.map(|m| m as i64).unwrap_or(0),
            // Lagrange `essence` 携带原作者 sender_id；缺省为 None。
            sender: ev.sender_id.map(Uin),
            operator: ev.operator_id.map(Uin),
            is_set: ev.sub_type.as_deref() != Some("delete"),
        },
        "offline_file" => {
            let f = ev.file.clone().unwrap_or_default();
            Notice::FriendFileUpload {
                user,
                file: FileMeta {
                    id: f.id.unwrap_or_default(),
                    name: f.name.unwrap_or_default(),
                    size: f.size.unwrap_or(0),
                    hash: f.hash,
                    busid: f.busid,
                    // offline_file notice 的 file 只带 id/name/size/hash/busid。
                    uploader: None,
                    upload_time: None,
                    dead_time: None,
                    download_times: None,
                    parent_folder_id: None,
                },
                is_self: false,
            }
        }
        "bot_offline" => Notice::BotOffline {
            // Lagrange 把原因序列化在 `message`/`reason` JSON 键里。
            reason: ev
                .reason
                .clone()
                .or_else(|| match &ev.message {
                    Some(WireMessage::Cq(s)) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                })
                .or_else(|| ev.tag.clone())
                .unwrap_or_default(),
            // `tag` 与 reason 不同维度：typed 单独保留（区分离线种类）。
            tag: ev.tag.clone(),
        },
        "friend_add" => Notice::FriendAdd { user },
        // ENDPOINT: LLOneBot src/onebot11/event/notice/（群解散 notice）。
        // 群主解散整个群时下发一次 group_dismiss(区别于逐成员的 group_decrease/disband)。
        "group_dismiss" => Notice::GroupDismiss {
            group,
            // 解散者通常为群主;wire 可能落在 operator_id,缺省回退 user_id。
            operator: Uin(ev.operator_id.or(ev.user_id).unwrap_or(0)),
        },
        // ENDPOINT: NapCat（在线/临时文件 notice）。
        // online_file_send / online_file_receive:在线文件收发提示,群场景带 group_id。
        "online_file_send" | "online_file_receive" => Notice::OnlineFile {
            direction: if nt == "online_file_receive" {
                OnlineFileDirection::Receive
            } else {
                OnlineFileDirection::Send
            },
            user,
            group: ev.group_id.map(Uin),
        },
        // ENDPOINT: LLOneBot src/onebot11/event/notice/OB11FlashTransferNoticeEvent.ts。
        // flash_file:闪传进度 notice,sub_type=downloading/downloaded/uploading/uploaded。
        // 完整闪传载荷保留在事件 raw(下游可读 file_set_id/title 等)。
        "flash_file" => Notice::FlashFile {
            phase: match ev.sub_type.as_deref() {
                Some("downloading") => FlashFilePhase::Downloading,
                Some("downloaded") => FlashFilePhase::Downloaded,
                Some("uploading") => FlashFilePhase::Uploading,
                Some("uploaded") => FlashFilePhase::Uploaded,
                _ => FlashFilePhase::Unknown,
            },
            user,
            group: ev.group_id.map(Uin),
        },
        // ENDPOINT: NapCat packages/napcat-onebot/event/notice/OB11MsgEmojiLikeEvent.ts
        //   (https://github.com/NapNeko/NapCatQQ);并对照 LLOneBot
        //   src/onebot11/event/notice/OB11MsgEmojiLikeEvent.ts。
        // 表情回应:data 形如 {user_id,group_id,message_id,is_add,likes:[{emoji_id,count}]}。
        // 一次 notice 可携带多个 emoji:全部上浮到 `likes`(忠实,不破坏单事件分发);
        // face_id/count 同步取首元素以兼容只读单 emoji 的下游。
        "group_msg_emoji_like" => {
            let likes: Vec<EmojiLike> = ev
                .extra
                .get("likes")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .map(|like| EmojiLike {
                            face_id: like
                                .get("emoji_id")
                                .and_then(value_as_string)
                                .unwrap_or_default(),
                            count: like.get("count").and_then(Value::as_i64),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let first = likes.first();
            let face_id = first.map(|l| l.face_id.clone()).unwrap_or_default();
            let count = first.and_then(|l| l.count);
            Notice::Reaction {
                group,
                user,
                // 无独立 seq 字段:从 message_id 派生(不可派生则 0)。
                seq: ev.message_id.map(|m| m as i64).unwrap_or(0),
                face_id,
                kind: ReactionKind::Emoji,
                // NapCat/LLOneBot 携带 is_add;缺省按新增(true)。
                is_add: ev.extra.get("is_add").and_then(|v| v.as_bool()).unwrap_or(true),
                // group_msg_emoji_like 无独立 sub_type 字段（is_add 即语义）。
                sub_type: None,
                count,
                likes,
            }
        }
        // ENDPOINT: NapCat packages/napcat-onebot/event/notice/OB11GroupCardEvent.ts
        //   (https://github.com/NapNeko/NapCatQQ);并对照 LLOneBot
        //   src/onebot11/event/notice/OB11GroupCardEvent.ts。
        // 群名片变更:data 形如 {group_id,user_id,card_new,card_old}。
        "group_card" => Notice::GroupCardChange {
            group,
            user,
            old_card: ev.extra.get("card_old").and_then(value_as_string).unwrap_or_default(),
            new_card: ev.extra.get("card_new").and_then(value_as_string).unwrap_or_default(),
        },
        // Lagrange `bot_online`：上线 notice，`reason` 携带上线原因文案。
        "bot_online" => return Event::Meta(Meta::BotOnline { reason: ev.reason.clone() }),
        "notify" => return decode_notify(ev),
        _ => Notice::Other { protocol: PROTO, kind: nt, raw },
    };
    Event::Notice(notice)
}

/// `notify` 子类型:`poke`(戳一戳)、`honor`、`lucky_king` 等。
fn decode_notify(ev: RawEventJson) -> Event {
    let display = NudgeDisplay {
        action: ev.action.clone().unwrap_or_default(),
        suffix: ev.suffix.clone().unwrap_or_default(),
        action_img_url: ev.action_img_url.clone().filter(|s| !s.is_empty()),
    };
    match ev.sub_type.as_deref() {
        Some("poke") => {
            // 群戳一戳带 group_id;好友戳一戳带 sender_id、无 group。
            if ev.group_id.is_some() {
                Event::Notice(Notice::GroupNudge {
                    group: Uin(ev.group_id.unwrap_or(0)),
                    sender: Uin(ev.user_id.unwrap_or(0)),
                    receiver: Uin(ev.target_id.unwrap_or(0)),
                    display,
                })
            } else {
                let sender = Uin(ev.sender_id.or(ev.user_id).unwrap_or(0));
                let self_id = Uin(ev.self_id);
                let target = Uin(ev.target_id.unwrap_or(0));
                Event::Notice(Notice::FriendNudge {
                    user: if sender == self_id { target } else { sender },
                    is_self_send: sender == self_id,
                    is_self_receive: target == self_id,
                    display,
                })
            }
        }
        // ENDPOINT: NapCat packages/napcat-onebot/event/notice/OB11GroupNameEvent.ts
        //   (https://github.com/NapNeko/NapCatQQ).
        // notify + sub_type=group_name:群名变更,新名在 `name_new`,操作者为 user_id。
        Some("group_name") => Event::Notice(Notice::GroupNameChange {
            group: Uin(ev.group_id.unwrap_or(0)),
            new_name: ev.extra.get("name_new").and_then(value_as_string).unwrap_or_default(),
            operator: Uin(ev.user_id.unwrap_or(0)),
        }),
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/notice.md (§群成员荣誉变更)
        // notify + sub_type=honor:honor_type talkative/performer/emotion=龙王/群聊之火/快乐源泉。
        Some("honor") => Event::Notice(Notice::Honor {
            group: Uin(ev.group_id.unwrap_or(0)),
            user: Uin(ev.user_id.unwrap_or(0)),
            // wire.rs 已解析 `honor_type` 字段,直接复用。
            honor: match ev.honor_type.as_deref() {
                Some("talkative") => HonorKind::Talkative,
                Some("performer") => HonorKind::Performer,
                Some("emotion") => HonorKind::Emotion,
                _ => HonorKind::Unknown,
            },
        }),
        // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/notice.md (§群红包运气王)
        // notify + sub_type=lucky_king: group_id / user_id(发红包者) / target_id(运气王)。
        Some("lucky_king") => Event::Notice(Notice::LuckyKing {
            group: Uin(ev.group_id.unwrap_or(0)),
            user: Uin(ev.user_id.unwrap_or(0)),
            target: Uin(ev.target_id.unwrap_or(0)),
        }),
        // ENDPOINT: NapCat OB11GroupTitleEvent.ts; cross-checked LLOneBot。
        // notify + sub_type=title:群头衔变更,新头衔在 `title`,成员为 user_id。
        Some("title") => Event::Notice(Notice::GroupTitleChange {
            group: Uin(ev.group_id.unwrap_or(0)),
            user: Uin(ev.user_id.unwrap_or(0)),
            title: ev.extra.get("title").and_then(value_as_string).unwrap_or_default(),
        }),
        // ENDPOINT: NapCat OB11InputStatusEvent.ts; cross-checked LLOneBot。
        // notify + sub_type=input_status:对端正在输入。群场景带 group_id;
        // status_text 为展示文案,event_type 为原始状态码。
        Some("input_status") => Event::Notice(Notice::InputStatus {
            user: Uin(ev.user_id.unwrap_or(0)),
            group: ev.group_id.map(Uin),
            status_text: ev.extra.get("status_text").and_then(value_as_string).unwrap_or_default(),
            event_type: ev.extra.get("event_type").and_then(Value::as_i64).unwrap_or(0),
        }),
        // ENDPOINT: NapCat OB11ProfileLikeEvent.ts; cross-checked LLOneBot。
        // notify + sub_type=profile_like:资料卡点赞。点赞者为 operator_id,
        // 次数 times,昵称 operator_nick。
        Some("profile_like") => Event::Notice(Notice::ProfileLike {
            operator: Uin(ev.operator_id.or(ev.user_id).unwrap_or(0)),
            operator_nick: ev.extra.get("operator_nick").and_then(value_as_string).unwrap_or_default(),
            times: ev.extra.get("times").and_then(Value::as_i64).unwrap_or(0),
        }),
        // ENDPOINT: NapCat（gray tip notice）; cross-checked LLOneBot。
        // notify + sub_type=gray_tip:灰字系统提示。子类繁多,统一以 content 透出文本,
        // 结构细节保留在事件 raw。
        Some("gray_tip") => Event::Notice(Notice::GrayTip {
            group: ev.group_id.map(Uin),
            user: ev.user_id.map(Uin),
            content: ev
                .extra
                .get("content")
                .or_else(|| ev.extra.get("tip"))
                .and_then(value_as_string)
                .unwrap_or_default(),
        }),
        // ENDPOINT: LLOneBot（poke_recall notice）。
        // notify + sub_type=poke_recall:戳一戳被撤回。群场景带 group_id。
        Some("poke_recall") => Event::Notice(Notice::PokeRecall {
            group: ev.group_id.map(Uin),
            user: Uin(ev.user_id.unwrap_or(0)),
        }),
        _ => raw_event(&ev, "notify"),
    }
}
