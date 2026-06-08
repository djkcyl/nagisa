//! 协议标识 [`Protocol`] 与能力枚举 [`Capability`]，用于运行时能力探测：业务用
//! `Bot::supports(cap)` 问「当前后端能不能做这件事」，据此对不支持的端做降级。

/// 后端协议。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Protocol {
    OneBot11,
    Milky,
}

/// 可探测的能力。适配器据实现/版本回答 `Bot::supports`。
/// `#[non_exhaustive]`：新增能力不破坏下游 match。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Capability {
    GroupMute,
    GroupAdmin,
    GroupKick,
    HandleRequest,
    Essence,
    Announcement,
    PeerPin,
    Reaction,
    Forward,
    /// 群/私聊文件上传、下载、管理（upload_group_file / get_file 等）。
    FileOps,
    /// 戳一戳（nudge / poke）。
    Nudge,
    /// 点赞（get_profile_like / send_like）。
    ProfileLike,
    /// 读写自身资料（set_self_longnick 等）。
    SelfProfile,
    /// 拉取历史消息（get_message_history）。
    MessageHistory,
    /// 获取登录 Cookie / SKEY。
    Cookies,
    /// OCR 图片文字识别（ocr_image）。
    Ocr,
    /// AI 语音合成与音色列表（get_ai_characters / send_group_ai_record 等）。
    /// 仅 Lagrange.OneBot / NapCat / LLOneBot 支持；其他端返回 `false`。
    Ai,
}

impl Protocol {
    pub fn name(&self) -> &'static str {
        match self {
            Protocol::OneBot11 => "onebot11",
            Protocol::Milky => "milky",
        }
    }
}
