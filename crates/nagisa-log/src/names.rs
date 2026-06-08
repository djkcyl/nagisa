//! 名称缓存:把群号 / QQ 号尽量解析成可读名字,供事件日志渲染;并顺带缓存商城表情的规范名。
//!
//! 两路来源,互补:
//! 1. **就地学习**(同步、无需 Bot):从观察到的事件里学名字——消息发送者的群名片/昵称、
//!    `group_name_change` 的新群名、消息自带的群信息等。`EventLog` 每事件调一次 [`learn_from_event`]。
//! 2. **Bot API 回填**(detached 后台任务,绝不阻塞日志/命令):启动时一次性预取群列表 +
//!    好友列表([`maybe_prefetch`]);首次见到某群时取该群信息 + **整份成员名单**([`ensure_group`]),
//!    一次调用即给该群所有成员命名,之后该群的表情回应/通知等都能从缓存解析。
//!
//! 商城表情另有一路:首次见到某表情包时 detached 拉 gtimg 表情包列表 JSON,反查出不受发送方改写的
//! 规范名(`包名·表情名`),缓存供渲染([`ensure_market_face`] / [`market_face_display`])。
//!
//! 查询([`user`]/[`group`])是同步只读;渲染直接查缓存,查不到就回落到裸号(当次显示号、
//! 回填完成后的后续事件再显示名)。失败/缺权限的回填静默放弃,绝不刷屏。
//!
//! [`learn_from_event`]: NameStore::learn_from_event
//! [`maybe_prefetch`]: NameStore::maybe_prefetch
//! [`ensure_group`]: NameStore::ensure_group
//! [`ensure_market_face`]: NameStore::ensure_market_face
//! [`market_face_display`]: NameStore::market_face_display
//! [`user`]: NameStore::user
//! [`group`]: NameStore::group

use nagisa_core::Bot;
use nagisa_types::event::{Event, Notice};
use nagisa_types::id::{Scene, Uin};
use nagisa_types::segment::Segment;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// 进程级共享的名称缓存(群号→群名、QQ 号→显示名)。克隆即共享(内部 `Arc` 字段),
/// 通常以 `Arc<NameStore>` 传递。
#[derive(Default)]
pub struct NameStore {
    /// 全局名:`uin → 昵称`（QQ 昵称、好友名、机器人自己）。跨群一致,作为群内名片缺失时的回落。
    users: RwLock<HashMap<i64, String>>,
    /// **按群**的群内显示名:`(group, uin) → 群名片(或群内昵称)`。群名片**因群而异**,故按 (群,人)
    /// 分别存,不会把 A 群的名片串到 B 群。查不到则回落到全局 [`users`](Self::users)。
    group_members: RwLock<HashMap<(i64, i64), String>>,
    groups: RwLock<HashMap<i64, String>>,
    /// 已尝试过 API 回填的群(无论成败,每群每进程只取一次,避免失败重试刷屏)。
    attempted: Mutex<HashSet<i64>>,
    /// 是否已做过一次性预取(群列表 + 好友列表)。
    prefetched: AtomicBool,
    /// 商城表情**规范名**缓存:`(emoji_package_id, emoji_id) → 原名`。`summary` 是发送方可改写的
    /// 描述、拿不到原名,故按 `emoji_package_id` 拉 gtimg 表情包列表反查(见 [`ensure_market_face`])。
    market_faces: RwLock<HashMap<(i32, String), String>>,
    /// 表情**包名**缓存:`emoji_package_id → 包名`(gtimg JSON 顶层 `name`)。
    mface_packages: RwLock<HashMap<i32, String>>,
    /// 已尝试拉过的表情包 id(每包每进程只拉一次)。
    mface_attempted: Mutex<HashSet<i32>>,
}

impl NameStore {
    /// 新建一个共享缓存句柄。
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 查 QQ 号的**全局**显示名(昵称;命中返回,否则 `None`)。
    pub fn user(&self, uin: i64) -> Option<String> {
        self.users.read().ok()?.get(&uin).cloned()
    }

    /// 查某人在**某群**的显示名:优先该群的群名片,回落到全局昵称([`user`](Self::user))。
    pub fn member(&self, group: i64, uin: i64) -> Option<String> {
        if let Ok(m) = self.group_members.read() {
            if let Some(name) = m.get(&(group, uin)) {
                return Some(name.clone());
            }
        }
        self.user(uin)
    }

    fn put_member(&self, group: i64, uin: i64, name: &str) {
        let name = name.trim();
        if group == 0 || uin == 0 || name.is_empty() {
            return;
        }
        if let Ok(mut m) = self.group_members.write() {
            m.insert((group, uin), name.to_string());
        }
    }

    /// 查群号的群名(命中返回名字,否则 `None`)。
    pub fn group(&self, gid: i64) -> Option<String> {
        self.groups.read().ok()?.get(&gid).cloned()
    }

    fn put_user(&self, uin: i64, name: &str) {
        let name = name.trim();
        if uin == 0 || name.is_empty() {
            return;
        }
        if let Ok(mut m) = self.users.write() {
            m.insert(uin, name.to_string());
        }
    }

    fn put_group(&self, gid: i64, name: &str) {
        let name = name.trim();
        if gid == 0 || name.is_empty() {
            return;
        }
        if let Ok(mut m) = self.groups.write() {
            m.insert(gid, name.to_string());
        }
    }

    /// 从一个事件里就地学名字(同步,无需 Bot):消息发送者名、消息自带的群名、改群名/群名片通知。
    pub fn learn_from_event(&self, ev: &Event) {
        match ev {
            Event::Message(m) if m.peer.scene == Scene::Group => {
                // 群内:群名片(或群内昵称)落 **per-group**;全局 QQ 昵称落 users(跨群一致的回落)。
                if let Some(name) = m.member.as_ref().and_then(|mem| non_empty(mem.display_name())) {
                    self.put_member(m.peer.id.0, m.sender.0, &name);
                }
                if let Some(nick) = m.member.as_ref().map(|mem| mem.nickname.trim()) {
                    self.put_user(m.sender.0, nick);
                }
                if let Some(g) = &m.group {
                    self.put_group(m.peer.id.0, &g.name);
                }
            }
            Event::Message(m) => {
                // 私聊/临时:只有全局名可学。
                if let Some(name) = m.friend.as_ref().and_then(|f| non_empty(f.display_name())) {
                    self.put_user(m.sender.0, &name);
                }
            }
            Event::Notice(Notice::GroupNameChange { group, new_name, .. }) => {
                self.put_group(group.0, new_name);
            }
            // 改群名片会发此通知,直接更新该群的 per-group 缓存——不用等他下次发言(空名片跳过)。
            Event::Notice(Notice::GroupCardChange { group, user, new_card, .. }) => {
                self.put_member(group.0, user.0, new_card);
            }
            // 框架就绪事件带了机器人自己的账号+昵称,顺手学进缓存,使机器人的 uin 在别处也能解析名。
            Event::Meta(nagisa_types::event::Meta::Ready { self_id, nickname }) => {
                self.put_user(self_id.0, nickname);
            }
            _ => {}
        }
    }

    /// 后台回填该事件涉及的名字(detached,绝不阻塞):先确保做过一次性预取,再对群作用域
    /// 事件确保解析其群名 + 成员名单。
    pub fn backfill(self: &Arc<Self>, bot: &Bot, ev: &Event) {
        self.maybe_prefetch(bot);
        if let Some(peer) = ev.peer() {
            if peer.scene == Scene::Group {
                self.ensure_group(bot, peer.id.0);
            }
        }
        // 进群:整份名单只拉一次,但拉过之后新进群的人不在缓存里——这里只取**这一个**新成员补进去
        // (不重拉整份)。`MemberIncrease` 通知带了群号 + 新成员号。
        if let Event::Notice(Notice::MemberIncrease { group, user, .. }) = ev {
            self.ensure_member(bot, group.0, user.0);
        }
        // 商城表情:对消息里每个 mface 段,确保拉过其表情包列表(反查规范名)。
        if let Event::Message(m) = ev {
            for seg in &m.content {
                if let Segment::MarketFace { package_id, .. } = seg {
                    self.ensure_market_face(*package_id);
                }
            }
        }
    }

    /// 查商城表情的**规范名**(命中返回原名,否则 `None`——回落到 summary）。
    pub fn market_face(&self, package_id: i32, emoji_id: &str) -> Option<String> {
        self.market_faces.read().ok()?.get(&(package_id, emoji_id.to_string())).cloned()
    }

    /// 查表情**包名**（命中返回包名，否则 `None`）。
    pub fn market_face_package(&self, package_id: i32) -> Option<String> {
        self.mface_packages.read().ok()?.get(&package_id).cloned()
    }

    /// 商城表情的渲染显示名:命中规范名时返回 `包名·表情名`（包名缺失则仅表情名）；否则 `None`。
    pub fn market_face_display(&self, package_id: i32, emoji_id: &str) -> Option<String> {
        let sticker = self.market_face(package_id, emoji_id)?;
        match self.market_face_package(package_id) {
            Some(pkg) => Some(format!("{pkg}·{sticker}")),
            None => Some(sticker),
        }
    }

    /// 写入一条商城表情规范名(`ensure_market_face` 解析 gtimg 列表后逐条灌入)。
    pub(crate) fn put_market_face(&self, package_id: i32, emoji_id: &str, name: &str) {
        let name = name.trim();
        if emoji_id.is_empty() || name.is_empty() {
            return;
        }
        if let Ok(mut m) = self.market_faces.write() {
            m.insert((package_id, emoji_id.to_string()), name.to_string());
        }
    }

    /// 写入一条表情包名(`ensure_market_face` 从 gtimg JSON 顶层 `name` 取得)。
    pub(crate) fn put_market_face_package(&self, package_id: i32, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Ok(mut m) = self.mface_packages.write() {
            m.insert(package_id, name.to_string());
        }
    }

    /// 首次见到某表情包:detached 拉 gtimg 表情包列表 JSON,把其中每个表情的
    /// `(package_id, emoji_id) → name`(规范名)灌进缓存。每包每进程只拉一次(无论成败)。
    pub fn ensure_market_face(self: &Arc<Self>, package_id: i32) {
        if package_id == 0 {
            return;
        }
        {
            let mut tried = match self.mface_attempted.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if !tried.insert(package_id) {
                return;
            }
        }
        let store = Arc::clone(self);
        tokio::spawn(async move {
            let url = gtimg_url(package_id);
            tracing::debug!(target: "nagisa::mface", package_id, %url, "拉取商城表情包列表");
            let resp = match reqwest::get(&url).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(target: "nagisa::mface", package_id, error = %e, "拉取商城表情包失败");
                    return;
                }
            };
            let json = match resp.json::<serde_json::Value>().await {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(target: "nagisa::mface", package_id, error = %e, "解析商城表情包 JSON 失败");
                    return;
                }
            };
            // 结构:{ name(包名), imgs: [{ id, name, ... }], ... }。`name` 是不受发送方改写的规范名。
            let mut count = 0usize;
            if let Some(imgs) = json.get("imgs").and_then(|v| v.as_array()) {
                for img in imgs {
                    if let (Some(id), Some(name)) = (
                        img.get("id").and_then(|v| v.as_str()),
                        img.get("name").and_then(|v| v.as_str()),
                    ) {
                        store.put_market_face(package_id, id, name);
                        count += 1;
                    }
                }
            }
            let pkg = json.get("name").and_then(|v| v.as_str()).unwrap_or("");
            store.put_market_face_package(package_id, pkg);
            tracing::info!(target: "nagisa::mface", package_id, pkg = %pkg, count, "已解析商城表情包");
        });
    }

    /// 首次见到某群:detached 取群信息(群名)+ 整份成员名单(给该群所有成员命名)。
    /// 每群每进程只尝试一次(无论成败),已知群名则直接跳过。
    pub fn ensure_group(self: &Arc<Self>, bot: &Bot, gid: i64) {
        if gid == 0 {
            return;
        }
        // 仅以「是否取过成员名单」去重——**不**因群名已知(预取拿到的是群名,不是成员名单)就跳过,
        // 否则成员名单永远不会拉、没发言的成员永远解析不出名字。
        {
            let mut tried = match self.attempted.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if !tried.insert(gid) {
                return; // 已在取了 / 取过了
            }
        }
        let store = Arc::clone(self);
        let bot = bot.clone();
        tokio::spawn(async move {
            tracing::debug!(target: "nagisa::names", group = gid, "解析群名与成员名单");
            let mut group_name = String::new();
            match bot.get_group_info(Uin(gid), false).await {
                Ok(info) => {
                    store.put_group(gid, &info.name);
                    group_name = info.name;
                }
                Err(e) => {
                    tracing::warn!(target: "nagisa::names", group = gid, error = %e, "拉取群信息失败")
                }
            }
            let mut members_count = 0usize;
            match bot.get_group_member_list(Uin(gid), false).await {
                Ok(members) => {
                    for m in members {
                        // 群名片(或群内昵称)落 per-group;全局 QQ 昵称落 users。
                        store.put_member(gid, m.user.0, m.display_name());
                        let nick = m.nickname.trim();
                        if !nick.is_empty() {
                            store.put_user(m.user.0, nick);
                        }
                        members_count += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!(target: "nagisa::names", group = gid, error = %e, "拉取群成员名单失败")
                }
            }
            tracing::info!(target: "nagisa::names", group = gid, name = %group_name, members = members_count, "已解析群");
        });
    }

    /// 拉**单个**群成员的信息(detached)并缓存其群名片(per-group)+ 全局昵称。
    /// 用于增量场景(如「进群」通知):新成员加群时只取他一个,无需重拉整份名单。
    pub fn ensure_member(self: &Arc<Self>, bot: &Bot, group: i64, uin: i64) {
        if group == 0 || uin == 0 {
            return;
        }
        let store = Arc::clone(self);
        let bot = bot.clone();
        tokio::spawn(async move {
            match bot.get_group_member_info(Uin(group), Uin(uin), false).await {
                Ok(m) => {
                    store.put_member(group, m.user.0, m.display_name());
                    let nick = m.nickname.trim();
                    if !nick.is_empty() {
                        store.put_user(m.user.0, nick);
                    }
                    tracing::debug!(target: "nagisa::names", group, uin, "已解析新成员");
                }
                Err(e) => {
                    tracing::warn!(target: "nagisa::names", group, uin, error = %e, "拉取群成员信息失败")
                }
            }
        });
    }

    /// 一次性预取(每进程一次):群列表(群名)+ 好友列表(好友名),detached。
    pub fn maybe_prefetch(self: &Arc<Self>, bot: &Bot) {
        if self.prefetched.swap(true, Ordering::Relaxed) {
            return;
        }
        let store = Arc::clone(self);
        let bot = bot.clone();
        tokio::spawn(async move {
            tracing::debug!(target: "nagisa::names", "预取群列表 + 好友列表");
            match bot.get_group_list(false).await {
                Ok(groups) => {
                    let n = groups.len();
                    for g in groups {
                        store.put_group(g.group.0, &g.name);
                    }
                    tracing::info!(target: "nagisa::names", count = n, "已预取群名");
                }
                Err(e) => tracing::warn!(target: "nagisa::names", error = %e, "预取群列表失败"),
            }
            match bot.get_friend_list(false).await {
                Ok(friends) => {
                    let n = friends.len();
                    for f in friends {
                        store.put_user(f.user.0, f.display_name());
                    }
                    tracing::info!(target: "nagisa::names", count = n, "已预取好友名");
                }
                Err(e) => tracing::warn!(target: "nagisa::names", error = %e, "预取好友列表失败"),
            }
        });
    }
}

/// 空串视作「无名」→ `None`;非空则拥有化返回。
fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// gtimg 表情包列表 URL：目录是 `package_id` 的**末位数字**,文件名是 `<package_id>_android.json`。
/// 例:240869 → `.../parcel/9/240869_android.json?mType=VIP_emosm`。
fn gtimg_url(package_id: i32) -> String {
    format!(
        "https://i.gtimg.cn/club/item/parcel/{}/{}_android.json?mType=VIP_emosm",
        package_id.rem_euclid(10),
        package_id
    )
}
