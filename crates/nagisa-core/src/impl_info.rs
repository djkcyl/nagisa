//! 各 adapter 连接时上报的实现信息（best-effort）。

/// 底层 bot 实现的名称与版本（协议端信息）。
///
/// 各 adapter 在连接成功后经 best-effort 的 `get_version_info` / `get_impl_info` 调用填充。
/// 经 `adapter.impl_info()` 取用。厂商判定是 OneBot 专用轴、不进本类型——走
/// [`ActionInvoker::vendor`](crate::invoker::ActionInvoker::vendor)。
#[derive(Clone, Debug)]
pub struct ImplInfo {
    pub name: String,
    pub version: String,
    /// QQ 协议版本（Milky `get_impl_info.qq_protocol_version`；OneBot 端为 `None`）。
    pub qq_protocol_version: Option<String>,
    /// QQ 协议类型（Milky `get_impl_info.qq_protocol_type`；OneBot 端为 `None`）。
    pub qq_protocol_type: Option<String>,
    /// Milky 协议规范版本（`get_impl_info.milky_version`，独立于实现版本 `version`）。
    /// 仅 Milky 端填充；OneBot 端为 `None`。surface 它以便上层按 Milky spec 版本协商能力。
    pub milky_version: Option<String>,
}
