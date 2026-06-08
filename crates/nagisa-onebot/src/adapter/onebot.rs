//! [`OneBotAdapter`] 的 `OneBotActions` trait 实现——整个出站动作面集中在一块,
//! 按 OneBot v11 标准动作 + 厂商扩展(NapCat / LLOneBot / Lagrange)分段组织。
//!
//! 每个方法带一条 `// OFFICIAL:`(onebot-11 规范)或 `// ENDPOINT:`(厂商源文件)溯源注释,
//! 标出上游 URL 与它映射的 wire 参数/响应形态;跨厂商命名分歧用 `call_alias`(在 `Unsupported`
//! 上改用备用动作名重试)或一次同发两种参数拼写来弥合。
use super::*;

#[async_trait]
impl OneBotActions for OneBotAdapter {
    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // get_status:响应 {online, good[, ...]}。
    async fn get_status(&self) -> Result<ImplStatus> {
        let data = self.call("get_status", json!({})).await?;
        // LLOneBot/Lagrange/go-cqhttp 在 `stat` 子对象内携带收发统计；
        // 无该块的实现 → None（其余统计字段仍保留于 raw，绝不丢弃）。
        let stat = impl_stat_from(&data);
        Ok(ImplStatus {
            online: data.get("online").and_then(|v| v.as_bool()).unwrap_or(false),
            good: data.get("good").and_then(|v| v.as_bool()).unwrap_or(false),
            stat,
            raw: data,
        })
    }

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // set_restart:参数 `delay`(ms)。异步重启(status `async`)。
    async fn set_restart(&self, delay_ms: u32) -> Result<()> {
        self.call("set_restart", json!({ "delay": delay_ms })).await?;
        Ok(())
    }

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // clean_cache:无参数。
    async fn clean_cache(&self) -> Result<()> {
        self.call("clean_cache", json!({})).await?;
        Ok(())
    }

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // can_send_image:响应 {yes:boolean}。
    async fn can_send_image(&self) -> Result<bool> {
        let data = self.call("can_send_image", json!({})).await?;
        Ok(data.get("yes").and_then(|v| v.as_bool()).unwrap_or(false))
    }

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // can_send_record:响应 {yes:boolean}。
    async fn can_send_record(&self) -> Result<bool> {
        let data = self.call("can_send_record", json!({})).await?;
        Ok(data.get("yes").and_then(|v| v.as_bool()).unwrap_or(false))
    }

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // get_group_honor_info:参数 {group_id, type}。type:talkative/performer/emotion/all
    // (Unknown→"all")。返回 typed 的 HonorList;raw 保留。
    async fn get_group_honor_info(&self, group: Uin, kind: HonorKind) -> Result<HonorList> {
        let ty = match kind {
            HonorKind::Talkative => "talkative",
            HonorKind::Performer => "performer",
            HonorKind::Emotion => "emotion",
            HonorKind::Unknown => "all",
        };
        let data = self.call("get_group_honor_info", json!({ "group_id": group.0, "type": ty })).await?;
        fn parse_list(data: &Value, key: &str) -> Vec<HonorMember> {
            data.get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(honor_member_from)
                .collect()
        }
        let current_talkative = data.get("current_talkative").map(honor_member_from);
        let talkative_list = parse_list(&data, "talkative_list");
        let performer_list = parse_list(&data, "performer_list");
        let legend_list = parse_list(&data, "legend_list");
        let strong_newbie_list = parse_list(&data, "strong_newbie_list");
        let emotion_list = parse_list(&data, "emotion_list");
        Ok(HonorList {
            group,
            current_talkative,
            talkative_list,
            performer_list,
            legend_list,
            strong_newbie_list,
            emotion_list,
            raw: data,
        })
    }

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // get_record:参数 {file, out_format}。响应 {file}(转换后路径)。
    async fn get_record(&self, file: &str, out_format: &str) -> Result<String> {
        let data = self
            .call("get_record", json!({ "file": file, "out_format": out_format }))
            .await?;
        Ok(data_str(&data, "file").unwrap_or_default())
    }

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // get_image:参数 {file}。响应 {file}(下载后路径)。
    async fn get_image(&self, file: &str) -> Result<String> {
        let data = self.call("get_image", json!({ "file": file })).await?;
        Ok(data_str(&data, "file").unwrap_or_default())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/router.ts (set_friend_remark)
    //   (https://github.com/NapNeko/NapCatQQ);亦见 LLOneBot `set_friend_remark`。
    // 参数 {user_id, remark}。
    async fn set_friend_remark(&self, user: Uin, remark: &str) -> Result<()> {
        self.call(
            "set_friend_remark",
            json!({ "user_id": user.0, "remark": remark }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: NapCat set_msg_emoji_like {message_id, emoji_id, set}。逐消息的表情回应,
    // 私聊里也能用(不像 ActionInvoker::send_reaction 发的 set_group_reaction 只限群)。
    async fn set_msg_reaction(&self, msg: &MessageId, emoji_id: &str, set: bool) -> Result<()> {
        let mid = onebot_id_of(msg)?;
        self.call(
            "set_msg_emoji_like",
            json!({ "message_id": mid, "emoji_id": emoji_id, "set": set }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: LLOneBot src/onebot11/action/types.ts (unset_msg_emoji_like)。
    //   参数 {message_id, emoji_id}——无 `set` 字段(它是 set_msg_emoji_like 的 set=false 别名,
    //   单独暴露为一个 wire 动作)。resp null。
    async fn unset_msg_emoji_like(&self, msg: &MessageId, emoji_id: &str) -> Result<()> {
        let mid = onebot_id_of(msg)?;
        self.call(
            "unset_msg_emoji_like",
            json!({ "message_id": mid, "emoji_id": emoji_id }),
        )
        .await?;
        Ok(())
    }

    // OFFICIAL: api/public.md get_version_info → {app_name, app_version, protocol_version}。
    async fn get_version_info(&self) -> Result<VersionInfo> {
        let data = self.call("get_version_info", json!({})).await?;
        Ok(VersionInfo {
            app_name: data_str(&data, "app_name").unwrap_or_default(),
            app_version: data_str(&data, "app_version").unwrap_or_default(),
            protocol_version: data_str(&data, "protocol_version").unwrap_or_default(),
            raw: data,
        })
    }

    // OFFICIAL: api/public.md set_group_anonymous {group_id, enable}。
    async fn set_group_anonymous(&self, group: Uin, enable: bool) -> Result<()> {
        self.call("set_group_anonymous", json!({ "group_id": group.0, "enable": enable })).await?;
        Ok(())
    }

    // OFFICIAL: api/public.md set_group_anonymous_ban
    //   {group_id, (anonymous: {id,name,flag} | anonymous_flag|flag), duration}。
    // 规范接受 `anonymous` 对象(随群消息事件携带)*或* `flag` 字符串二选一。调用方给了对象
    // 时就发它(同时从 `anonymous.flag` 回填 `flag`/`anonymous_flag`,照顾只读这俩的实现);
    // 否则回退到 flag 字符串形式。
    async fn set_group_anonymous_ban(
        &self,
        group: Uin,
        flag: &str,
        anonymous: Option<&Anonymous>,
        duration: u32,
    ) -> Result<()> {
        let mut params = json!({ "group_id": group.0, "duration": duration });
        match anonymous {
            Some(a) => {
                // 对象形式:事件里的 `anonymous` 子对象 {id, name, flag}。
                params["anonymous"] = json!({ "id": a.id, "name": a.name, "flag": a.flag });
                // 同时浮现 flag 字段,照顾只读 flag 的端(对象存在时 a.flag 优先于裸 `flag` 参数)。
                params["anonymous_flag"] = Value::String(a.flag.clone());
                params["flag"] = Value::String(a.flag.clone());
            }
            None => {
                // flag 字符串形式(同发 `flag` / `anonymous_flag` 兼容各厂商)。
                params["anonymous_flag"] = Value::String(flag.to_string());
                params["flag"] = Value::String(flag.to_string());
            }
        }
        self.call("set_group_anonymous_ban", params).await?;
        Ok(())
    }

    // OFFICIAL: api/hidden.md .handle_quick_operation {context, operation}。
    async fn handle_quick_operation(&self, context: Value, operation: Value) -> Result<()> {
        self.call(
            ".handle_quick_operation",
            json!({ "context": context, "operation": operation }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/OCRImage.ts (ocr_image)
    //   (https://github.com/NapNeko/NapCatQQ)。参数 {image};resp texts[]。
    async fn ocr_image(&self, image: &str) -> Result<Vec<OcrText>> {
        let data = self.call("ocr_image", json!({ "image": image })).await?;
        let arr = data
            .get("texts")
            .or(Some(&data))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(arr
            .into_iter()
            .map(|t| OcrText {
                text: t.get("text").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                raw: t,
            })
            .collect())
    }

    // ENDPOINT: NapCat action/extends/SetGroupSign.ts (set_group_sign / send_group_sign)
    //   (https://github.com/NapNeko/NapCatQQ)。参数 {group_id}。
    async fn send_group_sign(&self, group: Uin) -> Result<()> {
        // NapCat 同时注册 set_group_sign / send_group_sign；用别名兜底两端命名。
        self.call_alias("set_group_sign", "send_group_sign", json!({ "group_id": group.0 }))
            .await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/FetchEmojiLike.ts (fetch_emoji_like)
    //   (https://github.com/NapNeko/NapCatQQ)。参数 {message_id, emojiId, emojiType, count, cookie}。
    //   LLOneBot action/llbot/msg/FetchEmojiLike.ts 用蛇形 {emoji_id, message_id, count}。
    //   resp {emojiLikesList:[{tinyId, nickName, headUrl}]}。
    //   一帧里同发两种拼写;各端忽略自己不读的键,故一次调用对 NapCat(驼峰)和 LLOneBot(蛇形)
    //   都能命中。
    async fn fetch_emoji_like(&self, msg: &MessageId, emoji_id: &str, emoji_type: i32) -> Result<Vec<EmojiLiker>> {
        let mid = onebot_id_of(msg)?;
        let data = self
            .call(
                "fetch_emoji_like",
                json!({
                    "message_id": mid,
                    // NapCat 驼峰
                    "emojiId": emoji_id,
                    "emojiType": emoji_type.to_string(),
                    // LLOneBot 蛇形(emoji_id/message_id/count)
                    "emoji_id": emoji_id,
                    "emoji_type": emoji_type.to_string(),
                    "count": 20,
                    "cookie": ""
                }),
            )
            .await?;
        let arr = data
            .get("emojiLikesList")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(arr
            .into_iter()
            .map(|e| EmojiLiker {
                tiny_id: e.get("tinyId").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                nickname: e.get("nickName").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                head_url: e.get("headUrl").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            })
            .collect())
    }

    // ENDPOINT: NapCat action/extends/FetchCustomFace.ts (fetch_custom_face)
    //   (https://github.com/NapNeko/NapCatQQ)。参数 {count=48};resp string[]。
    async fn fetch_custom_face(&self, count: u32) -> Result<Vec<String>> {
        let data = self.call("fetch_custom_face", json!({ "count": count })).await?;
        // resp 是 string[]（直接在 data 上），兼容包了一层的实现。
        let arr = data
            .as_array()
            .cloned()
            .or_else(|| data.get("urls").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default();
        Ok(arr.into_iter().filter_map(|v| v.as_str().map(String::from)).collect())
    }

    // ENDPOINT: NapCat action/go-cqhttp/SendForwardMsg.ts (send_group_forward_msg)
    //   (https://github.com/NapNeko/NapCatQQ)。参数 {group_id, messages}。
    // 返回 message_id + Lagrange forward_id(resId);见 forward_send_result()。
    async fn send_group_forward(&self, group: Uin, nodes: &[ForwardNode]) -> Result<ForwardSendResult> {
        let messages = Self::encode_forward_nodes(nodes);
        let data = self
            .call("send_group_forward_msg", json!({ "group_id": group.0, "messages": messages }))
            .await?;
        Ok(forward_send_result(&data, Peer::group(group.0)))
    }

    // ENDPOINT: NapCat action/go-cqhttp/SendForwardMsg.ts (send_private_forward_msg)
    //   (https://github.com/NapNeko/NapCatQQ)。参数 {user_id, messages}。
    async fn send_private_forward(&self, user: Uin, nodes: &[ForwardNode]) -> Result<ForwardSendResult> {
        let messages = Self::encode_forward_nodes(nodes);
        let data = self
            .call("send_private_forward_msg", json!({ "user_id": user.0, "messages": messages }))
            .await?;
        Ok(forward_send_result(&data, Peer::friend(user.0)))
    }

    // ENDPOINT: NapCat action/go-cqhttp/SendForwardMsg.ts (send_forward_msg)
    //   (https://github.com/NapNeko/NapCatQQ)。参数 {messages}。
    async fn send_forward(&self, nodes: &[ForwardNode]) -> Result<ForwardSendResult> {
        let messages = Self::encode_forward_nodes(nodes);
        let data = self.call("send_forward_msg", json!({ "messages": messages })).await?;
        // 与场景无关:合成一个中性的 friend(0) peer;调用方用返回的 onebot_id。
        Ok(forward_send_result(&data, Peer::friend(0)))
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Generic/GetRkey.cs
    //   (get_rkey) (https://github.com/LagrangeDev/Lagrange.Core).
    //   亦见:NapCat packages/napcat-onebot/action/router.ts (get_rkey);
    //   LLOneBot src/onebot11/action/types.ts (get_rkey)。
    //   Lagrange/NapCat:{rkeys:[{type:"private"|"group", rkey, created_at, ttl}]}。
    //   LLOneBot:扁平 {private_key, group_key, expired_time}——拆成两条 Rkey。
    async fn get_rkey(&self) -> Result<Vec<Rkey>> {
        let data = self.call("get_rkey", json!({})).await?;
        Ok(decode_rkey_list(&data))
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Generic/GetAiCharacters.cs
    //   (get_ai_characters) (https://github.com/LagrangeDev/Lagrange.Core).
    //   亦见 NapCat / LLOneBot `get_ai_characters`。
    //   参数 {group_id, chat_type};resp [{type, characters:[{character_id, character_name, preview_url}]}]。
    async fn get_ai_characters(&self, group: Uin, chat_type: &str) -> Result<Vec<AiCharacterGroup>> {
        let data = self
            .call("get_ai_characters", json!({ "group_id": group.0, "chat_type": chat_type }))
            .await?;
        let arr = data
            .as_array()
            .cloned()
            .or_else(|| data.get("characters").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        Ok(arr
            .into_iter()
            .map(|g| AiCharacterGroup {
                kind: data_str(&g, "type").unwrap_or_default(),
                characters: g
                    .get("characters")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|c| AiCharacter {
                        id: data_str(&c, "character_id").unwrap_or_default(),
                        name: data_str(&c, "character_name").unwrap_or_default(),
                        preview_url: data_str(&c, "preview_url"),
                        raw: c,
                    })
                    .collect(),
                raw: g,
            })
            .collect())
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Group/GetAiRecordOperation.cs
    //   (get_ai_record) (https://github.com/LagrangeDev/Lagrange.Core)。亦见 NapCat `get_ai_record`。
    //   参数 {group_id, character, text, chat_type};resp string(录音 url,data 即 url)。
    async fn get_ai_record(
        &self,
        group: Uin,
        character: &str,
        text: &str,
        chat_type: &str,
    ) -> Result<String> {
        let data = self
            .call(
                "get_ai_record",
                json!({ "group_id": group.0, "character": character, "text": text, "chat_type": chat_type }),
            )
            .await?;
        Ok(data
            .as_str()
            .map(String::from)
            .or_else(|| data_str(&data, "url"))
            .unwrap_or_default())
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Message/SendGroupAiRecordOperation.cs
    //   (send_group_ai_record) (https://github.com/LagrangeDev/Lagrange.Core).
    //   亦见 NapCat / LLOneBot `send_group_ai_record`。
    //   参数 {group_id, character, text, chat_type}
    //   (OneBotGetAiRecord.ChatType 存在);resp {message_id}(OneBotMessageResponse)。
    async fn send_group_ai_record(
        &self,
        group: Uin,
        character: &str,
        text: &str,
        chat_type: &str,
    ) -> Result<MessageId> {
        let data = self
            .call(
                "send_group_ai_record",
                json!({ "group_id": group.0, "character": character, "text": text, "chat_type": chat_type }),
            )
            .await?;
        let onebot_id = data_i64(&data, "message_id").map(|v| v as i32);
        Ok(MessageId { peer: Peer::group(group.0), seq: 0, onebot_id })
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/user/SetOnlineStatus.ts (set_online_status)
    //   (https://github.com/NapNeko/NapCatQQ)。亦见 LLOneBot src/onebot11/action/types.ts。
    //   参数 {status, ext_status, battery_status};resp {}。
    async fn set_online_status(
        &self,
        status: i32,
        ext_status: i32,
        battery_status: i32,
    ) -> Result<()> {
        self.call(
            "set_online_status",
            json!({ "status": status, "ext_status": ext_status, "battery_status": battery_status }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/user/SetInputStatus.ts (set_input_status)
    //   (https://github.com/NapNeko/NapCatQQ)。亦见 LLOneBot src/onebot11/action/types.ts。
    //   参数 {user_id, event_type};resp {}。
    async fn set_input_status(&self, user: Uin, event_type: i32) -> Result<()> {
        self.call(
            "set_input_status",
            json!({ "user_id": user.0, "event_type": event_type }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/user/GetProfileLike.ts (get_profile_like)
    //   (https://github.com/NapNeko/NapCatQQ)。亦见 LLOneBot src/onebot11/action/types.ts。
    //   参数 {};resp [{uin, nick, num}] 或 [{uid, nick, num}]。
    async fn get_profile_like(&self) -> Result<Vec<ProfileLiker>> {
        let data = self.call("get_profile_like", json!({})).await?;
        let arr = data
            .get("userInfos")
            .and_then(Value::as_array)
            .or_else(|| data.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(arr
            .into_iter()
            .map(|v| ProfileLiker {
                user: Uin(data_i64(&v, "uin").or_else(|| data_i64(&v, "uid")).unwrap_or(0)),
                nickname: data_str(&v, "nick")
                    .or_else(|| data_str(&v, "nickName"))
                    .unwrap_or_default(),
                times: data_i64(&v, "num").unwrap_or(0) as i32,
                raw: v,
            })
            .collect())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/user/GetFriendsWithCategory.ts
    //   (get_friends_with_category) (https://github.com/NapNeko/NapCatQQ).
    //   亦见 LLOneBot src/onebot11/action/types.ts。
    //   参数 {};resp [{categoryId, categoryName, buddyList:[FriendInfo]}]。
    async fn get_friends_with_category(&self) -> Result<Vec<FriendCategoryList>> {
        let data = self.call("get_friends_with_category", json!({})).await?;
        let arr = data.as_array().cloned().unwrap_or_default();
        Ok(arr
            .into_iter()
            .map(|g| {
                let friends = g
                    .get("buddyList")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(friend_info_from)
                    .collect();
                FriendCategoryList {
                    category_id: data_i64(&g, "categoryId").unwrap_or(0) as i32,
                    category_name: data_str(&g, "categoryName").unwrap_or_default(),
                    friends,
                }
            })
            .collect())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/extends/SetGroupRemark.ts
    //   (set_group_remark) (https://github.com/NapNeko/NapCatQQ)。参数 {group_id, remark}。
    //   亦见 LLOneBot set_group_remark(共有)。
    async fn set_group_remark(&self, group: Uin, remark: &str) -> Result<()> {
        self.call("set_group_remark", json!({ "group_id": group.0, "remark": remark }))
            .await?;
        Ok(())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/group/GetGroupShutList.ts
    //   (get_group_shut_list) (https://github.com/NapNeko/NapCatQQ)。参数 {group_id}。
    //   resp [{user_id, nickname, shut_up_time}];经 member_info_from 映射。亦见 LLOneBot(共有)。
    async fn get_group_shut_list(&self, group: Uin) -> Result<Vec<MemberInfo>> {
        let data = self.call("get_group_shut_list", json!({ "group_id": group.0 })).await?;
        let arr = data.as_array().cloned().unwrap_or_default();
        Ok(arr.iter().map(member_info_from).collect())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/go-cqhttp/GetGroupAtAllRemain.ts
    //   (get_group_at_all_remain) (https://github.com/NapNeko/NapCatQQ)。参数 {group_id}。
    //   resp {can_at_all, remain_at_all_count_for_group, remain_at_all_count_for_uin}。go-cqhttp(共有)。
    async fn get_group_at_all_remain(&self, group: Uin) -> Result<Value> {
        self.call("get_group_at_all_remain", json!({ "group_id": group.0 })).await
    }

    // ===== 媒体 / 文件 / 转发（NapCat + LLOneBot 共有）=====

    // ENDPOINT: NapCat action/file/GetFile.ts (get_file);亦见 LLOneBot get_file。
    //   参数 {file_id};resp {file?, url?, file_size?(str), file_name?, base64?}。
    async fn get_file(&self, file_id: &str) -> Result<FileFetch> {
        let data = self.call("get_file", json!({ "file_id": file_id })).await?;
        let size = data_str(&data, "file_size")
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| data.get("file_size").and_then(Value::as_u64))
            .unwrap_or(0);
        Ok(FileFetch {
            url: data_str(&data, "url"),
            path: data_str(&data, "file"),
            name: data_str(&data, "file_name").unwrap_or_default(),
            size,
            base64: data_str(&data, "base64"),
            raw: data,
        })
    }

    // ENDPOINT: NapCat action/msg/ForwardSingleMsg.ts (forward_friend_single_msg);亦见 LLOneBot。
    //   参数 {user_id, message_id};resp null。合成一个 friend(user) MessageId。
    async fn forward_friend_single_msg(&self, user: Uin, msg: &MessageId) -> Result<MessageId> {
        let mid = onebot_id_of(msg)?;
        let data = self
            .call("forward_friend_single_msg", json!({ "user_id": user.0, "message_id": mid }))
            .await?;
        let onebot_id = data_i64(&data, "message_id").map(|v| v as i32);
        Ok(MessageId { peer: Peer::friend(user.0), seq: 0, onebot_id })
    }

    // ENDPOINT: NapCat action/msg/ForwardSingleMsg.ts (forward_group_single_msg);亦见 LLOneBot。
    //   参数 {group_id, message_id};resp null。合成一个 group(group) MessageId。
    async fn forward_group_single_msg(&self, group: Uin, msg: &MessageId) -> Result<MessageId> {
        let mid = onebot_id_of(msg)?;
        let data = self
            .call("forward_group_single_msg", json!({ "group_id": group.0, "message_id": mid }))
            .await?;
        let onebot_id = data_i64(&data, "message_id").map(|v| v as i32);
        Ok(MessageId { peer: Peer::group(group.0), seq: 0, onebot_id })
    }

    // ENDPOINT: NapCat action/system/GetSystemMsg.ts (get_group_system_msg);共有(NapCat + LLOneBot)。
    //   参数 {}(可选 count,服务端默认 50)。
    async fn get_group_system_msg(&self) -> Result<Value> {
        self.call("get_group_system_msg", json!({})).await
    }

    // ENDPOINT: NapCat action/extends/GetRobotUinRange.ts (get_robot_uin_range);共有(NapCat + LLOneBot)。
    //   参数 {};resp [{minUin, maxUin}]。
    async fn get_robot_uin_range(&self) -> Result<Value> {
        self.call("get_robot_uin_range", json!({})).await
    }

    // ENDPOINT: NapCat action/extends/GetGroupAlbumMediaList.ts +
    //   LLOneBot action/llbot/group/GroupAlbum/GetGroupAlbumMediaList.ts (get_group_album_media_list);
    //   两厂商共用此 wire 名。参数 {group_id, album_id, attach_info?};resp 媒体列表。
    //   group_id 序列化为字符串(NapCat Type.String / LLOneBot handler .toString())。
    async fn get_group_album_media_list(
        &self,
        group: Uin,
        album_id: &str,
        attach_info: Option<&str>,
    ) -> Result<Value> {
        let mut params = Map::new();
        params.insert("group_id".into(), json!(group.0.to_string()));
        params.insert("album_id".into(), json!(album_id));
        if let Some(a) = attach_info {
            params.insert("attach_info".into(), json!(a));
        }
        self.call("get_group_album_media_list", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/extends/TransGroupFile.ts (trans_group_file) +
    //   LLOneBot action/llbot/file/SetGroupFileForever.ts (set_group_file_forever).
    //   同一逻辑动作(把临时群文件转永久)在两个不同 wire 名下。以本厂商原生名为主,经
    //   call_alias 回退到另一个。参数 {group_id, file_id}。
    async fn set_group_file_forever(&self, group: Uin, file_id: &str) -> Result<()> {
        let params = json!({ "group_id": group.0, "file_id": file_id });
        let (primary, alt) = match self.vendor() {
            nagisa_types::vendor::Vendor::NapCat => ("trans_group_file", "set_group_file_forever"),
            _ => ("set_group_file_forever", "trans_group_file"),
        };
        self.call_alias(primary, alt, params).await?;
        Ok(())
    }

    // ENDPOINT: LLOneBot action/llbot/user/GetDoubtFriendsAddRequest.ts +
    //   NapCat (get_doubt_friends_add_request);共有。参数 {count}(默认 50);
    //   resp [{flag,uin,nick,...}]。
    async fn get_doubt_friends_add_request(&self, count: u32) -> Result<Value> {
        self.call("get_doubt_friends_add_request", json!({ "count": count })).await
    }

    // ENDPOINT: LLOneBot action/llbot/user/SetDoubtFriendsAddRequest.ts(参数 {flag})+
    //   NapCat(参数 {flag, approve});共有。我们无条件同发 {flag, approve}——LLOneBot 会忽略多出
    //   的 `approve` 键(它总是同意),故这种同发是安全的。
    async fn set_doubt_friends_add_request(&self, flag: &str, approve: bool) -> Result<()> {
        self.call(
            "set_doubt_friends_add_request",
            json!({ "flag": flag, "approve": approve }),
        )
        .await?;
        Ok(())
    }

    // ===== NapCat 专属 =====

    // ENDPOINT: NapCat packages/napcat-onebot/action/user/SetDiyOnlineStatus.ts
    //   (set_diy_online_status) (https://github.com/NapNeko/NapCatQQ)。
    //   参数 {face_id, face_type, wording};resp {}。
    async fn set_diy_online_status(
        &self,
        face_id: i32,
        face_type: i32,
        wording: &str,
    ) -> Result<()> {
        self.call(
            "set_diy_online_status",
            json!({ "face_id": face_id, "face_type": face_type, "wording": wording }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/user/GetUserStatus.ts
    //   (nc_get_user_status) (https://github.com/NapNeko/NapCatQQ)。
    //   参数 {user_id};resp {status, ext_status}。
    //   注意:wire 动作名是 `nc_get_user_status`(含 `nc_` 前缀)。
    async fn get_user_status(&self, user: Uin) -> Result<Value> {
        self.call("nc_get_user_status", json!({ "user_id": user.0 })).await
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/user/SetSelfLongnick.ts
    //   (set_self_longnick) (https://github.com/NapNeko/NapCatQQ)。
    //   参数 {longNick}(驼峰 wire 键);resp {}。
    async fn set_self_longnick(&self, longnick: &str) -> Result<()> {
        self.call("set_self_longnick", json!({ "longNick": longnick })).await?;
        Ok(())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/user/GetUnidirectionalFriendList.ts
    //   (get_unidirectional_friend_list) (https://github.com/NapNeko/NapCatQQ)。
    //   参数 {};resp [FriendInfo]。
    async fn get_unidirectional_friend_list(&self) -> Result<Vec<FriendInfo>> {
        let data = self.call("get_unidirectional_friend_list", json!({})).await?;
        let arr = data.as_array().cloned().unwrap_or_default();
        Ok(arr.iter().map(friend_info_from).collect())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/user/GetRecentContact.ts
    //   (get_recent_contact) (https://github.com/NapNeko/NapCatQQ)。
    //   参数 {count};resp Value(最近联系人数组)。
    async fn get_recent_contact(&self, count: u32) -> Result<Value> {
        self.call("get_recent_contact", json!({ "count": count })).await
    }

    // ENDPOINT: NapCat action/extends/SetGroupKickMembers.ts (set_group_kick_members)。
    //   参数 {group_id, user_id:[Uin], reject_add_request};user_id 是整数 JSON 数组。
    async fn set_group_kick_members(
        &self,
        group: Uin,
        users: &[Uin],
        reject_add: bool,
    ) -> Result<()> {
        let ids: Vec<i64> = users.iter().map(|u| u.0).collect();
        self.call(
            "set_group_kick_members",
            json!({ "group_id": group.0, "user_id": ids, "reject_add_request": reject_add }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/group/GetGroupDetailInfo.ts (get_group_detail_info)。
    //   参数 {group_id};resp 更丰富的 GroupInfo,经 group_info_from 映射。
    async fn get_group_detail_info(&self, group: Uin) -> Result<GroupInfo> {
        let data = self.call("get_group_detail_info", json!({ "group_id": group.0 })).await?;
        Ok(group_info_from(&data))
    }

    // ENDPOINT: NapCat action/extends/GetGroupInfoEx.ts (get_group_info_ex)。
    //   参数 {group_id};resp 原始 groupExtInfo Value。
    async fn get_group_info_ex(&self, group: Uin) -> Result<Value> {
        self.call("get_group_info_ex", json!({ "group_id": group.0 })).await
    }

    // ENDPOINT: NapCat action/group/GetGroupIgnoredNotifies.ts (get_group_ignored_notifies)。
    //   参数 {};resp {invited_requests[], InvitedRequest[], join_requests[]}。
    async fn get_group_ignored_notifies(&self) -> Result<Value> {
        self.call("get_group_ignored_notifies", json!({})).await
    }

    // ENDPOINT: NapCat action/extends/SetGroupSearch.ts (set_group_search)。
    //   参数 {group_id, no_code_finger_open?, no_finger_open?}。
    async fn set_group_search(
        &self,
        group: Uin,
        no_code_finger_open: Option<i32>,
        no_finger_open: Option<i32>,
    ) -> Result<()> {
        let mut params = Map::new();
        params.insert("group_id".into(), json!(group.0));
        if let Some(v) = no_code_finger_open {
            params.insert("no_code_finger_open".into(), json!(v));
        }
        if let Some(v) = no_finger_open {
            params.insert("no_finger_open".into(), json!(v));
        }
        self.call("set_group_search", Value::Object(params)).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/SetGroupAddOption.ts (set_group_add_option)。
    //   参数 {group_id, add_type, group_question?, group_answer?}。
    async fn set_group_add_option(
        &self,
        group: Uin,
        add_type: i32,
        group_question: Option<&str>,
        group_answer: Option<&str>,
    ) -> Result<()> {
        let mut params = Map::new();
        params.insert("group_id".into(), json!(group.0));
        params.insert("add_type".into(), json!(add_type));
        if let Some(q) = group_question {
            params.insert("group_question".into(), json!(q));
        }
        if let Some(a) = group_answer {
            params.insert("group_answer".into(), json!(a));
        }
        self.call("set_group_add_option", Value::Object(params)).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/SetGroupRobotAddOption.ts (set_group_robot_add_option)。
    //   参数 {group_id, robot_member_switch?, robot_member_examine?}。
    async fn set_group_robot_add_option(
        &self,
        group: Uin,
        robot_member_switch: Option<i32>,
        robot_member_examine: Option<i32>,
    ) -> Result<()> {
        let mut params = Map::new();
        params.insert("group_id".into(), json!(group.0));
        if let Some(v) = robot_member_switch {
            params.insert("robot_member_switch".into(), json!(v));
        }
        if let Some(v) = robot_member_examine {
            params.insert("robot_member_examine".into(), json!(v));
        }
        self.call("set_group_robot_add_option", Value::Object(params)).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/GetGroupSignedList.ts (get_group_signed_list)。
    //   参数 {group_id};resp [{user_id, nick, time, rank}],原样作 Value 透传。
    async fn get_group_signed_list(&self, group: Uin) -> Result<Value> {
        self.call("get_group_signed_list", json!({ "group_id": group.0 })).await
    }

    // ENDPOINT: NapCat action/msg/FetchPttText.ts (fetch_ptt_text)。
    //   参数 {message_id};resp {text}。
    async fn fetch_ptt_text(&self, msg: &MessageId) -> Result<String> {
        let mid = onebot_id_of(msg)?;
        let data = self.call("fetch_ptt_text", json!({ "message_id": mid })).await?;
        Ok(data_str(&data, "text").unwrap_or_default())
    }

    // ENDPOINT: NapCat action/extends/TranslateEnWordToZn.ts (translate_en2zh)。
    //   参数 {words:[String]};resp {words:[String]}。
    async fn translate_en2zh(&self, words: &[String]) -> Result<Vec<String>> {
        let data = self.call("translate_en2zh", json!({ "words": words })).await?;
        Ok(data
            .get("words")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect())
    }

    // ENDPOINT: NapCat action/packet/GetRkeyServer.ts (get_rkey_server)。
    //   参数 {};resp {private_rkey?, group_rkey?, expired_time?, name},原样透传。
    async fn get_rkey_server(&self) -> Result<Value> {
        self.call("get_rkey_server", json!({})).await
    }

    // ENDPOINT: NapCat action/extends/GetClientkey.ts (get_clientkey)。
    //   参数 {};resp {clientkey}。
    async fn get_clientkey(&self) -> Result<String> {
        let data = self.call("get_clientkey", json!({})).await?;
        Ok(data_str(&data, "clientkey").unwrap_or_default())
    }

    // ENDPOINT: NapCat action/extends/GetMiniAppArk.ts (get_mini_app_ark)。
    //   参数 union(驼峰键);原样透传,resp ark Value。
    async fn get_mini_app_ark(&self, params: Value) -> Result<Value> {
        self.call("get_mini_app_ark", params).await
    }

    // ENDPOINT: NapCat action/extends/ShareContact.ts。
    //   wire 名是 `send_ark_share`(当前),带已弃用的别名 `ArkSharePeer`。
    //   参数 {user_id?, group_id?, phone_number?};resp ark JSON Value。
    async fn share_contact(
        &self,
        user: Option<Uin>,
        group: Option<Uin>,
        phone_number: Option<&str>,
    ) -> Result<Value> {
        let mut params = Map::new();
        if let Some(u) = user {
            params.insert("user_id".into(), json!(u.0.to_string()));
        }
        if let Some(g) = group {
            params.insert("group_id".into(), json!(g.0.to_string()));
        }
        if let Some(p) = phone_number {
            params.insert("phone_number".into(), json!(p));
        }
        self.call_alias("send_ark_share", "ArkSharePeer", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/extends/GetEmojiLikes.ts (get_emoji_likes);NapCat 专属
    //   (LLOneBot 只有 fetch_emoji_like)。参数 {message_id, emoji_id, emoji_type, count};
    //   resp {emoji_like_list:[{user_id, nick_name}]}。emoji_type 序列化为字符串
    //   (与 fetch_emoji_like 的 emojiType 约定一致)。
    async fn get_emoji_likes(
        &self,
        msg: &MessageId,
        emoji_id: &str,
        emoji_type: i32,
        count: u32,
    ) -> Result<Value> {
        let mid = onebot_id_of(msg)?;
        self.call(
            "get_emoji_likes",
            json!({
                "message_id": mid,
                "emoji_id": emoji_id,
                "emoji_type": emoji_type.to_string(),
                "count": count,
            }),
        )
        .await
    }

    // ----- NapCat 群相册 (qun album) —— group_id 序列化为字符串 (NapCat Type.String) -----

    // ENDPOINT: NapCat action/extends/GetQunAlbumList.ts (get_qun_album_list)。
    //   参数 {group_id, attach_info?};resp {album_list, attach_info, has_more}。
    async fn get_qun_album_list(&self, group: Uin, attach_info: Option<&str>) -> Result<Value> {
        let mut params = Map::new();
        params.insert("group_id".into(), json!(group.0.to_string()));
        if let Some(a) = attach_info {
            params.insert("attach_info".into(), json!(a));
        }
        self.call("get_qun_album_list", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/extends/UploadImageToQunAlbum.ts (upload_image_to_qun_album)。
    //   参数 {group_id, album_id, album_name, file}。
    async fn upload_image_to_qun_album(
        &self,
        group: Uin,
        album_id: &str,
        album_name: &str,
        file: &str,
    ) -> Result<Value> {
        self.call(
            "upload_image_to_qun_album",
            json!({
                "group_id": group.0.to_string(),
                "album_id": album_id,
                "album_name": album_name,
                "file": file,
            }),
        )
        .await
    }

    // ENDPOINT: NapCat action/extends/DelGroupAlbumMedia.ts (del_group_album_media)。
    //   参数 {group_id, album_id, lloc}。
    async fn del_group_album_media(
        &self,
        group: Uin,
        album_id: &str,
        lloc: &str,
    ) -> Result<()> {
        self.call(
            "del_group_album_media",
            json!({ "group_id": group.0.to_string(), "album_id": album_id, "lloc": lloc }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/SetGroupAlbumMediaLike.ts (set_group_album_media_like)。
    //   参数 {group_id, album_id, batch_id, lloc?}。
    async fn set_group_album_media_like(
        &self,
        group: Uin,
        album_id: &str,
        batch_id: &str,
        lloc: Option<&str>,
    ) -> Result<()> {
        let mut params = Map::new();
        params.insert("group_id".into(), json!(group.0.to_string()));
        params.insert("album_id".into(), json!(album_id));
        params.insert("batch_id".into(), json!(batch_id));
        if let Some(l) = lloc {
            params.insert("lloc".into(), json!(l));
        }
        self.call("set_group_album_media_like", Value::Object(params)).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/DoGroupAlbumComment.ts (do_group_album_comment)。
    //   参数 {group_id, album_id, lloc, content}。
    async fn do_group_album_comment(
        &self,
        group: Uin,
        album_id: &str,
        lloc: &str,
        content: &str,
    ) -> Result<Value> {
        self.call(
            "do_group_album_comment",
            json!({
                "group_id": group.0.to_string(),
                "album_id": album_id,
                "lloc": lloc,
                "content": content,
            }),
        )
        .await
    }

    // ----- NapCat 闪传 / 文件集 (flash / fileset) -----

    // ENDPOINT: NapCat action/file/flash/CreateFlashTask.ts (create_flash_task)。
    //   参数 {files:[String], name?, thumb_path?}。
    async fn create_flash_task(
        &self,
        files: &[String],
        name: Option<&str>,
        thumb_path: Option<&str>,
    ) -> Result<Value> {
        let mut params = Map::new();
        params.insert("files".into(), json!(files));
        if let Some(n) = name {
            params.insert("name".into(), json!(n));
        }
        if let Some(t) = thumb_path {
            params.insert("thumb_path".into(), json!(t));
        }
        self.call("create_flash_task", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/file/flash/SendFlashMsg.ts (send_flash_msg)。
    //   参数 {fileset_id, user_id?, group_id?};各 id 序列化为字符串。
    async fn send_flash_msg(
        &self,
        fileset_id: &str,
        user: Option<Uin>,
        group: Option<Uin>,
    ) -> Result<Value> {
        let mut params = Map::new();
        params.insert("fileset_id".into(), json!(fileset_id));
        if let Some(u) = user {
            params.insert("user_id".into(), json!(u.0.to_string()));
        }
        if let Some(g) = group {
            params.insert("group_id".into(), json!(g.0.to_string()));
        }
        self.call("send_flash_msg", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/file/flash/GetShareLink.ts (get_share_link)。
    //   参数 {fileset_id}。
    async fn get_share_link(&self, fileset_id: &str) -> Result<Value> {
        self.call("get_share_link", json!({ "fileset_id": fileset_id })).await
    }

    // ENDPOINT: NapCat action/file/flash/DownloadFileset.ts (download_fileset)。
    //   参数 {fileset_id}。
    async fn download_fileset(&self, fileset_id: &str) -> Result<Value> {
        self.call("download_fileset", json!({ "fileset_id": fileset_id })).await
    }

    // ENDPOINT: NapCat action/file/flash/GetFilesetInfo.ts (get_fileset_info)。
    //   参数 {fileset_id}。
    async fn get_fileset_info(&self, fileset_id: &str) -> Result<Value> {
        self.call("get_fileset_info", json!({ "fileset_id": fileset_id })).await
    }

    // ENDPOINT: NapCat action/file/flash/GetFlashFileList.ts (get_flash_file_list)。
    //   参数 {fileset_id}。
    async fn get_flash_file_list(&self, fileset_id: &str) -> Result<Value> {
        self.call("get_flash_file_list", json!({ "fileset_id": fileset_id })).await
    }

    // ENDPOINT: NapCat action/file/flash/GetFlashFileUrl.ts (get_flash_file_url)。
    //   参数 {fileset_id, file_name?, file_index?}。
    async fn get_flash_file_url(
        &self,
        fileset_id: &str,
        file_name: Option<&str>,
        file_index: Option<i64>,
    ) -> Result<Value> {
        let mut params = Map::new();
        params.insert("fileset_id".into(), json!(fileset_id));
        if let Some(n) = file_name {
            params.insert("file_name".into(), json!(n));
        }
        if let Some(i) = file_index {
            params.insert("file_index".into(), json!(i));
        }
        self.call("get_flash_file_url", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/file/flash/GetFilesetIdByCode.ts (get_fileset_id)。
    //   参数 {share_code};resp {fileset_id}。
    async fn get_fileset_id(&self, share_code: &str) -> Result<Value> {
        self.call("get_fileset_id", json!({ "share_code": share_code })).await
    }

    // ----- NapCat 在线文件 (online file) -----

    // ENDPOINT: NapCat action/file/online/SendOnlineFile.ts (send_online_file)。
    //   参数 {user_id, file_path, file_name?};user_id 序列化为字符串。
    async fn send_online_file(
        &self,
        user: Uin,
        file_path: &str,
        file_name: Option<&str>,
    ) -> Result<Value> {
        let mut params = Map::new();
        params.insert("user_id".into(), json!(user.0.to_string()));
        params.insert("file_path".into(), json!(file_path));
        if let Some(n) = file_name {
            params.insert("file_name".into(), json!(n));
        }
        self.call("send_online_file", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/file/online/SendOnlineFolder.ts (send_online_folder)。
    //   参数 {user_id, folder_path, folder_name?}。
    async fn send_online_folder(
        &self,
        user: Uin,
        folder_path: &str,
        folder_name: Option<&str>,
    ) -> Result<Value> {
        let mut params = Map::new();
        params.insert("user_id".into(), json!(user.0.to_string()));
        params.insert("folder_path".into(), json!(folder_path));
        if let Some(n) = folder_name {
            params.insert("folder_name".into(), json!(n));
        }
        self.call("send_online_folder", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/file/online/GetOnlineFileMessages.ts (get_online_file_msg)。
    //   wire 名是 `get_online_file_msg`(不是 ..._messages)。参数 {user_id}。
    async fn get_online_file_msg(&self, user: Uin) -> Result<Value> {
        self.call("get_online_file_msg", json!({ "user_id": user.0.to_string() })).await
    }

    // ENDPOINT: NapCat action/file/online/ReceiveOnlineFile.ts (receive_online_file)。
    //   参数 {user_id, msg_id, element_id}。
    async fn receive_online_file(
        &self,
        user: Uin,
        msg_id: &str,
        element_id: &str,
    ) -> Result<Value> {
        self.call(
            "receive_online_file",
            json!({ "user_id": user.0.to_string(), "msg_id": msg_id, "element_id": element_id }),
        )
        .await
    }

    // ENDPOINT: NapCat action/file/online/RefuseOnlineFile.ts (refuse_online_file)。
    //   参数 {user_id, msg_id, element_id}。
    async fn refuse_online_file(
        &self,
        user: Uin,
        msg_id: &str,
        element_id: &str,
    ) -> Result<()> {
        self.call(
            "refuse_online_file",
            json!({ "user_id": user.0.to_string(), "msg_id": msg_id, "element_id": element_id }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/file/online/CancelOnlineFile.ts (cancel_online_file)。
    //   参数 {user_id, msg_id}。
    async fn cancel_online_file(&self, user: Uin, msg_id: &str) -> Result<()> {
        self.call(
            "cancel_online_file",
            json!({ "user_id": user.0.to_string(), "msg_id": msg_id }),
        )
        .await?;
        Ok(())
    }

    // ----- NapCat 群待办 (group todo) —— 共用 BaseGroupTodoAction {group_id, message_id} -----

    // ENDPOINT: NapCat action/packet/SetGroupTodo.ts (set_group_todo);参数 {group_id, message_id}。
    async fn set_group_todo(&self, group: Uin, msg: &MessageId) -> Result<()> {
        let mid = onebot_id_of(msg)?;
        self.call("set_group_todo", json!({ "group_id": group.0, "message_id": mid })).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/packet/CompleteGroupTodo.ts (complete_group_todo)。
    async fn complete_group_todo(&self, group: Uin, msg: &MessageId) -> Result<()> {
        let mid = onebot_id_of(msg)?;
        self.call("complete_group_todo", json!({ "group_id": group.0, "message_id": mid }))
            .await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/packet/CancelGroupTodo.ts (cancel_group_todo)。
    async fn cancel_group_todo(&self, group: Uin, msg: &MessageId) -> Result<()> {
        let mid = onebot_id_of(msg)?;
        self.call("cancel_group_todo", json!({ "group_id": group.0, "message_id": mid }))
            .await?;
        Ok(())
    }

    // ----- NapCat 收藏 / 杂项 (collection / misc) -----

    // ENDPOINT: NapCat action/extends/CreateCollection.ts (create_collection)。
    //   参数 {rawData, brief}(驼峰 wire 键 `rawData`)。
    async fn create_collection(&self, raw_data: &str, brief: &str) -> Result<Value> {
        self.call("create_collection", json!({ "rawData": raw_data, "brief": brief })).await
    }

    // ENDPOINT: NapCat action/extends/GetCollectionList.ts (get_collection_list)。
    //   参数 {category, count}(服务端两者皆字符串)。
    async fn get_collection_list(&self, category: &str, count: u32) -> Result<Value> {
        self.call(
            "get_collection_list",
            json!({ "category": category, "count": count.to_string() }),
        )
        .await
    }

    // ENDPOINT: NapCat action/go-cqhttp/GoCQHTTPCheckUrlSafely.ts (check_url_safely)。
    //   参数 {url};resp {level:i32}。
    async fn check_url_safely(&self, url: &str) -> Result<Value> {
        self.call("check_url_safely", json!({ "url": url })).await
    }

    // ENDPOINT: NapCat action/go-cqhttp/GetOnlineClient.ts (get_online_clients)。
    //   参数 {}(NapCat schema 为空——无 `no_cache`);resp clients 数组。
    async fn get_online_clients(&self) -> Result<Value> {
        self.call("get_online_clients", json!({})).await
    }

    // ENDPOINT: NapCat action/go-cqhttp/DownloadFile.ts (download_file)。
    //   参数 {url?, base64?, name?, headers?}(无 thread_count);resp {file} = 本地路径。
    async fn download_file(
        &self,
        url: Option<&str>,
        base64: Option<&str>,
        name: Option<&str>,
        headers: Option<Value>,
    ) -> Result<String> {
        let mut params = Map::new();
        if let Some(u) = url {
            params.insert("url".into(), json!(u));
        }
        if let Some(b) = base64 {
            params.insert("base64".into(), json!(b));
        }
        if let Some(n) = name {
            params.insert("name".into(), json!(n));
        }
        if let Some(h) = headers {
            params.insert("headers".into(), h);
        }
        let data = self.call("download_file", Value::Object(params)).await?;
        Ok(data_str(&data, "file").unwrap_or_default())
    }

    // ENDPOINT: NapCat action/extends/BotExit.ts (bot_exit);参数 {};resp null。
    async fn bot_exit(&self) -> Result<()> {
        self.call("bot_exit", json!({})).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/msg/MarkMsgAsRead.ts (_mark_all_as_read);参数 {};resp null。
    async fn mark_all_as_read(&self) -> Result<()> {
        self.call("_mark_all_as_read", json!({})).await?;
        Ok(())
    }

    // ----- NapCat 自定义表情 (custom face) -----

    // ENDPOINT: NapCat action/extends/CustomFace.ts (fetch_custom_face_detail);参数 {count}。
    async fn fetch_custom_face_detail(&self, count: u32) -> Result<Value> {
        self.call("fetch_custom_face_detail", json!({ "count": count })).await
    }

    // ENDPOINT: NapCat action/extends/CustomFace.ts (add_custom_face);参数 union,原样透传。
    async fn add_custom_face(&self, params: Value) -> Result<Value> {
        self.call("add_custom_face", params).await
    }

    // ENDPOINT: NapCat action/extends/CustomFace.ts (delete_custom_face);参数 union {res_id?,id?,ids?,md5?}。
    async fn delete_custom_face(&self, params: Value) -> Result<()> {
        self.call("delete_custom_face", params).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/CustomFace.ts (set_custom_face_desc)。
    //   参数 {emoji_id, res_id, md5, desc}。
    async fn set_custom_face_desc(
        &self,
        emoji_id: &str,
        res_id: &str,
        md5: &str,
        desc: &str,
    ) -> Result<()> {
        self.call(
            "set_custom_face_desc",
            json!({ "emoji_id": emoji_id, "res_id": res_id, "md5": md5, "desc": desc }),
        )
        .await?;
        Ok(())
    }

    // ----- NapCat go-cqhttp 兼容杂项 -----

    // ENDPOINT: NapCat action/go-cqhttp/GoCQHTTPGetModelShow.ts (_get_model_show);参数 {model?}。
    async fn get_model_show(&self, model: Option<&str>) -> Result<Value> {
        let mut params = Map::new();
        if let Some(m) = model {
            params.insert("model".into(), json!(m));
        }
        self.call("_get_model_show", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/go-cqhttp/GoCQHTTPSetModelShow.ts (_set_model_show)。
    //   NapCat schema 为空(兼容用的空操作);我们按 go-cqhttp 惯例发 {model, model_show}。
    async fn set_model_show(&self, model: &str, model_show: &str) -> Result<()> {
        self.call("_set_model_show", json!({ "model": model, "model_show": model_show })).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/go-cqhttp/GetGroupFileSystemInfo.ts (get_group_file_system_info)。
    //   参数 {group_id};resp {file_count, limit_count, used_space, total_space}。
    async fn get_group_file_system_info(&self, group: Uin) -> Result<Value> {
        self.call("get_group_file_system_info", json!({ "group_id": group.0 })).await
    }

    // ENDPOINT: NapCat action/packet/GetPacketStatus.ts (nc_get_packet_status);参数 {}。
    async fn nc_get_packet_status(&self) -> Result<Value> {
        self.call("nc_get_packet_status", json!({})).await
    }

    // ENDPOINT: NapCat action/msg/MarkMsgAsRead.ts (MarkPrivateMsgAsRead -> mark_private_msg_as_read)。
    //   参数 {user_id?, group_id?, message_id?}(至少一个)。私聊变体发 {user_id}。
    async fn mark_private_msg_as_read(&self, user: Uin) -> Result<()> {
        self.call("mark_private_msg_as_read", json!({ "user_id": user.0 })).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/msg/MarkMsgAsRead.ts (MarkGroupMsgAsRead -> mark_group_msg_as_read)。
    //   参数 {user_id?, group_id?, message_id?}(至少一个)。群变体把 {group_id} 作字符串发
    //   (PayloadSchema:group_id 为 Type.String)。
    async fn mark_group_msg_as_read(&self, group: Uin) -> Result<()> {
        self.call("mark_group_msg_as_read", json!({ "group_id": group.0.to_string() })).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/ShareContact.ts
    //   (SendGroupArkShare -> send_group_ark_share;已弃用别名 ShareGroupEx -> ArkShareGroup)。
    //   参数 {group_id:String};resp Ark JSON 字符串,原样作 Value 透传。
    async fn send_group_ark_share(&self, group: Uin) -> Result<Value> {
        self.call_alias(
            "send_group_ark_share",
            "ArkShareGroup",
            json!({ "group_id": group.0.to_string() }),
        )
        .await
    }

    // ENDPOINT: NapCat action/packet/GetRkeyEx.ts (GetRkey class -> nc_get_rkey);参数 {}。
    //   resp Rkey 数组(Type.Any),原样透传。与 OB 共有的 get_rkey 不同。
    async fn nc_get_rkey(&self) -> Result<Value> {
        self.call("nc_get_rkey", json!({})).await
    }

    // ENDPOINT: NapCat action/extends/SendPacket.ts (send_packet)。
    //   参数 {cmd, data(hex), rsp:bool};resp hex string|undefined,原样作 Value 透传。
    async fn send_packet(&self, cmd: &str, data: &str, rsp: bool) -> Result<Value> {
        self.call("send_packet", json!({ "cmd": cmd, "data": data, "rsp": rsp })).await
    }

    // ENDPOINT: NapCat action/extends/GetGroupAddRequest.ts (-> get_group_ignore_add_request);参数 {}。
    //   resp [{request_id, invitor_uin, group_id, checked, actor, ...}],原样透传。
    async fn get_group_ignore_add_request(&self) -> Result<Value> {
        self.call("get_group_ignore_add_request", json!({})).await
    }

    // ENDPOINT: NapCat action/extends/GetGroupAddRequest.ts (-> get_group_ignore_add_request);参数 {}。
    //   NapCat 不注册独立的 `get_group_add_request` wire 名(go-cqhttp 已弃用,重定向到
    //   get_group_system_msg)。先试旧名,再回退到当前的私有 wire 名,使新旧端都能解析。
    async fn get_group_add_request(&self) -> Result<Value> {
        self.call_alias("get_group_add_request", "get_group_ignore_add_request", json!({}))
            .await
    }

    // ENDPOINT: NapCat action/extends/SetGroupAlbumMediaLike.ts
    //   (CancelGroupAlbumMediaLike -> cancel_group_album_media_like)。
    //   参数 {group_id, album_id, batch_id, lloc?};group_id 序列化为字符串。
    async fn cancel_group_album_media_like(
        &self,
        group: Uin,
        album_id: &str,
        batch_id: &str,
        lloc: Option<&str>,
    ) -> Result<()> {
        let mut params = Map::new();
        params.insert("group_id".into(), json!(group.0.to_string()));
        params.insert("album_id".into(), json!(album_id));
        params.insert("batch_id".into(), json!(batch_id));
        if let Some(l) = lloc {
            params.insert("lloc".into(), json!(l));
        }
        self.call("cancel_group_album_media_like", Value::Object(params)).await?;
        Ok(())
    }

    // ENDPOINT: NapCat action/extends/ClickInlineKeyboardButton.ts (click_inline_keyboard_button)。
    //   参数 {group_id:String, bot_appid:String, button_id:String, callback_data:String,
    //   msg_seq:String};resp 点击结果,原样透传。
    async fn click_inline_keyboard_button(
        &self,
        group: Uin,
        bot_appid: &str,
        button_id: &str,
        callback_data: &str,
        msg_seq: &str,
    ) -> Result<Value> {
        self.call(
            "click_inline_keyboard_button",
            json!({
                "group_id": group.0.to_string(),
                "bot_appid": bot_appid,
                "button_id": button_id,
                "callback_data": callback_data,
                "msg_seq": msg_seq,
            }),
        )
        .await
    }

    // ENDPOINT: NapCat router.ts (GoCQHTTP_GetWordSlices: '.get_word_slices')。
    //   go-cqhttp 隐藏 API 语义:参数 {content};resp {slices:[String]}。
    async fn get_word_slices(&self, content: &str) -> Result<Vec<String>> {
        let data = self.call(".get_word_slices", json!({ "content": content })).await?;
        Ok(data
            .get("slices")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect())
    }

    // ENDPOINT: NapCat action/guild/GetGuildList.ts (get_guild_list);参数 {};resp null(占位)。
    async fn get_guild_list(&self) -> Result<Value> {
        self.call("get_guild_list", json!({})).await
    }

    // ENDPOINT: NapCat action/guild/GetGuildProfile.ts (-> get_guild_service_profile);
    //   参数 {};resp null(占位)。
    async fn get_guild_service_profile(&self) -> Result<Value> {
        self.call("get_guild_service_profile", json!({})).await
    }

    // ----- NapCat stream 文件家族 -----
    // 每个 NapCat stream 动作在一次 `call` 内是单条 JSON 请求/响应。下载类变体在服务端置
    // `useStream=true`,在同一个 echo 上发多帧(file_info → N×file_chunk → 最终响应);nagisa 的
    // 一次性 echo 关联 `call` 只在第一帧(file_info 包)解决,故 chunk 载荷是一个遗留限制,
    // 记录在 invoker/onebot.rs 的 trait 方法上。

    // ENDPOINT: NapCat action/stream/UploadFileStream.ts (upload_file_stream)。
    //   参数 union(只需 stream_id),原样透传;resp StreamPacket<StreamResult>。
    async fn upload_file_stream(&self, params: Value) -> Result<Value> {
        self.call("upload_file_stream", params).await
    }

    // ENDPOINT: NapCat action/stream/DownloadFileStream.ts (download_file_stream)。
    //   参数 {file?, file_id?, chunk_size?};resp 首个 file_info 包(stream 遗留)。
    async fn download_file_stream(
        &self,
        file: Option<&str>,
        file_id: Option<&str>,
        chunk_size: Option<i64>,
    ) -> Result<Value> {
        let mut params = Map::new();
        if let Some(f) = file {
            params.insert("file".into(), json!(f));
        }
        if let Some(id) = file_id {
            params.insert("file_id".into(), json!(id));
        }
        if let Some(cs) = chunk_size {
            params.insert("chunk_size".into(), json!(cs));
        }
        self.call("download_file_stream", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/stream/DownloadFileRecordStream.ts (download_file_record_stream)。
    //   参数 {file?, file_id?, chunk_size?, out_format?};resp 首个 file_info 包(stream 遗留)。
    async fn download_file_record_stream(
        &self,
        file: Option<&str>,
        file_id: Option<&str>,
        chunk_size: Option<i64>,
        out_format: Option<&str>,
    ) -> Result<Value> {
        let mut params = Map::new();
        if let Some(f) = file {
            params.insert("file".into(), json!(f));
        }
        if let Some(id) = file_id {
            params.insert("file_id".into(), json!(id));
        }
        if let Some(cs) = chunk_size {
            params.insert("chunk_size".into(), json!(cs));
        }
        if let Some(of) = out_format {
            params.insert("out_format".into(), json!(of));
        }
        self.call("download_file_record_stream", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/stream/DownloadFileImageStream.ts (download_file_image_stream)。
    //   参数 {file?, file_id?, chunk_size?};resp 首个 file_info 包(+ width/height)(stream 遗留)。
    async fn download_file_image_stream(
        &self,
        file: Option<&str>,
        file_id: Option<&str>,
        chunk_size: Option<i64>,
    ) -> Result<Value> {
        let mut params = Map::new();
        if let Some(f) = file {
            params.insert("file".into(), json!(f));
        }
        if let Some(id) = file_id {
            params.insert("file_id".into(), json!(id));
        }
        if let Some(cs) = chunk_size {
            params.insert("chunk_size".into(), json!(cs));
        }
        self.call("download_file_image_stream", Value::Object(params)).await
    }

    // ENDPOINT: NapCat action/stream/CleanStreamTempFile.ts (clean_stream_temp_file)。
    //   参数 {};resp null。
    async fn clean_stream_temp_file(&self) -> Result<()> {
        self.call("clean_stream_temp_file", json!({})).await?;
        Ok(())
    }

    // ===== LLOneBot 专属 =====
    // wire 名 + 参数键已于 2026-06-04 对照 LLOneBot/LLOneBot
    // src/onebot11/action/types.ts + src/onebot11/action/llbot/** 核实。

    // ENDPOINT: LLOneBot action/llbot/user/SetFriendCategory.ts (set_friend_category)。
    //   参数 {user_id, category_id}。
    async fn set_friend_category(&self, user: Uin, category_id: i64) -> Result<()> {
        self.call(
            "set_friend_category",
            json!({ "user_id": user.0, "category_id": category_id }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: LLOneBot action/llbot/group/SetGroupMsgMask.ts (set_group_msg_mask)。
    //   参数 {group_id, mask}。
    async fn set_group_msg_mask(&self, group: Uin, mask: i64) -> Result<()> {
        self.call("set_group_msg_mask", json!({ "group_id": group.0, "mask": mask })).await?;
        Ok(())
    }

    // ENDPOINT: LLOneBot action/llbot/user/GetProfileLikeMe.ts (get_profile_like_me)。
    //   参数 {start, count}(共有 get_profile_like 的分页变体)。
    async fn get_profile_like_me(&self, start: i64, count: u32) -> Result<Value> {
        self.call("get_profile_like_me", json!({ "start": start, "count": count })).await
    }

    // ENDPOINT: LLOneBot action/llbot/user/GetQQAvatar.ts (get_qq_avatar)。
    //   参数 {user_id?}/{group_id?}(至少一个);resp {url}。
    async fn get_qq_avatar(&self, user: Option<Uin>, group: Option<Uin>) -> Result<String> {
        let mut params = Map::new();
        if let Some(u) = user {
            params.insert("user_id".into(), json!(u.0));
        }
        if let Some(g) = group {
            params.insert("group_id".into(), json!(g.0));
        }
        let data = self.call("get_qq_avatar", Value::Object(params)).await?;
        Ok(data
            .as_str()
            .map(String::from)
            .or_else(|| data_str(&data, "url"))
            .unwrap_or_default())
    }

    // ENDPOINT: LLOneBot action/llbot/msg/GetRecommendFace.ts (get_recommend_face)。
    //   参数 {word};resp {url:[String]}。
    async fn get_recommend_face(&self, word: &str) -> Result<Vec<String>> {
        let data = self.call("get_recommend_face", json!({ "word": word })).await?;
        Ok(data
            .get("url")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect())
    }

    // ENDPOINT: LLOneBot action/llbot/msg/VoiceMsg2Text.ts
    //   (VoiceMsg2Text = 'voice_msg_to_text'——不是 voice_msg_2_text)。
    //   参数 {message_id};resp {text}。
    async fn voice_msg_to_text(&self, msg: &MessageId) -> Result<String> {
        let mid = onebot_id_of(msg)?;
        let data = self.call("voice_msg_to_text", json!({ "message_id": mid })).await?;
        Ok(data_str(&data, "text").unwrap_or_default())
    }

    // ENDPOINT: LLOneBot action/llbot/system/ScanQRCode.ts (scan_qrcode)。
    //   参数 {file};resp [{text}]。
    async fn scan_qrcode(&self, file: &str) -> Result<Vec<String>> {
        let data = self.call("scan_qrcode", json!({ "file": file })).await?;
        Ok(data
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| data_str(&v, "text"))
            .collect())
    }

    // ENDPOINT: LLOneBot action/llbot/group/BatchDeleteGroupMember.ts
    //   (batch_delete_group_member)。参数 {group_id, user_ids:[Uin]}——wire 字段是
    //   `user_ids`(复数),且没有 reject_add_request(已对照源码核实;与 NapCat
    //   set_group_kick_members 用 user_id + reject_add_request 不同)。
    async fn batch_delete_group_member(&self, group: Uin, users: &[Uin]) -> Result<()> {
        let ids: Vec<i64> = users.iter().map(|u| u.0).collect();
        self.call(
            "batch_delete_group_member",
            json!({ "group_id": group.0, "user_ids": ids }),
        )
        .await?;
        Ok(())
    }

    // ----- LLOneBot 群相册 (album) —— group_id 序列化为字符串 -----

    // ENDPOINT: LLOneBot action/llbot/group/GroupAlbum/CreateGroupAlbum.ts (create_group_album)。
    //   参数 {group_id, name, desc?}。
    async fn create_group_album(
        &self,
        group: Uin,
        name: &str,
        desc: Option<&str>,
    ) -> Result<Value> {
        let mut params = Map::new();
        params.insert("group_id".into(), json!(group.0.to_string()));
        params.insert("name".into(), json!(name));
        if let Some(d) = desc {
            params.insert("desc".into(), json!(d));
        }
        self.call("create_group_album", Value::Object(params)).await
    }

    // ENDPOINT: LLOneBot action/llbot/group/GroupAlbum/DeleteGroupAlbum.ts (delete_group_album)。
    //   参数 {group_id, album_id}。
    async fn delete_group_album(&self, group: Uin, album_id: &str) -> Result<()> {
        self.call(
            "delete_group_album",
            json!({ "group_id": group.0.to_string(), "album_id": album_id }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: LLOneBot action/llbot/group/GroupAlbum/GetGroupAlbumList.ts (get_group_album_list)。
    //   参数 {group_id}。
    async fn get_group_album_list(&self, group: Uin) -> Result<Value> {
        self.call("get_group_album_list", json!({ "group_id": group.0.to_string() })).await
    }

    // ENDPOINT: LLOneBot action/llbot/group/GroupAlbum/UploadGroupAlbum.ts (upload_group_album)。
    //   参数 {group_id, album_id, files:[String]}。
    async fn upload_group_album(
        &self,
        group: Uin,
        album_id: &str,
        files: &[String],
    ) -> Result<Value> {
        self.call(
            "upload_group_album",
            json!({ "group_id": group.0.to_string(), "album_id": album_id, "files": files }),
        )
        .await
    }

    // ----- LLOneBot 闪传文件 (flash file) -----

    // ENDPOINT: LLOneBot action/llbot/file/UploadFlashFile.ts (upload_flash_file)。
    //   参数 {title?, paths:[String]}。
    async fn upload_flash_file(&self, title: Option<&str>, paths: &[String]) -> Result<Value> {
        let mut params = Map::new();
        if let Some(t) = title {
            params.insert("title".into(), json!(t));
        }
        params.insert("paths".into(), json!(paths));
        self.call("upload_flash_file", Value::Object(params)).await
    }

    // ENDPOINT: LLOneBot action/llbot/file/DownloadFlashFile.ts (download_flash_file)。
    //   参数 {file_set_id?, share_link?}(至少一个)。
    async fn download_flash_file(
        &self,
        file_set_id: Option<&str>,
        share_link: Option<&str>,
    ) -> Result<Value> {
        let mut params = Map::new();
        if let Some(id) = file_set_id {
            params.insert("file_set_id".into(), json!(id));
        }
        if let Some(link) = share_link {
            params.insert("share_link".into(), json!(link));
        }
        self.call("download_flash_file", Value::Object(params)).await
    }

    // ENDPOINT: LLOneBot action/llbot/file/ReShareFlashFile.ts (reshare_flash_file)。
    //   参数 {file_set_id}。
    async fn reshare_flash_file(&self, file_set_id: &str) -> Result<Value> {
        self.call("reshare_flash_file", json!({ "file_set_id": file_set_id })).await
    }

    // ENDPOINT: LLOneBot action/llbot/file/GetFlashFileInfo.ts (get_flash_file_info)。
    //   参数 {file_set_id?, share_link?}(至少一个)。
    async fn get_flash_file_info(
        &self,
        file_set_id: Option<&str>,
        share_link: Option<&str>,
    ) -> Result<Value> {
        let mut params = Map::new();
        if let Some(id) = file_set_id {
            params.insert("file_set_id".into(), json!(id));
        }
        if let Some(link) = share_link {
            params.insert("share_link".into(), json!(link));
        }
        self.call("get_flash_file_info", Value::Object(params)).await
    }

    // ----- LLOneBot 原始封包 / 配置 / 调试 -----

    // ENDPOINT: LLOneBot action/llbot/system/SendPacket.ts (send_pb)。
    //   参数 {cmd, hex};resp 是原始回复(Value 透传)。
    async fn send_pb(&self, cmd: &str, hex: &str) -> Result<Value> {
        self.call("send_pb", json!({ "cmd": cmd, "hex": hex })).await
    }

    // ENDPOINT: LLOneBot action/llbot/system/GetConfigAction.ts (get_config)。
    //   参数 {};resp 是完整配置对象(Value 透传)。
    async fn get_config(&self) -> Result<Value> {
        self.call("get_config", Value::Object(Map::new())).await
    }

    // ENDPOINT: LLOneBot action/llbot/system/SetConfigAction.ts (set_config)。
    //   参数 = 配置对象(Value 透传);resp null。
    async fn set_config(&self, config: Value) -> Result<()> {
        self.call("set_config", config).await?;
        Ok(())
    }

    // ENDPOINT: LLOneBot action/llbot/system/Debug.ts (llonebot_debug)。
    //   危险的调试动作;参数 = 调试载荷(Value 透传)。
    async fn llonebot_debug(&self, payload: Value) -> Result<Value> {
        self.call("llonebot_debug", payload).await
    }

    // ENDPOINT: LLOneBot action/llbot/system/GetEvent.ts (get_event)。
    //   参数 {};resp = 排队的 OneBot 事件对象数组(动作的 `data`)。
    //   每条都过共享的 `decode_event` 管线,使长轮询事件与 webhook / forward-WS 完全一致。
    //   非数组 / 空 `data` 产出空 Vec(绝不 panic;缺字段在 decode_event 内降级为 Raw)。
    async fn get_event(&self) -> Result<Vec<Event>> {
        let data = self.call("get_event", Value::Object(Map::new())).await?;
        Ok(decode_event_batch(data))
    }

    // ===== Lagrange 专属 =====

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Ability/UploadImageOperation.cs
    //   (upload_image) (https://github.com/LagrangeDev/Lagrange.Core)。
    //   参数 {file}(path/url/base64);resp string(url,data 即 url 字符串)。
    async fn upload_image(&self, file: &str) -> Result<String> {
        let data = self.call("upload_image", json!({ "file": file })).await?;
        Ok(data
            .as_str()
            .map(String::from)
            .or_else(|| data_str(&data, "url"))
            .unwrap_or_default())
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Generic/FetchMFaceKeyOperation.cs
    //   (fetch_mface_key) (https://github.com/LagrangeDev/Lagrange.Core)。
    //   参数 {emoji_ids:[String]};resp [String](key 数组,data 即该数组)。
    async fn fetch_mface_key(&self, emoji_ids: &[String]) -> Result<Vec<String>> {
        let data = self
            .call("fetch_mface_key", json!({ "emoji_ids": emoji_ids }))
            .await?;
        Ok(data
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect())
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Message/GetMusicArkOperation.cs
    //   (get_music_ark) (https://github.com/LagrangeDev/Lagrange.Core)。
    //   参数 {title, desc, jumpUrl, musicUrl, source_icon, tag, preview, sourceMsgId};
    //   resp string(已签名的 ark JSON)。参数是自定义音乐元数据字段
    //   (不是 type/id 模式——Lagrange 经 docs.qq.com 签名自定义 ark)。
    async fn get_music_ark(&self, params: Value) -> Result<Value> {
        self.call("get_music_ark", params).await
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Group/SetGroupBotOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core)。
    //   wire action_name 是 `set_group_bot_status`(SetGroupBotOperation.cs 里原文
    //   [Operation("set_group_bot_status")]);参数 {group_id, bot_id, enable}(OneBotSetGroupBot:
    //   GroupId/BotId/Enable;enable 在 wire 上是 uint,故发 1/0)。
    async fn set_group_bot_status(&self, group: Uin, bot_id: Uin, enable: bool) -> Result<()> {
        self.call(
            "set_group_bot_status",
            json!({ "group_id": group.0, "bot_id": bot_id.0, "enable": if enable { 1 } else { 0 } }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Group/SetGroupBothdOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core)。
    //   wire action_name `send_group_bot_callback`;参数
    //   {group_id, bot_id, data_1, data_2}(OneBotSetGroupBothd: GroupId/BotId/Data_1/Data_2)——
    //   按钮回调载荷是两个字符串,不是单个 `bot_appid`/`data` 对。
    async fn send_group_bot_callback(
        &self,
        group: Uin,
        bot_id: Uin,
        data_1: &str,
        data_2: &str,
    ) -> Result<()> {
        self.call(
            "send_group_bot_callback",
            json!({ "group_id": group.0, "bot_id": bot_id.0, "data_1": data_1, "data_2": data_2 }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Generic/FriendJoinEmojiChainOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core)。
    //   wire action_name 带前导点 `.join_friend_emoji_chain`;
    //   参数 {message_id, user_id, emoji_id}(OneBotPrivateJoinEmojiChain)。
    async fn join_friend_emoji_chain(&self, user: Uin, emoji_id: i64, msg: &MessageId) -> Result<()> {
        let mid = onebot_id_of(msg)?;
        self.call(
            ".join_friend_emoji_chain",
            json!({ "message_id": mid, "user_id": user.0, "emoji_id": emoji_id }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Generic/GroupJoinEmojiChainOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core)。
    //   wire action_name 带前导点 `.join_group_emoji_chain`;
    //   参数 {message_id, group_id, emoji_id}(OneBotGroupJoinEmojiChain)。
    async fn join_group_emoji_chain(&self, group: Uin, emoji_id: i64, msg: &MessageId) -> Result<()> {
        let mid = onebot_id_of(msg)?;
        self.call(
            ".join_group_emoji_chain",
            json!({ "message_id": mid, "group_id": group.0, "emoji_id": emoji_id }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Generic/SendPacketOperation.cs
    //   (.send_packet) (https://github.com/LagrangeDev/Lagrange.Core)。
    //   wire action_name 带前导点 `.send_packet`(区别于 NapCat 不带点的 `send_packet`);
    //   参数 {cmd, data, rsp:bool};resp 原样透传。方法名带 `lagrange_` 前缀,以便在统一
    //   `Actions` 面上与 `send_packet` 区分。
    async fn lagrange_send_packet(&self, cmd: &str, data: &str, rsp: bool) -> Result<Value> {
        self.call(".send_packet", json!({ "cmd": cmd, "data": data, "rsp": rsp })).await
    }
}
