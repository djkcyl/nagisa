//! 响应映射:wire `{status,retcode,data}` 封包 + typed 结构体提取器。
use crate::wire::RespJson;
use nagisa_types::prelude::*;
use serde_json::Value;

/// 把 wire 的 `{status,retcode,data,message}` 映射成统一的 `Result<Value>`。
pub(crate) fn map_response(action: &str, resp: RespJson) -> Result<Value> {
    // OneBot status ∈ ok(0) / async(1) / failed。`async` = 已异步受理(如 set_restart):
    // 当成功处理。只有 `failed`(或非 ok/非 async 且 retcode 非零的 status)才算错误。
    //
    // 不把 `retcode == 1` 单独当成功:retcode 1 仅在 `status == "async"` 下才表示「已异步受理」
    // (上面那支已覆盖);否则一个不合规的 `{status:"failed", retcode:1}` 会被吞成成功(data 多为
    // null)。retcode 0 仍作为权威成功信号保留(兼容省略 status 字段的宽松实现)。
    let ok = resp.status == "ok" || resp.status == "async" || resp.retcode == 0;
    if ok {
        return Ok(resp.data);
    }
    // 未知动作 → retcode 404 → Unsupported。
    if resp.retcode == 404 {
        return Err(Error::Unsupported(action.to_string()));
    }
    let message = resp.message.unwrap_or_else(|| format!("retcode {}", resp.retcode));
    let kind = classify_retcode(resp.retcode);
    Err(Error::Action { retcode: resp.retcode, message, kind })
}

/// 启发式地把 retcode 归类成错误种类(绝非精确取值契约)。
pub(crate) fn classify_retcode(retcode: i64) -> ActionErrorKind {
    match retcode {
        404 | 1404 => ActionErrorKind::Unsupported,
        1400 | 10003 | 10004 => ActionErrorKind::BadParams,
        401 | 403 | 1403 => ActionErrorKind::AuthFailed,
        429 => ActionErrorKind::RateLimited,
        r if (100..600).contains(&r) => ActionErrorKind::Internal,
        _ => ActionErrorKind::Other,
    }
}

/// 把 echo 值字符串化以供查表。我方生成的 echo 恒为字符串,但服务端原样回任意 JSON,故保持宽松兜底。
pub(crate) fn echo_key(echo: &Value) -> String {
    match echo {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ===== 解包响应 data 的辅助函数 =====

pub(crate) fn data_i64(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}
pub(crate) fn data_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(String::from)
}

/// 从 `get_csrf_token`/`get_credentials` 响应里提取 CSRF token。兼容规范的 int32 形态与字符串
/// 形态而**不丢**数值:先试 `data_i64`(数字或数字字符串)并完整字符串化,再回退到非数字字符串。
/// 读 `csrf_token`(官方),再读 `token`(Lagrange/NapCat 别名)。缺省 → 空字符串。
pub(crate) fn csrf_token_from_data(data: &Value) -> String {
    data_i64(data, "csrf_token")
        .or_else(|| data_i64(data, "token"))
        .map(|t| t.to_string())
        .or_else(|| data_str(data, "csrf_token"))
        .or_else(|| data_str(data, "token"))
        .unwrap_or_default()
}

/// 从 `send_*_forward_msg` 响应构造 [`ForwardSendResult`]。`message_id` → 统一的 `MessageId`
/// (落在给定 peer 下);Lagrange 合并转发的引用 id(resId)一并作为 `forward_id` 携带。
/// resId 的 wire 键各端不同:Lagrange 返回 `forward_id`,部分构建用 `res_id`/`resid`。标准
/// OneBot 端缺省 → `None`(不算错误;消息照样发到了聊天)。
pub(crate) fn forward_send_result(data: &Value, peer: Peer) -> ForwardSendResult {
    let onebot_id = data_i64(data, "message_id").map(|v| v as i32);
    let forward_id = data_str(data, "forward_id")
        .or_else(|| data_str(data, "res_id"))
        .or_else(|| data_str(data, "resid"));
    ForwardSendResult { message_id: MessageId { peer, seq: 0, onebot_id }, forward_id }
}

/// 把 `get_rkey` 响应解码成 `Vec<Rkey>`,兼容两种形态:
///   - Lagrange/NapCat:`{rkeys:[{type, rkey, created_at, ttl}]}`(或裸数组)。
///   - LLOneBot 扁平形:`{private_key, group_key, expired_time}` → 每个存在的键产一个 `Rkey`,
///     把 `expired_time` 作为 `ttl` 携带。完整载荷保留在各自的 `raw` 里。
pub(crate) fn decode_rkey_list(data: &Value) -> Vec<Rkey> {
    // 首选形态:`rkeys` 数组(Lagrange/NapCat)或裸数组。
    let arr = data
        .get("rkeys")
        .and_then(Value::as_array)
        .or_else(|| data.as_array())
        .cloned();
    if let Some(arr) = arr {
        return arr
            .into_iter()
            .map(|v| Rkey {
                kind: data_str(&v, "type").unwrap_or_default(),
                rkey: data_str(&v, "rkey").unwrap_or_default(),
                // Lagrange wire 字段是 `created_at`;同时兼容 `create_time` 拼写。
                create_time: data_i64(&v, "created_at").or_else(|| data_i64(&v, "create_time")),
                // Lagrange wire 字段是 `ttl`;同时兼容 `ttl_seconds` 拼写。
                ttl: data_i64(&v, "ttl").or_else(|| data_i64(&v, "ttl_seconds")),
                raw: v,
            })
            .collect();
    }
    // LLOneBot 扁平形:{private_key, group_key, expired_time}。
    let expired_time = data_i64(data, "expired_time");
    let mut out = Vec::new();
    for (field, kind) in [("private_key", "private"), ("group_key", "group")] {
        if let Some(rkey) = data_str(data, field) {
            out.push(Rkey {
                kind: kind.to_string(),
                rkey,
                create_time: None,
                ttl: expired_time,
                raw: data.clone(),
            });
        }
    }
    out
}

/// 从统一 id 提取 OneBot wire 的 `message_id`(合成整型),错误形态与 `recall` 一致。
pub(crate) fn onebot_id_of(id: &MessageId) -> Result<i32> {
    id.onebot_id.ok_or_else(|| {
        Error::action_kind(ActionErrorKind::BadParams, "message id has no onebot_id")
    })
}

fn parse_sex(s: Option<&str>) -> Sex {
    match s {
        Some("male") => Sex::Male,
        Some("female") => Sex::Female,
        _ => Sex::Unknown,
    }
}
fn parse_role(s: Option<&str>) -> Role {
    match s {
        Some("owner") => Role::Owner,
        Some("admin") => Role::Admin,
        _ => Role::Member,
    }
}

pub(crate) fn group_info_from(v: &Value) -> GroupInfo {
    GroupInfo {
        group: Uin(data_i64(v, "group_id").unwrap_or(0)),
        name: data_str(v, "group_name").unwrap_or_default(),
        member_count: data_i64(v, "member_count").unwrap_or(0) as i32,
        max_member_count: data_i64(v, "max_member_count").unwrap_or(0) as i32,
        remark: data_str(v, "group_remark"),
        created_time: data_i64(v, "group_create_time"),
        description: data_str(v, "group_memo"),
        announcement: None,
        question: None,
        // LLOneBot 扩展：is_freeze/active_member_count/is_top/owner_id/shut_up_*。
        // 其余实现缺这些字段 → None（绝不 panic）。
        is_freeze: v.get("is_freeze").and_then(Value::as_bool),
        active_member_count: data_i64(v, "active_member_count").map(|x| x as i32),
        is_top: v.get("is_top").and_then(Value::as_bool),
        owner_id: data_i64(v, "owner_id").map(Uin),
        shut_up_all_time: data_i64(v, "shut_up_all_timestamp"),
        shut_up_me_time: data_i64(v, "shut_up_me_timestamp"),
        raw: v.clone(),
    }
}

pub(crate) fn member_info_from(v: &Value) -> MemberInfo {
    MemberInfo {
        user: Uin(data_i64(v, "user_id").unwrap_or(0)),
        group: Uin(data_i64(v, "group_id").unwrap_or(0)),
        nickname: data_str(v, "nickname").unwrap_or_default(),
        card: data_str(v, "card").unwrap_or_default(),
        title: data_str(v, "title").unwrap_or_default(),
        level: data_str(v, "level").and_then(|s| s.parse().ok()).unwrap_or(0),
        role: parse_role(v.get("role").and_then(|x| x.as_str())),
        sex: parse_sex(v.get("sex").and_then(|x| x.as_str())),
        age: data_i64(v, "age").map(|x| x as i32),
        join_time: data_i64(v, "join_time").unwrap_or(0),
        last_sent_time: data_i64(v, "last_sent_time"),
        mute_end_time: data_i64(v, "shut_up_timestamp").or_else(|| data_i64(v, "mute_end_time")),
        // OFFICIAL: api/public.md get_group_member_info —— area/unfriendly/title_expire_time/card_changeable。
        area: data_str(v, "area").filter(|s| !s.is_empty()),
        unfriendly: v.get("unfriendly").and_then(Value::as_bool),
        title_expire_time: data_i64(v, "title_expire_time"),
        card_changeable: v.get("card_changeable").and_then(Value::as_bool),
        // LLOneBot 扩展：qq_level/is_robot/qage。其余实现缺 → None（绝不 panic）。
        qq_level: data_i64(v, "qq_level").map(|x| x as i32),
        is_robot: v.get("is_robot").and_then(Value::as_bool),
        qage: data_i64(v, "qage").map(|x| x as i32),
        raw: v.clone(),
    }
}

/// 把 Lagrange `get_stranger_info.status` 子对象解析成 typed 的 `FriendStatus`。所有字段都是
/// 原样浮现的可选 int(不对 status 枚举做语义解读)——缺的 wire 字段 → `None`。
fn friend_status_from(v: &Value) -> FriendStatus {
    FriendStatus {
        status: data_i64(v, "status").map(|x| x as i32),
        ext_status: data_i64(v, "ext_status").map(|x| x as i32),
        battery_status: data_i64(v, "battery_status").map(|x| x as i32),
    }
}

/// 把一条 Lagrange `get_stranger_info.Business[]`(VIP / 业务徽章)解析成 typed 的 `Business`。
/// 宽松:缺字段 → `None`。
fn business_from(v: &Value) -> Business {
    Business {
        kind: data_i64(v, "type").map(|x| x as i32),
        name: data_str(v, "name").filter(|s| !s.is_empty()),
        level: data_i64(v, "level").map(|x| x as i32),
        icon: data_str(v, "icon").filter(|s| !s.is_empty()),
        is_pro: v.get("is_pro").and_then(Value::as_bool),
        is_year: v.get("is_year").and_then(Value::as_bool),
    }
}

/// 从 NapCat/LLOneBot 拆开的 `birthday_year/birthday_month/birthday_day` 三个 int 拼出
/// `YYYY-MM-DD` 生日。任一缺失/为 0 → `None`(全零生日是「未设」,不是真日期)。
fn birthday_from(v: &Value) -> Option<String> {
    let y = data_i64(v, "birthday_year")?;
    let m = data_i64(v, "birthday_month")?;
    let d = data_i64(v, "birthday_day")?;
    if y == 0 || m == 0 || d == 0 {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

pub(crate) fn user_info_from(v: &Value) -> UserInfo {
    UserInfo {
        user: Uin(data_i64(v, "user_id").unwrap_or(0)),
        nickname: data_str(v, "nickname").unwrap_or_default(),
        sex: parse_sex(v.get("sex").and_then(|x| x.as_str())),
        age: data_i64(v, "age").map(|a| a as i32),
        level: data_i64(v, "level").map(|l| l as i32),
        // Lagrange 把 QID 序列化在 `q_id` 下;同时兼容 `qid` 拼写。
        qid: data_str(v, "q_id").or_else(|| data_str(v, "qid")).filter(|s| !s.is_empty()),
        // Lagrange 把个性签名暴露为 `sign`。
        bio: data_str(v, "sign").or_else(|| data_str(v, "bio")).filter(|s| !s.is_empty()),
        country: data_str(v, "country").filter(|s| !s.is_empty()),
        city: data_str(v, "city").filter(|s| !s.is_empty()),
        school: data_str(v, "school").filter(|s| !s.is_empty()),
        remark: data_str(v, "remark").filter(|s| !s.is_empty()),
        // Lagrange 才有的丰富字段(其余实现省略 → None / 空 Vec)。
        status: v.get("status").filter(|s| s.is_object()).map(friend_status_from),
        business: v
            .get("Business")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().map(business_from).collect())
            .unwrap_or_default(),
        register_time: data_i64(v, "RegisterTime").or_else(|| data_i64(v, "register_time")),
        avatar: data_str(v, "avatar").filter(|s| !s.is_empty()),
        raw: v.clone(),
    }
}

pub(crate) fn friend_info_from(v: &Value) -> FriendInfo {
    FriendInfo {
        user: Uin(data_i64(v, "user_id").unwrap_or(0)),
        nickname: data_str(v, "nickname").unwrap_or_default(),
        sex: parse_sex(v.get("sex").and_then(|x| x.as_str())),
        remark: data_str(v, "remark").unwrap_or_default(),
        // Lagrange 把 QID 序列化在 `q_id` 下;同时兼容 `qid` 拼写。
        qid: data_str(v, "q_id").or_else(|| data_str(v, "qid")).filter(|s| !s.is_empty()),
        // NapCat/LLOneBot 把分组扁平化为 `categoryId`/`categroyName`(注意 NapCat 拼错的
        // `categroyName`);两种拼写都接受。
        category: friend_category_from(v),
        // Lagrange `group` → `{group_id, group_name}`(其余实现省略 → None)。
        group: v.get("group").filter(|g| g.is_object()).map(friend_group_from),
        // NapCat/LLOneBot 扩展(Lagrange 省略 → None)。
        birthday: birthday_from(v),
        // NapCat 把手机号驼峰为 `phoneNum`;裸 `-` 表示「未设」。
        phone: data_str(v, "phoneNum")
            .or_else(|| data_str(v, "phone"))
            .filter(|s| !s.is_empty() && s != "-"),
        email: data_str(v, "email").filter(|s| !s.is_empty()),
        login_days: data_i64(v, "login_days").map(|x| x as i32),
        // NapCat 驼峰为 `longNick`;LLOneBot 用 `long_nick`。
        long_nick: data_str(v, "longNick")
            .or_else(|| data_str(v, "long_nick"))
            .filter(|s| !s.is_empty()),
        raw: v.clone(),
    }
}

/// 把 Lagrange `friend.group` 子对象解析成 typed 的 `FriendGroup`。宽松:缺字段 → 0 / 空字符串。
fn friend_group_from(v: &Value) -> FriendGroup {
    FriendGroup {
        group_id: data_i64(v, "group_id").map(|x| x as i32).unwrap_or(0),
        group_name: data_str(v, "group_name").unwrap_or_default(),
    }
}

/// 把扁平化的 NapCat/LLOneBot 好友分组(`categoryId` + `categroyName`/`categoryName`)解析成
/// typed 的 `FriendCategory`。NapCat 的 wire 键是拼错的 `categroyName`;也接受正确拼写。
/// 没有分组 id 时返回 `None`。
fn friend_category_from(v: &Value) -> Option<FriendCategory> {
    let id = data_i64(v, "categoryId").or_else(|| data_i64(v, "category_id"))?;
    let name = data_str(v, "categroyName")
        .or_else(|| data_str(v, "categoryName"))
        .or_else(|| data_str(v, "category_name"))
        .unwrap_or_default();
    Some(FriendCategory { id: id as i32, name })
}

pub(crate) fn honor_member_from(v: &Value) -> HonorMember {
    HonorMember {
        user: Uin(data_i64(v, "user_id").unwrap_or(0)),
        nickname: data_str(v, "nickname").unwrap_or_default(),
        avatar: data_str(v, "avatar"),
        description: data_str(v, "description"),
        day_count: data_i64(v, "day_count").map(|x| x as i32),
        raw: v.clone(),
    }
}

/// 把 `_get_group_notice` 数组里的一条解析成 `Announcement`。
/// 形态(Lagrange/NapCat/LLOneBot):`{ notice_id, sender_id, publish_time,
/// message: { text, images: [{ id, height, width }] } }`。图片以 `id`(非 URL)暴露;
/// 我们尽力把它放进 `image_url`,并兼容未来实现可能给出的真正 `url` 字段。
pub(crate) fn announcement_from(group: Uin, v: &Value) -> Announcement {
    let msg = v.get("message");
    let content = msg
        .and_then(|m| m.get("text"))
        .and_then(|t| t.as_str())
        .map(String::from)
        .unwrap_or_default();
    let image_url = msg
        .and_then(|m| m.get("images"))
        .and_then(|i| i.as_array())
        .and_then(|arr| arr.first())
        .and_then(|img| {
            img.get("url")
                .and_then(|u| u.as_str())
                .or_else(|| img.get("id").and_then(|u| u.as_str()))
                .map(String::from)
        })
        .filter(|s| !s.is_empty());
    Announcement {
        id: data_str(v, "notice_id").unwrap_or_default(),
        group,
        sender: Uin(data_i64(v, "sender_id").unwrap_or(0)),
        content,
        // Lagrange/NapCat/LLOneBot 用 `publish_time`;同时兼容裸 `time`。
        time: data_i64(v, "publish_time").or_else(|| data_i64(v, "time")).unwrap_or(0),
        image_url,
        raw: v.clone(),
    }
}

/// 把 `get_essence_msg_list` 数组里的一条解析成 `EssenceMessage`。NapCat 带 `content`(段数组);
/// LLOneBot/Lagrange 省略(→ 空消息)。字段:`sender_id`、`sender_nick`、`operator_id`、
/// `operator_time`、`message_id`。
pub(crate) fn essence_from(group: Uin, v: &Value) -> EssenceMessage {
    let onebot_id = data_i64(v, "message_id").map(|m| m as i32);
    // 精华消息属于群;把该 peer 串进去,让嵌套 `reply` 段恢复真实会话,而非 `friend(0)` 兜底。
    let content = v
        .get("content")
        .map(|c| crate::decode::decode_message_value(c, Peer::group(group.0)))
        .unwrap_or_default();
    EssenceMessage {
        group,
        message_id: MessageId { peer: Peer::group(group.0), seq: 0, onebot_id },
        sender: Uin(data_i64(v, "sender_id").unwrap_or(0)),
        sender_nick: data_str(v, "sender_nick").unwrap_or_default(),
        operator: Uin(data_i64(v, "operator_id").unwrap_or(0)),
        operator_time: data_i64(v, "operator_time").unwrap_or(0),
        content,
        raw: v.clone(),
    }
}

/// 把群文件列表里的一条(`get_group_root_files` / `get_group_files_by_folder` → `files[]`)
/// 解析成 `FileMeta`。字段(Lagrange/NapCat):`file_id`、`file_name`、`file_size`、`busid`。
/// go-cqhttp 形态额外携带 `uploader`/`upload_time`/`dead_time`/`download_times`。
pub(crate) fn group_file_from(v: &Value) -> FileMeta {
    FileMeta {
        id: data_str(v, "file_id").unwrap_or_default(),
        name: data_str(v, "file_name").unwrap_or_default(),
        size: data_i64(v, "file_size").unwrap_or(0) as u64,
        hash: data_str(v, "file_hash").or_else(|| data_str(v, "hash")),
        busid: data_i64(v, "busid"),
        // go-cqhttp `get_group_root_files`/`get_group_files_by_folder` 文件富字段。
        uploader: data_i64(v, "uploader").map(Uin),
        upload_time: data_i64(v, "upload_time"),
        dead_time: data_i64(v, "dead_time"),
        download_times: data_i64(v, "download_times").map(|x| x as i32),
        // OneBot 群文件元素无父文件夹 ID 字段。
        parent_folder_id: None,
    }
}

/// 把群文件夹列表里的一条(`...→folders[]`)解析成 `GroupFolder`。
/// 字段(Lagrange/NapCat):`folder_id`、`folder_name`、`total_file_count`、`create_time`。
pub(crate) fn group_folder_from(v: &Value) -> GroupFolder {
    GroupFolder {
        id: data_str(v, "folder_id").unwrap_or_default(),
        name: data_str(v, "folder_name").unwrap_or_default(),
        file_count: data_i64(v, "total_file_count").map(|c| c as u32),
        create_time: data_i64(v, "create_time"),
        // OneBot 群文件夹元素无父文件夹/最后修改/创建者字段。
        parent_folder_id: None,
        last_modified_time: None,
        creator_id: None,
        raw: v.clone(),
    }
}

/// 把 `get_status` 响应里可选的 `stat` 子对象解析成 typed 的 `ImplStat`
/// (LLOneBot/Lagrange/go-cqhttp 扩展)。缺整块 → `None`;缺内层字段 → `None`(整块仍保留在
/// status 的 `raw` 里)。
pub(crate) fn impl_stat_from(data: &Value) -> Option<ImplStat> {
    let s = data.get("stat")?;
    Some(ImplStat {
        message_received: s.get("message_received").and_then(Value::as_i64),
        message_sent: s.get("message_sent").and_then(Value::as_i64),
    })
}
