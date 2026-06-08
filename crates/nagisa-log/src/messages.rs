//! 最近消息缓存:按**稳定锚点**暂存最近的消息(发送者 + 内容),容量有界、FIFO 淘汰。
//!
//! 用途:撤回通知只带被撤消息的 id、不带内容,故要显示「撤回了什么」必须自己留一份最近消息。
//! 这也是「防撤回」的底子——业务可经 [`MessageStore::get`] 取回被撤消息做处理(转存/复读等);
//! 不过真正的防撤回(把被撤消息复读出来)应在业务层落库 + `Bot::get_message` 兜底,本缓存只为
//! 给**日志行**补上「撤回了什么」。
//!
//! 设计:键不是整个 [`MessageId`],而是 [`MsgKey`]——跨「原消息 / 撤回通知 / get_msg」**稳定**的
//! 锚点。原因:OneBot 把会话内 `message_seq` 落进 `MessageId.seq`(NapCat/LLOneBot 非 0),但撤回
//! 通知不带 seq、decode 恒填 0,故同一条消息在「收到」与「被撤」两刻的 `MessageId` 整体并不相等;
//! 唯一稳定的锚是 `onebot_id`(decode 注释明言「撤回/get_msg 以 onebot_id 为锚」)。Milky 无
//! `onebot_id`,退回 `(peer, seq)`。`HashMap` 做查,`VecDeque` 记插入序做淘汰;超过 `cap` 即从队头
//! 逐出最旧的。纯内存、进程级,无持久化。`cap == 0` 表示禁用(record 直接跳过)。

use nagisa_types::id::{MessageId, Peer, Uin};
use nagisa_types::segment::Segment;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// 缓存键:跨「原消息 / 撤回通知 / get_msg」稳定的锚点(见模块文档)。
#[derive(Clone, PartialEq, Eq, Hash)]
enum MsgKey {
    /// OneBot:`onebot_id`(message_id)是三者间唯一稳定的锚——撤回通知里 `seq` 恒为 0,
    /// 不可靠,故只认 `(peer, onebot_id)`。
    OneBot(Peer, i32),
    /// Milky:无 `onebot_id`,用 `(peer, seq)`。
    Seq(Peer, i64),
}

impl MsgKey {
    fn of(id: &MessageId) -> Self {
        match id.onebot_id {
            Some(ob) => MsgKey::OneBot(id.peer, ob),
            None => MsgKey::Seq(id.peer, id.seq),
        }
    }
}

/// 暂存的一条消息:发送者 + 内容段(供渲染被撤消息的预览 / 业务取回)。
#[derive(Clone, Debug)]
pub struct StoredMessage {
    /// 发送者 QQ 号。
    pub sender: Uin,
    /// 消息内容段。
    pub content: Vec<Segment>,
}

/// 进程级、容量有界的最近消息缓存(按 [`MessageId`])。克隆即共享(内部 `Arc`)。
pub struct MessageStore {
    inner: Mutex<Inner>,
    cap: usize,
}

#[derive(Default)]
struct Inner {
    map: HashMap<MsgKey, StoredMessage>,
    order: VecDeque<MsgKey>,
}

impl MessageStore {
    /// 新建一个容量为 `cap` 的共享缓存(`cap == 0` 即禁用)。
    pub fn shared(cap: usize) -> Arc<Self> {
        Arc::new(Self { inner: Mutex::new(Inner::default()), cap })
    }

    /// 记一条消息。容量满则逐出最旧的。`cap == 0` 直接跳过。
    pub fn record(&self, id: MessageId, sender: Uin, content: Vec<Segment>) {
        if self.cap == 0 {
            return;
        }
        let key = MsgKey::of(&id);
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.map.insert(key.clone(), StoredMessage { sender, content }).is_none() {
            // 仅新键入队;重复键(同 id 再发,罕见)只更新值、不重复入队。
            inner.order.push_back(key);
        }
        while inner.order.len() > self.cap {
            if let Some(old) = inner.order.pop_front() {
                inner.map.remove(&old);
            }
        }
    }

    /// 取回某条消息(若仍在缓存里)。
    pub fn get(&self, id: &MessageId) -> Option<StoredMessage> {
        self.inner.lock().ok()?.map.get(&MsgKey::of(id)).cloned()
    }
}
