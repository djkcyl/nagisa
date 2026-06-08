//! 寻址基元：QQ 号 [`Uin`]、会话场景 [`Scene`]、对端寻址 [`Peer`]、不透明消息 ID
//! [`MessageId`]。事件、消息段、实体都靠这几个类型互相引用。
use serde::{Deserialize, Serialize};
use std::fmt;

/// QQ 号。用 newtype 包住 i64，防止与普通整数混淆。serde 透明序列化为内层 i64。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub struct Uin(pub i64);

/// 会话场景。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scene {
    Friend,
    Group,
    Temp,
}

/// 会话寻址：场景 + 对端 id（好友 QQ 号或群号）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Peer {
    pub scene: Scene,
    pub id: Uin,
}

/// 不透明消息标识：打包两协议各自的定位信息。业务把它当不透明句柄，不对其做算术/比较。
/// - Milky 用 `(peer.scene, peer.id, seq)` 三元组。
/// - OneBot 用 `onebot_id` 合成整数。
///
/// **固有限制（无 time 槽）**：不携带发送时间。Milky 的发送响应同时回传 `message_seq` 与
/// `time`，但本类型没有 time 字段，故发送返回路径有意丢弃 `time`（见 `nagisa-milky/src/actions.rs`
/// 的 `send`）。下游若需时间戳，应经 `get_message(id).time` 回查——这是统一类型不带时间槽的
/// 固有取舍，并非解码缺陷。
///
/// 派生 serde 以便持久化/缓存（OneBot 适配器的 message_id 映射缓存会用到）。
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct MessageId {
    pub peer: Peer,
    pub seq: i64,
    pub onebot_id: Option<i32>,
}

impl From<i64> for Uin {
    fn from(v: i64) -> Self {
        Uin(v)
    }
}

impl Uin {
    /// 是否像一个真实用户 QQ 号——启发式（≥ 10000），非类型保证。系统号/匿名等可能落在
    /// 此范围外。供「@-或-QQ 号」入参等场景做尽力校验，省得各处手写 `< 10000` 魔法数；
    /// 需要权威判断时仍应结合具体协议语境。
    pub fn is_user(self) -> bool {
        self.0 >= 10_000
    }
}

impl fmt::Display for Uin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Peer {
    pub fn group(id: impl Into<Uin>) -> Self {
        Peer { scene: Scene::Group, id: id.into() }
    }
    pub fn friend(id: impl Into<Uin>) -> Self {
        Peer { scene: Scene::Friend, id: id.into() }
    }
    pub fn temp(id: impl Into<Uin>) -> Self {
        Peer { scene: Scene::Temp, id: id.into() }
    }
    pub fn is_group(&self) -> bool {
        matches!(self.scene, Scene::Group)
    }
}

impl MessageId {
    /// Milky 风格：由 (peer, seq) 构造。
    pub fn from_seq(peer: Peer, seq: i64) -> Self {
        MessageId { peer, seq, onebot_id: None }
    }
}
