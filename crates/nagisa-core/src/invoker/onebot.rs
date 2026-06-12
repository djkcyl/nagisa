//! OneBot 协议**独有**动作(Milky 无对应),含 OneBot v11 官方动作与各 OneBot 厂商
//! (NapCat / LLOneBot / Lagrange)的私有/扩展动作——共 145 个方法,三个 vendor 的动作并入本 trait。
//!
//! 在 Milky adapter 上默认返回 `Unsupported`;OneBot adapter override 真实实现。
//! 见 [`ActionInvoker`](super::ActionInvoker)(两协议通用)与 [`Actions`](super::Actions)(组合)。
//!
//! `// ===== <vendor> 专属 =====` 分节注释标出各动作归属:OneBot v11 官方 + ≥2 厂商共名
//! 的动作在前,其后是单厂商私有的 NapCat / LLOneBot / Lagrange 三节。所有方法都带默认实现
//! (返回 `unsupported`),不支持该动作的实现端零样板。
use super::unsupported;
use async_trait::async_trait;
use nagisa_types::prelude::*;
use nagisa_types::segment::ForwardNode;
use serde_json::Value;

/// OneBot 独有动作(含 OneBot v11 官方 + NapCat / LLOneBot / Lagrange 厂商扩展)。
/// 默认实现全为 `Unsupported`;OneBot adapter override 真实实现,Milky 端经空 impl 全走默认。
#[async_trait]
pub trait OneBotActions: Send + Sync + 'static {
    /// 协议端运行状态。
    async fn get_status(&self) -> Result<ImplStatus> {
        Err(unsupported("get_status"))
    }
    /// 重启协议端实现。
    async fn set_restart(&self, _delay_ms: u32) -> Result<()> {
        Err(unsupported("set_restart"))
    }
    async fn clean_cache(&self) -> Result<()> {
        Err(unsupported("clean_cache"))
    }
    async fn can_send_image(&self) -> Result<bool> {
        Err(unsupported("can_send_image"))
    }
    async fn can_send_record(&self) -> Result<bool> {
        Err(unsupported("can_send_record"))
    }
    /// 群荣誉信息（typed `HonorList`；`raw` 字段保留原始 data 供前向兼容）。
    async fn get_group_honor_info(&self, _group: Uin, _kind: HonorKind) -> Result<HonorList> {
        Err(unsupported("get_group_honor_info"))
    }
    /// 取语音文件本地路径(按 `out_format` 转码)。
    async fn get_record(&self, _file: &str, _out_format: &str) -> Result<String> {
        Err(unsupported("get_record"))
    }
    /// 取图片文件本地路径。
    async fn get_image(&self, _file: &str) -> Result<String> {
        Err(unsupported("get_image"))
    }
    /// 设置好友备注。
    async fn set_friend_remark(&self, _user: Uin, _remark: &str) -> Result<()> {
        Err(unsupported("set_friend_remark"))
    }
    /// 对单条消息设置/取消表情回应（私聊也可用）。
    /// 区别于群专属的 `send_reaction`/`set_group_reaction`（ActionInvoker::send_reaction）。
    async fn set_msg_reaction(&self, _msg: &MessageId, _emoji_id: &str, _set: bool) -> Result<()> {
        Err(unsupported("set_msg_reaction"))
    }
    /// 取消对一条消息的表情回应（LLOneBot 私有动作，语义等价于 `set_msg_reaction(set=false)`，
    /// 但作为显式 wire 动作 `unset_msg_emoji_like` 单独提供）。
    async fn unset_msg_emoji_like(&self, _msg: &MessageId, _emoji_id: &str) -> Result<()> {
        Err(unsupported("unset_msg_emoji_like"))
    }
    /// 协议端版本信息（按需查询，区别于连接时的能力探测）。
    async fn get_version_info(&self) -> Result<VersionInfo> {
        Err(unsupported("get_version_info"))
    }
    /// 启用/停用群匿名聊天。
    async fn set_group_anonymous(&self, _group: Uin, _enable: bool) -> Result<()> {
        Err(unsupported("set_group_anonymous"))
    }
    /// 禁言匿名群成员 `duration` 秒。OneBot 接受两种定位形式：`flag` 字符串
    /// （取自 `Anonymous.flag`，主用例）或事件里完整的 `anonymous` 对象。传 `anonymous`
    /// 时优先发对象形（spec 允许 `flag` 与 `anonymous` 二选一）；否则回落到 `flag` 字符串。
    async fn set_group_anonymous_ban(
        &self,
        _group: Uin,
        _flag: &str,
        _anonymous: Option<&Anonymous>,
        _duration: u32,
    ) -> Result<()> {
        Err(unsupported("set_group_anonymous_ban"))
    }
    /// 对一个事件执行内联快速操作（`.handle_quick_operation`）。`context` 为原事件对象，
    /// `operation` 为操作对象（如 `{reply, at_sender, delete, kick, ban, ban_duration}` 或
    /// `{approve, reason}`）。低层逃生口；常规效果优先用一级动作。
    async fn handle_quick_operation(&self, _context: Value, _operation: Value) -> Result<()> {
        Err(unsupported(".handle_quick_operation"))
    }
    /// 图片 OCR（识别图片中的文字）。`image` 为路径/URL/base64。
    async fn ocr_image(&self, _image: &str) -> Result<Vec<OcrText>> {
        Err(unsupported("ocr_image"))
    }
    /// 群打卡/签到。
    async fn send_group_sign(&self, _group: Uin) -> Result<()> {
        Err(unsupported("set_group_sign"))
    }
    /// 查询某条消息某个表情的回应者列表。NapCat 与 LLOneBot 均注册此动作（≥2 端共有 → `OneBotActions`）。
    /// 两端参数命名分歧：NapCat 用 camelCase `emojiId`/`emojiType`，LLOneBot 用 snake `emoji_id`/`message_id`/`count`。
    /// adapter 同帧**双发**两套键名（多发的键被对端忽略），故无需按 vendor 分支即可命中两端。
    async fn fetch_emoji_like(&self, _msg: &MessageId, _emoji_id: &str, _emoji_type: i32) -> Result<Vec<EmojiLiker>> {
        Err(unsupported("fetch_emoji_like"))
    }
    /// 拉取账号收藏的自定义表情 URL 列表。NapCat 与 LLOneBot 均注册此动作（≥2 端共有 → `OneBotActions`）。
    async fn fetch_custom_face(&self, _count: u32) -> Result<Vec<String>> {
        Err(unsupported("fetch_custom_face"))
    }
    /// 发送群合并转发（独立动作；与 `send(Forward::Nodes)` 等效，提供显式入口）。
    /// 返回 [`ForwardSendResult`]：消息 `message_id` + Lagrange 合并转发引用
    /// `forward_id`(resId，端未回传则 `None`)。
    async fn send_group_forward(&self, _group: Uin, _nodes: &[ForwardNode]) -> Result<ForwardSendResult> {
        Err(unsupported("send_group_forward_msg"))
    }
    /// 发送私聊合并转发。返回 [`ForwardSendResult`]（含 `forward_id`/resId）。
    async fn send_private_forward(&self, _user: Uin, _nodes: &[ForwardNode]) -> Result<ForwardSendResult> {
        Err(unsupported("send_private_forward_msg"))
    }
    /// 发送场景无关的合并转发（Lagrange `send_forward_msg`）。返回 [`ForwardSendResult`]
    /// （含 `forward_id`/resId）。
    async fn send_forward(&self, _nodes: &[ForwardNode]) -> Result<ForwardSendResult> {
        Err(unsupported("send_forward_msg"))
    }

    /// 取富媒体下载鉴权密钥列表（private + group 各一枚）。
    /// 解码兼容两种返回形态：Lagrange/NapCat 的 `{rkeys:[...]}` 数组，以及 LLOneBot 的
    /// 扁平形态 `{private_key, group_key, expired_time}`（拆为 private/group 两枚 `Rkey`）。
    async fn get_rkey(&self) -> Result<Vec<Rkey>> {
        Err(unsupported("get_rkey"))
    }

    /// 取群内可用的 AI 语音音色列表（按类型分组）。`chat_type` 为聊天类型（通常 `"1"`）。
    async fn get_ai_characters(&self, _group: Uin, _chat_type: &str) -> Result<Vec<AiCharacterGroup>> {
        Err(unsupported("get_ai_characters"))
    }

    /// 用指定 AI 音色把文字合成为语音，返回录音文件 URL（不发送）。
    async fn get_ai_record(&self, _group: Uin, _character: &str, _text: &str, _chat_type: &str) -> Result<String> {
        Err(unsupported("get_ai_record"))
    }

    /// 用指定 AI 音色把文字合成为语音并直接发到群里，返回消息 id。
    async fn send_group_ai_record(
        &self,
        _group: Uin,
        _character: &str,
        _text: &str,
        _chat_type: &str,
    ) -> Result<MessageId> {
        Err(unsupported("send_group_ai_record"))
    }

    /// 设置账号在线状态（`status`/`ext_status`/`battery_status`）。
    async fn set_online_status(&self, _status: i32, _ext_status: i32, _battery_status: i32) -> Result<()> {
        Err(unsupported("set_online_status"))
    }

    /// 发送正在输入状态通知。
    async fn set_input_status(&self, _user: Uin, _event_type: i32) -> Result<()> {
        Err(unsupported("set_input_status"))
    }

    /// 获取点赞我的用户列表。
    async fn get_profile_like(&self) -> Result<Vec<ProfileLiker>> {
        Err(unsupported("get_profile_like"))
    }

    /// 获取带分类的好友列表（包含每个分组下的好友）。
    async fn get_friends_with_category(&self) -> Result<Vec<FriendCategoryList>> {
        Err(unsupported("get_friends_with_category"))
    }

    /// 设置群备注（仅自己可见的群名称）。
    async fn set_group_remark(&self, _group: Uin, _remark: &str) -> Result<()> {
        Err(unsupported("set_group_remark"))
    }

    /// 获取群内当前被禁言的成员列表。
    async fn get_group_shut_list(&self, _group: Uin) -> Result<Vec<MemberInfo>> {
        Err(unsupported("get_group_shut_list"))
    }

    /// 查询群「@全体成员」的剩余次数（群额度 + 个人额度）。
    async fn get_group_at_all_remain(&self, _group: Uin) -> Result<Value> {
        Err(unsupported("get_group_at_all_remain"))
    }

    // ===== 媒体 / 文件 / 转发（NapCat + LLOneBot 共有）=====

    /// 按 file_id 取回富媒体文件（下载 URL / 本地路径 / Base64）。
    async fn get_file(&self, _file_id: &str) -> Result<FileFetch> {
        Err(unsupported("get_file"))
    }

    /// 将一条消息单独转发给好友。
    async fn forward_friend_single_msg(&self, _user: Uin, _msg: &MessageId) -> Result<MessageId> {
        Err(unsupported("forward_friend_single_msg"))
    }

    /// 将一条消息单独转发到群。
    async fn forward_group_single_msg(&self, _group: Uin, _msg: &MessageId) -> Result<MessageId> {
        Err(unsupported("forward_group_single_msg"))
    }

    /// 获取群系统消息（待处理的入群邀请 / 加群申请）。NapCat 与 LLOneBot（go-cqhttp 兼容）
    /// 均注册此动作（≥2 端共有 → `OneBotActions`）。
    async fn get_group_system_msg(&self) -> Result<Value> {
        Err(unsupported("get_group_system_msg"))
    }

    /// 获取官方机器人账号的 UIN 范围（用于判定某账号是否为官方 bot）。NapCat 与 LLOneBot
    /// 均注册此动作（≥2 端共有 → `OneBotActions`）。
    async fn get_robot_uin_range(&self) -> Result<Value> {
        Err(unsupported("get_robot_uin_range"))
    }

    /// 获取群相册内媒体列表（支持分页，`attach_info` 由上次返回结果带入）。NapCat 与 LLOneBot
    /// 各自私有但共用同一 wire 名 `get_group_album_media_list`、同一签名（≥2 端共名 → `OneBotActions`）。
    async fn get_group_album_media_list(
        &self,
        _group: Uin,
        _album_id: &str,
        _attach_info: Option<&str>,
    ) -> Result<Value> {
        Err(unsupported("get_group_album_media_list"))
    }

    /// 将群临时文件转存为永久文件（去除临时有效期）。同一逻辑动作在两端 wire 名不同：
    /// NapCat 为 `trans_group_file`，LLOneBot 为 `set_group_file_forever`——合并为单一
    /// `OneBotActions` 方法，adapter 按 vendor 选 wire 名并用 `call_alias` 互为回退。
    async fn set_group_file_forever(&self, _group: Uin, _file_id: &str) -> Result<()> {
        Err(unsupported("set_group_file_forever"))
    }

    /// 获取「可疑好友」添加请求列表（疑似风险账号的加好友申请）。NapCat 与 LLOneBot
    /// 均注册此动作（≥2 端共有 → `OneBotActions`）。
    async fn get_doubt_friends_add_request(&self, _count: u32) -> Result<Value> {
        Err(unsupported("get_doubt_friends_add_request"))
    }

    /// 处理一条「可疑好友」添加请求（`flag` 取自 `get_doubt_friends_add_request`）。
    /// NapCat 与 LLOneBot 均注册此动作（≥2 端共有 → `OneBotActions`）。两端形态分歧：
    /// LLOneBot 仅 `{flag}`（恒同意，无 approve），NapCat 含 `{flag, approve}`。
    /// 故统一签名补 `approve` 入参：LLOneBot 端忽略多发的 `approve` 键，语义不变。
    async fn set_doubt_friends_add_request(&self, _flag: &str, _approve: bool) -> Result<()> {
        Err(unsupported("set_doubt_friends_add_request"))
    }

    // ===== NapCat 专属 =====
    // 来源：NapNeko/NapCatQQ `packages/napcat-onebot/action/router.ts` 的 ActionName 映射。

    /// 设置自定义在线状态（自定义表情 + 文字）。
    async fn set_diy_online_status(&self, _face_id: i32, _face_type: i32, _wording: &str) -> Result<()> {
        Err(unsupported("set_diy_online_status"))
    }

    /// 查询指定用户当前在线状态（NapCat 专有，wire 名称有 `nc_` 前缀）。
    async fn get_user_status(&self, _user: Uin) -> Result<Value> {
        Err(unsupported("nc_get_user_status"))
    }

    /// 设置账号的个性签名（长昵称）。
    async fn set_self_longnick(&self, _longnick: &str) -> Result<()> {
        Err(unsupported("set_self_longnick"))
    }

    /// 获取单向好友列表（对方加了我但我未加对方）。
    async fn get_unidirectional_friend_list(&self) -> Result<Vec<FriendInfo>> {
        Err(unsupported("get_unidirectional_friend_list"))
    }

    /// 获取最近联系人列表。
    async fn get_recent_contact(&self, _count: u32) -> Result<Value> {
        Err(unsupported("get_recent_contact"))
    }

    /// 批量踢出多名群成员。`reject_add`=是否同时拒绝其再次加群。
    /// 注意：LLOneBot 用 `batch_delete_group_member`，故此动作归 NapCat 私有。
    async fn set_group_kick_members(&self, _group: Uin, _users: &[Uin], _reject_add: bool) -> Result<()> {
        Err(unsupported("set_group_kick_members"))
    }

    /// 获取群详细信息（比 `get_group_info` 更丰富，含全员禁言状态等）。
    async fn get_group_detail_info(&self, _group: Uin) -> Result<GroupInfo> {
        Err(unsupported("get_group_detail_info"))
    }

    /// 获取群扩展信息（gFlagExt / groupExtInfo 原始 JSON）。
    async fn get_group_info_ex(&self, _group: Uin) -> Result<Value> {
        Err(unsupported("get_group_info_ex"))
    }

    /// 获取被忽略的入群通知（邀请 / 申请）。
    async fn get_group_ignored_notifies(&self) -> Result<Value> {
        Err(unsupported("get_group_ignored_notifies"))
    }

    /// 设置群搜索选项（是否允许通过指纹 / 无码指纹搜索到群）。
    async fn set_group_search(
        &self,
        _group: Uin,
        _no_code_finger_open: Option<i32>,
        _no_finger_open: Option<i32>,
    ) -> Result<()> {
        Err(unsupported("set_group_search"))
    }

    /// 设置加群选项（加群方式 + 可选问答）。`add_type`：加群方式枚举。
    async fn set_group_add_option(
        &self,
        _group: Uin,
        _add_type: i32,
        _group_question: Option<&str>,
        _group_answer: Option<&str>,
    ) -> Result<()> {
        Err(unsupported("set_group_add_option"))
    }

    /// 设置群机器人加群选项（机器人成员开关 / 审核）。
    async fn set_group_robot_add_option(
        &self,
        _group: Uin,
        _robot_member_switch: Option<i32>,
        _robot_member_examine: Option<i32>,
    ) -> Result<()> {
        Err(unsupported("set_group_robot_add_option"))
    }

    /// 获取群组今日打卡列表。
    async fn get_group_signed_list(&self, _group: Uin) -> Result<Value> {
        Err(unsupported("get_group_signed_list"))
    }

    /// 提取语音消息（PTT）转写文本。
    async fn fetch_ptt_text(&self, _msg: &MessageId) -> Result<String> {
        Err(unsupported("fetch_ptt_text"))
    }

    /// 英文单词批量翻译为中文。
    async fn translate_en2zh(&self, _words: &[String]) -> Result<Vec<String>> {
        Err(unsupported("translate_en2zh"))
    }

    /// 获取富媒体下载 RKey（packet 服务侧形态，含 private/group rkey + 过期时间）。
    async fn get_rkey_server(&self) -> Result<Value> {
        Err(unsupported("get_rkey_server"))
    }

    /// 获取当前账号的 clientkey（用于部分 web 接口鉴权）。
    async fn get_clientkey(&self) -> Result<String> {
        Err(unsupported("get_clientkey"))
    }

    /// 生成小程序分享 Ark JSON（bili/weibo 模板或自定义模板）。
    async fn get_mini_app_ark(&self, _params: Value) -> Result<Value> {
        Err(unsupported("get_mini_app_ark"))
    }

    /// 生成联系人/群名片分享 Ark JSON（按 user_id 或 group_id）。
    /// 实际注册名为 `send_ark_share`（当前）+ 废弃别名 `ArkSharePeer`。
    /// adapter 用 `call_alias("send_ark_share", "ArkSharePeer", ..)`。
    async fn share_contact(
        &self,
        _user: Option<Uin>,
        _group: Option<Uin>,
        _phone_number: Option<&str>,
    ) -> Result<Value> {
        Err(unsupported("send_ark_share"))
    }

    /// 获取群相册列表（支持分页，`attach_info` 由上次返回结果带入）。
    async fn get_qun_album_list(&self, _group: Uin, _attach_info: Option<&str>) -> Result<Value> {
        Err(unsupported("get_qun_album_list"))
    }

    /// 上传图片到群相册。`file`：本地路径 / URL / Base64。
    async fn upload_image_to_qun_album(
        &self,
        _group: Uin,
        _album_id: &str,
        _album_name: &str,
        _file: &str,
    ) -> Result<Value> {
        Err(unsupported("upload_image_to_qun_album"))
    }

    /// 删除群相册中的一条媒体（`lloc` 为媒体 ID）。
    async fn del_group_album_media(&self, _group: Uin, _album_id: &str, _lloc: &str) -> Result<()> {
        Err(unsupported("del_group_album_media"))
    }

    /// 点赞群相册媒体（`batch_id` 为该批上传 id；`lloc` 省略则点赞整批）。
    async fn set_group_album_media_like(
        &self,
        _group: Uin,
        _album_id: &str,
        _batch_id: &str,
        _lloc: Option<&str>,
    ) -> Result<()> {
        Err(unsupported("set_group_album_media_like"))
    }

    /// 发表群相册评论。
    async fn do_group_album_comment(&self, _group: Uin, _album_id: &str, _lloc: &str, _content: &str) -> Result<Value> {
        Err(unsupported("do_group_album_comment"))
    }

    /// 创建闪传任务（上传一组文件，返回 fileset 信息）。
    async fn create_flash_task(
        &self,
        _files: &[String],
        _name: Option<&str>,
        _thumb_path: Option<&str>,
    ) -> Result<Value> {
        Err(unsupported("create_flash_task"))
    }

    /// 发送闪传消息（把一个 fileset 以闪传卡片发给好友 / 群）。
    async fn send_flash_msg(&self, _fileset_id: &str, _user: Option<Uin>, _group: Option<Uin>) -> Result<Value> {
        Err(unsupported("send_flash_msg"))
    }

    /// 获取 fileset 的分享链接。
    async fn get_share_link(&self, _fileset_id: &str) -> Result<Value> {
        Err(unsupported("get_share_link"))
    }

    /// 下载整个 fileset。
    async fn download_fileset(&self, _fileset_id: &str) -> Result<Value> {
        Err(unsupported("download_fileset"))
    }

    /// 获取 fileset 元信息。
    async fn get_fileset_info(&self, _fileset_id: &str) -> Result<Value> {
        Err(unsupported("get_fileset_info"))
    }

    /// 获取 fileset 内文件列表。
    async fn get_flash_file_list(&self, _fileset_id: &str) -> Result<Value> {
        Err(unsupported("get_flash_file_list"))
    }

    /// 获取 fileset 内某文件的下载 URL（按文件名或索引）。
    async fn get_flash_file_url(
        &self,
        _fileset_id: &str,
        _file_name: Option<&str>,
        _file_index: Option<i64>,
    ) -> Result<Value> {
        Err(unsupported("get_flash_file_url"))
    }

    /// 由分享码 / 分享链接解析出 fileset_id。wire 名为 `get_fileset_id`（非 `..._by_code`）。
    async fn get_fileset_id(&self, _share_code: &str) -> Result<Value> {
        Err(unsupported("get_fileset_id"))
    }

    /// 向好友发送在线文件。
    async fn send_online_file(&self, _user: Uin, _file_path: &str, _file_name: Option<&str>) -> Result<Value> {
        Err(unsupported("send_online_file"))
    }

    /// 向好友发送在线文件夹。
    async fn send_online_folder(&self, _user: Uin, _folder_path: &str, _folder_name: Option<&str>) -> Result<Value> {
        Err(unsupported("send_online_folder"))
    }

    /// 获取与某好友的在线文件消息列表。
    async fn get_online_file_msg(&self, _user: Uin) -> Result<Value> {
        Err(unsupported("get_online_file_msg"))
    }

    /// 接收一条在线文件。
    async fn receive_online_file(&self, _user: Uin, _msg_id: &str, _element_id: &str) -> Result<Value> {
        Err(unsupported("receive_online_file"))
    }

    /// 拒绝一条在线文件。
    async fn refuse_online_file(&self, _user: Uin, _msg_id: &str, _element_id: &str) -> Result<()> {
        Err(unsupported("refuse_online_file"))
    }

    /// 取消一条（自己发出的）在线文件。
    async fn cancel_online_file(&self, _user: Uin, _msg_id: &str) -> Result<()> {
        Err(unsupported("cancel_online_file"))
    }

    /// 将一条消息设为群待办。
    async fn set_group_todo(&self, _group: Uin, _msg: &MessageId) -> Result<()> {
        Err(unsupported("set_group_todo"))
    }

    /// 将一条群待办标记为已完成。
    async fn complete_group_todo(&self, _group: Uin, _msg: &MessageId) -> Result<()> {
        Err(unsupported("complete_group_todo"))
    }

    /// 取消一条群待办。
    async fn cancel_group_todo(&self, _group: Uin, _msg: &MessageId) -> Result<()> {
        Err(unsupported("cancel_group_todo"))
    }

    /// 创建收藏。`raw_data` 为原始内容，`brief` 为标题摘要。
    async fn create_collection(&self, _raw_data: &str, _brief: &str) -> Result<Value> {
        Err(unsupported("create_collection"))
    }

    /// 获取收藏列表。`category` 分类 ID，`count` 数量（wire 为字符串）。
    async fn get_collection_list(&self, _category: &str, _count: u32) -> Result<Value> {
        Err(unsupported("get_collection_list"))
    }

    /// 检测 URL 是否安全。resp `{level}`（1 安全 / 2 未知 / 3 危险）。
    async fn check_url_safely(&self, _url: &str) -> Result<Value> {
        Err(unsupported("check_url_safely"))
    }

    /// 获取其它在线客户端（手机 / PC / iPad 等）列表。
    async fn get_online_clients(&self) -> Result<Value> {
        Err(unsupported("get_online_clients"))
    }

    /// 下载 URL / Base64 到本地，返回本地路径（resp `{file}`）。
    async fn download_file(
        &self,
        _url: Option<&str>,
        _base64: Option<&str>,
        _name: Option<&str>,
        _headers: Option<Value>,
    ) -> Result<String> {
        Err(unsupported("download_file"))
    }

    /// 让 NapCat 进程退出（wire 名 `bot_exit`，class `BotExit`）。
    async fn bot_exit(&self) -> Result<()> {
        Err(unsupported("bot_exit"))
    }

    /// 将所有会话标记为已读（go-cqhttp 兼容，wire 名带前导下划线）。
    async fn mark_all_as_read(&self) -> Result<()> {
        Err(unsupported("_mark_all_as_read"))
    }

    /// 获取一条消息上某个表情回应的点赞者列表（`fetch_emoji_like` 的变体，字段命名更贴近
    /// OneBot 风格）。实测**仅 NapCat** 注册此 wire 名（LLOneBot 只有 `fetch_emoji_like`），
    /// 故为单厂商私有 → 归 NapCat 专属。
    async fn get_emoji_likes(&self, _msg: &MessageId, _emoji_id: &str, _emoji_type: i32, _count: u32) -> Result<Value> {
        Err(unsupported("get_emoji_likes"))
    }

    /// 获取自定义表情详情列表。
    async fn fetch_custom_face_detail(&self, _count: u32) -> Result<Value> {
        Err(unsupported("fetch_custom_face_detail"))
    }

    /// 添加一个自定义表情。参数为联合体（file/emoji_id/package_id/...），故 `Value` 透传。
    async fn add_custom_face(&self, _params: Value) -> Result<Value> {
        Err(unsupported("add_custom_face"))
    }

    /// 删除自定义表情（按 res_id/id/ids/md5 之一）。参数联合体，`Value` 透传。
    async fn delete_custom_face(&self, _params: Value) -> Result<()> {
        Err(unsupported("delete_custom_face"))
    }

    /// 设置自定义表情描述。
    async fn set_custom_face_desc(&self, _emoji_id: &str, _res_id: &str, _md5: &str, _desc: &str) -> Result<()> {
        Err(unsupported("set_custom_face_desc"))
    }

    /// 获取当前账号机型显示信息。
    async fn get_model_show(&self, _model: Option<&str>) -> Result<Value> {
        Err(unsupported("_get_model_show"))
    }

    /// 设置当前账号机型显示（NapCat 侧为兼容 no-op，仍按 go-cqhttp 约定传 {model, model_show}）。
    async fn set_model_show(&self, _model: &str, _model_show: &str) -> Result<()> {
        Err(unsupported("_set_model_show"))
    }

    /// 获取群文件系统信息（文件数 / 上限 / 空间用量）。
    async fn get_group_file_system_info(&self, _group: Uin) -> Result<Value> {
        Err(unsupported("get_group_file_system_info"))
    }

    /// 获取 NapCat packet 服务（NTQQ 协议封包侧）状态。
    async fn nc_get_packet_status(&self) -> Result<Value> {
        Err(unsupported("nc_get_packet_status"))
    }

    /// 按会话精确标记私聊已读（go-cqhttp `mark_msg_as_read` 的私聊变体；部分端只认按消息标记）。
    /// 与群版共用同一 `getPeer` 逻辑：优先 `message_id`，否则按 `user_id` 解析会话。
    /// 这里以 `user_id` 入参（私聊语义），故只发 `{user_id}`。
    async fn mark_private_msg_as_read(&self, _user: Uin) -> Result<()> {
        Err(unsupported("mark_private_msg_as_read"))
    }

    /// 按会话精确标记群聊已读（go-cqhttp `mark_msg_as_read` 的群聊变体）。
    async fn mark_group_msg_as_read(&self, _group: Uin) -> Result<()> {
        Err(unsupported("mark_group_msg_as_read"))
    }

    /// 生成群名片分享 Ark JSON（ark 群名片，标准化 wire 名 `send_group_ark_share`，
    /// 含废弃别名 `ArkShareGroup`/`ShareGroupEx`）。返回 Ark JSON（字符串，按 `Value` 透传）。
    /// adapter 用 `call_alias("send_group_ark_share", "ArkShareGroup", ..)`。
    async fn send_group_ark_share(&self, _group: Uin) -> Result<Value> {
        Err(unsupported("send_group_ark_share"))
    }

    /// 获取富媒体下载 RKey（NapCat `nc_get_rkey` 变体，packet 侧 `FetchRkey`）。
    /// 与 OneBot 共有的 `get_rkey`（wire 名 `get_rkey`）不同：这里 wire 名为 `nc_get_rkey`。
    /// resp 为 Rkey 数组（`Type.Any`），按 `Value` 透传。
    async fn nc_get_rkey(&self) -> Result<Value> {
        Err(unsupported("nc_get_rkey"))
    }

    /// 发送 NapCat 原始封包（packet 侧 `sendPacket`）。`cmd` 为命令字，`data` 为十六进制数据，
    /// `rsp` 指示是否等待响应（默认 true）。resp 为响应十六进制字符串（不等待响应时为 null），透传。
    async fn send_packet(&self, _cmd: &str, _data: &str, _rsp: bool) -> Result<Value> {
        Err(unsupported("send_packet"))
    }

    /// 获取被忽略的加群请求列表（NapCat 私有，遍历 type==7 的单屏通知）。
    /// resp 为 `[{request_id, invitor_uin, invitor_nick?, group_id, message?, group_name?,
    /// checked, actor, requester_nick?}]`，按 `Value` 透传。
    ///
    /// 注：NapCat 的 `GetGroupAddRequest` 类实际注册到 wire 名 `get_group_ignore_add_request`
    /// （router.ts 中 **无** 独立的 `get_group_add_request` 字符串）；旧 go-cqhttp
    /// `get_group_add_request` 已废弃并重定向到 `get_group_system_msg`。故 [`Self::get_group_add_request`]
    /// 作为兼容别名指向同一端点，残留的废弃 wire 名无法独立表达。
    async fn get_group_ignore_add_request(&self) -> Result<Value> {
        Err(unsupported("get_group_ignore_add_request"))
    }

    /// 获取加群请求列表（NapCat 私有的 add-request 列表，兼容别名）。
    /// NapCat 端 **未** 注册独立的 `get_group_add_request` wire 名——其 `GetGroupAddRequest`
    /// 类绑定到 `get_group_ignore_add_request`；旧 go-cqhttp `get_group_add_request` 已废弃。
    /// 故此方法语义等价于 [`Self::get_group_ignore_add_request`]，adapter 用
    /// `call_alias("get_group_add_request", "get_group_ignore_add_request", ..)`：
    /// 若某端仍认旧名则命中，否则回退到现行私有名。
    async fn get_group_add_request(&self) -> Result<Value> {
        Err(unsupported("get_group_add_request"))
    }

    /// 取消群相册媒体点赞（`set_group_album_media_like` 的反向操作，isLike=false）。
    /// `batch_id` 为该批上传 id；`lloc` 省略则对整批取消点赞。
    async fn cancel_group_album_media_like(
        &self,
        _group: Uin,
        _album_id: &str,
        _batch_id: &str,
        _lloc: Option<&str>,
    ) -> Result<()> {
        Err(unsupported("cancel_group_album_media_like"))
    }

    /// 点击群机器人内联键盘按钮（触发回调）。`bot_appid` 机器人 AppID，`button_id` 按钮 ID，
    /// `callback_data` 回调数据，`msg_seq` 消息序列号（NapCat 默认 "10086"）。
    /// 与 Lagrange 的 `send_group_bot_callback`（data_1/data_2）wire 名 / 参数均不同，故归 NapCat 私有。
    async fn click_inline_keyboard_button(
        &self,
        _group: Uin,
        _bot_appid: &str,
        _button_id: &str,
        _callback_data: &str,
        _msg_seq: &str,
    ) -> Result<Value> {
        Err(unsupported("click_inline_keyboard_button"))
    }

    /// 中文分词（go-cqhttp 隐藏 API，wire 名带前导点 `.get_word_slices`）。返回分词结果数组。
    async fn get_word_slices(&self, _content: &str) -> Result<Vec<String>> {
        Err(unsupported(".get_word_slices"))
    }

    /// 获取已加入的频道列表（NapCat 频道 stub：handler 为空，恒返回 null）。
    /// 故 nagisa 仅提供 typed 入口并按 `Value` 透传（实测返回 `null`）。
    async fn get_guild_list(&self) -> Result<Value> {
        Err(unsupported("get_guild_list"))
    }

    /// 获取频道个人资料（NapCat 频道 stub：handler 为空，恒返回 null）。
    /// 故 nagisa 仅提供 typed 入口并按 `Value` 透传（实测返回 `null`）。
    async fn get_guild_service_profile(&self) -> Result<Value> {
        Err(unsupported("get_guild_service_profile"))
    }

    // ----- NapCat stream 文件家族（chunked file streaming）-----
    // 关于「流式」：NapCat 这些 stream 动作内部用 `useStream = true`，下载侧会就**同一**
    // request 连续 `req.send(...)` 多帧（`file_info` → N×`file_chunk` → 终态 `response`），
    // 各帧共用同一 echo。nagisa 的 `call` 走一次性 `echo`-相关的 oneshot：仅命中**首帧**即 resolve，
    // 余下分块帧（含真正的文件数据与终态）会因 `pending` 表已无该 echo 而被丢弃。详见各方法 doc。

    /// 分块上传文件流（每次调用 = 一次完整的 JSON 请求/响应，非 wire 流式）。
    ///
    /// 入参为联合体（仅 `stream_id` 必填；按交互阶段携带不同字段），故按 `Value` 透传：
    /// `{stream_id, chunk_data?(Base64), chunk_index?, total_chunks?, file_size?, expected_sha256?,
    /// is_complete?, filename?, reset?, verify_only?, file_retention?}`。典型流程为
    /// 「首帧带 total_chunks 建流 → 逐块 chunk_data+chunk_index → is_complete 收尾」，
    /// 每一步都是独立的请求/响应，故 nagisa 可正常 typed 暴露、无残留限制。
    /// 返回 StreamPacket（`{type, stream_id, status, received_chunks, total_chunks, file_path?,
    /// file_size?, sha256?}`），按 `Value` 透传。
    async fn upload_file_stream(&self, _params: Value) -> Result<Value> {
        Err(unsupported("upload_file_stream"))
    }

    /// 分块下载文件流。`file`：文件路径 / URL / 文件 ID（NapCat 会按 `file ||= file_id` 解析）；
    /// `chunk_size`：分块字节数（省略则 NapCat 默认 64KB）。
    ///
    /// 残留限制：NapCat 下载侧 `useStream=true`，就同一 echo 连发 `file_info` + N×`file_chunk` +
    /// 终态 `response` 多帧；nagisa 的一次性 echo 相关 `call` 仅 resolve **首帧**（`file_info`，含
    /// `{file_name, file_size, chunk_size}`），后续分块数据与终态帧因 `pending` 表无该 echo 被丢弃。
    /// 故本方法 typed 透传的 `Value` 是**首个 file_info 包**，完整分块数据无法经现有通道取回。
    async fn download_file_stream(
        &self,
        _file: Option<&str>,
        _file_id: Option<&str>,
        _chunk_size: Option<i64>,
    ) -> Result<Value> {
        Err(unsupported("download_file_stream"))
    }

    /// 分块下载语音文件流（`download_file_stream` 的语音变体，额外支持 `out_format` 转码）。
    /// `out_format`：目标音频格式（mp3/amr/wma/m4a/spx/ogg/wav/flac 之一），省略则不转码。
    ///
    /// 残留限制同 [`Self::download_file_stream`]：typed 透传的 `Value` 仅为**首个 file_info 包**
    /// （含 `{file_name, file_size, chunk_size, out_format?}`），完整分块数据无法经现有通道取回。
    async fn download_file_record_stream(
        &self,
        _file: Option<&str>,
        _file_id: Option<&str>,
        _chunk_size: Option<i64>,
        _out_format: Option<&str>,
    ) -> Result<Value> {
        Err(unsupported("download_file_record_stream"))
    }

    /// 分块下载图片文件流（`download_file_stream` 的图片变体，首帧额外含 `{width, height}`）。
    ///
    /// 残留限制同 [`Self::download_file_stream`]：typed 透传的 `Value` 仅为**首个 file_info 包**
    /// （含 `{file_name, file_size, chunk_size, width, height}`），完整分块数据无法经现有通道取回。
    async fn download_file_image_stream(
        &self,
        _file: Option<&str>,
        _file_id: Option<&str>,
        _chunk_size: Option<i64>,
    ) -> Result<Value> {
        Err(unsupported("download_file_image_stream"))
    }

    /// 清理流式传输产生的临时文件（删除 NapCatTempPath 下所有文件）。
    /// 本身即单次请求/响应 JSON（NapCat 内部尽力删除，恒返回成功），无残留限制。
    async fn clean_stream_temp_file(&self) -> Result<()> {
        Err(unsupported("clean_stream_temp_file"))
    }

    // ===== LLOneBot 专属 =====
    // 来源：LLOneBot/LLOneBot `src/onebot11/action/types.ts` 的 ActionName 枚举（及
    // `src/onebot11/action/llbot/**` 各 action 的 `payloadSchema`）。

    /// 设置好友所属分组（分类）。
    async fn set_friend_category(&self, _user: Uin, _category_id: i64) -> Result<()> {
        Err(unsupported("set_friend_category"))
    }

    /// 设置群消息接收掩码（消息通知设置）。`mask`：通知方式枚举。
    async fn set_group_msg_mask(&self, _group: Uin, _mask: i64) -> Result<()> {
        Err(unsupported("set_group_msg_mask"))
    }

    /// 获取点赞我的列表（分页：`start` 起始，`count` 数量，最大 30/页，start=-1 取全部）。
    /// 注意：区别于 shared 的 `get_profile_like`（无分页）。
    async fn get_profile_like_me(&self, _start: i64, _count: u32) -> Result<Value> {
        Err(unsupported("get_profile_like_me"))
    }

    /// 获取 QQ 头像 URL（按 `user_id` 取好友头像，或按 `group_id` 取群头像）。
    async fn get_qq_avatar(&self, _user: Option<Uin>, _group: Option<Uin>) -> Result<String> {
        Err(unsupported("get_qq_avatar"))
    }

    /// 获取某词对应的推荐表情图 URL 列表。
    async fn get_recommend_face(&self, _word: &str) -> Result<Vec<String>> {
        Err(unsupported("get_recommend_face"))
    }

    /// 将一条语音消息转写为文本。wire 名为 `voice_msg_to_text`（**非** `voice_msg_2_text`）。
    async fn voice_msg_to_text(&self, _msg: &MessageId) -> Result<String> {
        Err(unsupported("voice_msg_to_text"))
    }

    /// 识别一张图片中的二维码内容。
    async fn scan_qrcode(&self, _file: &str) -> Result<Vec<String>> {
        Err(unsupported("scan_qrcode"))
    }

    /// 批量踢出多名群成员。
    /// 注意：wire 字段名为 **`user_ids`（复数）**，且 LLOneBot **无** `reject_add_request`
    /// 字段（与 NapCat `set_group_kick_members` 的 `user_id`/`reject_add_request` 不同）。
    async fn batch_delete_group_member(&self, _group: Uin, _users: &[Uin]) -> Result<()> {
        Err(unsupported("batch_delete_group_member"))
    }

    /// 创建群相册。
    async fn create_group_album(&self, _group: Uin, _name: &str, _desc: Option<&str>) -> Result<Value> {
        Err(unsupported("create_group_album"))
    }

    /// 删除群相册。
    async fn delete_group_album(&self, _group: Uin, _album_id: &str) -> Result<()> {
        Err(unsupported("delete_group_album"))
    }

    /// 获取群相册列表。
    async fn get_group_album_list(&self, _group: Uin) -> Result<Value> {
        Err(unsupported("get_group_album_list"))
    }

    /// 上传媒体到群相册。
    async fn upload_group_album(&self, _group: Uin, _album_id: &str, _files: &[String]) -> Result<Value> {
        Err(unsupported("upload_group_album"))
    }

    /// 上传一组文件为闪传文件集。
    async fn upload_flash_file(&self, _title: Option<&str>, _paths: &[String]) -> Result<Value> {
        Err(unsupported("upload_flash_file"))
    }

    /// 下载闪传文件集（按 `file_set_id` 或解析 `share_link`，二者至少其一）。
    async fn download_flash_file(&self, _file_set_id: Option<&str>, _share_link: Option<&str>) -> Result<Value> {
        Err(unsupported("download_flash_file"))
    }

    /// 重新分享一个闪传文件集，返回新的分享信息。
    async fn reshare_flash_file(&self, _file_set_id: &str) -> Result<Value> {
        Err(unsupported("reshare_flash_file"))
    }

    /// 获取闪传文件集元信息（按 `file_set_id` 或解析 `share_link`，二者至少其一）。
    async fn get_flash_file_info(&self, _file_set_id: Option<&str>, _share_link: Option<&str>) -> Result<Value> {
        Err(unsupported("get_flash_file_info"))
    }

    /// 发送原始 protobuf 封包（私有底层入口）：`cmd` 为 NTQQ 命令名（如
    /// `"trpc.qqnt.xxx"`），`hex` 为序列化后 protobuf 的十六进制字符串。返回后端
    /// 回包（Value 透传——形态随 cmd 而异，无法 typed）。
    async fn send_pb(&self, _cmd: &str, _hex: &str) -> Result<Value> {
        Err(unsupported("send_pb"))
    }

    /// 读取 LLOneBot 当前运行配置（全量配置对象，Value 透传——结构随版本演进，
    /// 不 typed 以免有损）。
    async fn get_config(&self) -> Result<Value> {
        Err(unsupported("get_config"))
    }

    /// 写入 LLOneBot 运行配置。`config` 为配置对象（Value 透传，作为 params 直接下发，
    /// 与 [`get_config`](Self::get_config) 的返回结构对应）。
    async fn set_config(&self, _config: Value) -> Result<()> {
        Err(unsupported("set_config"))
    }

    /// **危险调试动作**：直接调用 LLOneBot 内部调试入口（可触达未公开/内部 API，
    /// 行为随版本变化，仅供排障）。`payload` 为调试请求体（Value 透传，原样下发）。
    async fn llonebot_debug(&self, _payload: Value) -> Result<Value> {
        Err(unsupported("llonebot_debug"))
    }

    /// 长轮询拉取一批排队的事件（LLOneBot 纯 HTTP 客户端用：无公网回调 / 无 WS 时
    /// 仍能收事件）。每次调用排空后端事件队列，返回**已解码**的统一
    /// [`Event`] 列表（队列为空时返回空 `Vec`）。
    ///
    /// LLOneBot 的 `get_event` 动作把缓冲区里的 OneBot 事件以数组（动作 `data`）一次
    /// 性返回；本方法逐条 `decode_event`，使其与 webhook / forward-WS 走同一解码语义。
    /// 用作事件源时，由 adapter 的拉取循环反复调用并把结果灌进 `sink`（见
    /// `OneBotTransport::LLOneBotHttp` / `http_post::run_llonebot_long_poll`）。
    ///
    ///
    /// 注意：纯长轮询无「拉取等待超时」入参（实现侧立即返回当前队列），故由调用方
    /// 决定轮询间隔；SSE `/_events` 流是其推送式的对应物（见 `http_post`）。
    async fn get_event(&self) -> Result<Vec<Event>> {
        Err(unsupported("get_event"))
    }

    // ===== Lagrange 专属 =====
    // 来源：LagrangeDev/Lagrange.Core `Lagrange.OneBot/Core/Operation/*` 的
    // `[Operation("...")]` 属性。

    /// 上传一张图片并返回其 CDN URL（Lagrange 特有；用于非消息场景的图床）。
    async fn upload_image(&self, _file: &str) -> Result<String> {
        Err(unsupported("upload_image"))
    }

    /// 批量取魔法表情（MFace）下载密钥。
    async fn fetch_mface_key(&self, _emoji_ids: &[String]) -> Result<Vec<String>> {
        Err(unsupported("fetch_mface_key"))
    }

    /// 构造签名后的音乐卡片 ark（Lagrange 通过 docs.qq.com 代签）。
    /// 注：Lagrange 的 `get_music_ark` 接受自定义字段（非 type/id），故以 `Value` 透传参数。
    async fn get_music_ark(&self, _params: Value) -> Result<Value> {
        Err(unsupported("get_music_ark"))
    }

    /// 在群里启用/停用某个群机器人（bot）。
    async fn set_group_bot_status(&self, _group: Uin, _bot_id: Uin, _enable: bool) -> Result<()> {
        Err(unsupported("set_group_bot_status"))
    }

    /// 触发群机器人按钮回调（inline keyboard callback）。
    async fn send_group_bot_callback(&self, _group: Uin, _bot_id: Uin, _data_1: &str, _data_2: &str) -> Result<()> {
        Err(unsupported("send_group_bot_callback"))
    }

    /// 跟随好友的某条消息加入“表情接龙”。
    async fn join_friend_emoji_chain(&self, _user: Uin, _emoji_id: i64, _msg: &MessageId) -> Result<()> {
        Err(unsupported(".join_friend_emoji_chain"))
    }

    /// 跟随群内某条消息加入“表情接龙”。
    async fn join_group_emoji_chain(&self, _group: Uin, _emoji_id: i64, _msg: &MessageId) -> Result<()> {
        Err(unsupported(".join_group_emoji_chain"))
    }

    /// 发送 Lagrange 原始底层封包（私有逃生口）。`cmd` 为命令字（如 `OidbSvcTrpcTcp.0x...`），
    /// `data` 为十六进制编码的请求体，`rsp` 指示是否等待响应。resp 为响应（十六进制
    /// 字符串或端实现相关结构）按 `Value` 透传——此通道**协议私有、无法在统一模型建模**，
    /// 故仅提供 typed 入口（取代裸 `call_raw(".send_packet", ..)`）而不解构返回。
    ///
    /// 方法名带 `lagrange_` 前缀以与 NapCat 的同 wire 名而不同语义的
    /// [`Self::send_packet`](Self::send_packet)（无前导点）在统一 `Actions` 面上消歧；
    /// 其 wire action_name 带前导点 `.send_packet`。
    async fn lagrange_send_packet(&self, _cmd: &str, _data: &str, _rsp: bool) -> Result<Value> {
        Err(unsupported(".send_packet"))
    }
}
