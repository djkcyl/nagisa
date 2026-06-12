//! 两个协议适配器(nagisa-onebot / nagisa-milky)共用的 **wire 层基建**:协议帧日志漏斗
//! [`log_wire`]、零依赖 base64 编码 [`base64_encode`]、以及 HTTP 动作通道的公共骨架
//! [`http_action_envelope`]。
//!
//! 这些都是「与具体协议字段无关、两适配器逐字同实现」的部分,上移到 core 消重;协议本质差异
//! (封包字段名、retcode 语义、echo 关联等)仍留在各 crate——core 只抽两边**读起来是同一个
//! 形状**的骨架。

use nagisa_types::error::{Error, Result};

/// 原始网络帧 debug 日志(target `nagisa::wire`,`dir`="in"/"out")。
///
/// 两个适配器共用同一 `nagisa::wire` target + debug 级:一条
/// `RUST_LOG=info,nagisa::wire=debug` 即抓两个适配器、收发两端的所有协议帧
/// (未解析、未过滤)。默认关、零开销——这是跨适配器统一的**观测口径**,不要改 target 名。
pub fn log_wire(dir: &'static str, frame: &str) {
    tracing::debug!(target: "nagisa::wire", dir, "{frame}");
}

/// base64 编码(标准 alphabet、带 padding、无换行;零外部依赖)。
///
/// 两适配器出站把媒体字节拼成 `base64://…` URI 时共用此实现(原来 milky / onebot 各手写一份
/// 逐字节同逻辑)。标准 RFC 4648 alphabet,每 3 字节 → 4 个 base64 字符,不足 3 字节补 `=`。
pub fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// HTTP 动作通道的**公共骨架**(两适配器逐字同形状的「404→Unsupported + 封包成功检查 +
/// classify」三段)。各 crate 先自己做完网络 IO(POST、读 body)与**协议专属的非 2xx 预筛**
/// (milky 只短路 401/405,onebot 把所有非 2xx 都判 Action——这部分差异留在各 crate,不进骨架),
/// 再把 `action` 名、HTTP 状态码、响应正文与一个 `parse` 闭包交给本函数收尾:
///
/// - **404** → 返回携带 action 名的 [`Error::Unsupported`](未知 action / 路由未命中,非封包;
///   两适配器一致,故放进骨架)。
/// - 其余状态:调 `parse` 把正文解析成 [`Envelope`](封包的 status/retcode/data/message 四元组,
///   外加已归类的 `classify`)。各 crate 封包字段名不同(milky `message`、onebot `msg`/`wording`
///   alias),retcode 语义也不同,故由调用方用自己的 wire 结构反序列化、并把 retcode 启发式归类
///   后填进 [`Envelope`];解析失败应返回 `Err`(如 `Error::Decode`),本函数原样透传——绝不把
///   无法解析的 body 吞成 `Ok(Null)`。
/// - 封包 `status=="ok" && retcode==0` → 返回 `data`(缺省 `Value::Null`);否则
///   [`Error::Action { retcode, message, kind: classify }`](拿 [`Envelope`] 里的归类)。
pub fn http_action_envelope<P>(action: &str, status: u16, body: &str, parse: P) -> Result<serde_json::Value>
where
    P: FnOnce(&str) -> Result<Envelope>,
{
    // 未知 action / 路由未命中 → HTTP 404(非封包)→ Unsupported。
    if status == 404 {
        return Err(Error::Unsupported(action.to_string()));
    }
    // 解封包(解析失败原样透传,绝不吞成 Ok(Null))。
    let env = parse(body)?;
    if env.status == "ok" && env.retcode == 0 {
        Ok(env.data.unwrap_or(serde_json::Value::Null))
    } else {
        Err(Error::Action { retcode: env.retcode, message: env.message.unwrap_or_default(), kind: env.classify })
    }
}

/// HTTP 动作响应封包的**统一四元组视图**(供 [`http_action_envelope`] 的 `parse` 闭包返回)。
///
/// 各 crate 用自己的 wire 结构(字段名不同)反序列化 body 后,填进这个统一形状;`classify` 槽
/// 让调用方在解析时就把 retcode 归类好交进来(retcode 语义各 crate 不同,故归类逻辑留在调用侧)。
pub struct Envelope {
    /// 状态串;成功约定为 `"ok"`。
    pub status: String,
    /// 返回码;成功约定为 `0`。
    pub retcode: i64,
    /// 成功时的载荷(缺省视为 `Value::Null`)。
    pub data: Option<serde_json::Value>,
    /// 失败时的文案(各 crate 字段名不同,解析时已归一到此)。
    pub message: Option<String>,
    /// 失败时的归类(由调用方对 retcode 启发式分类后填入)。
    pub classify: nagisa_types::error::ActionErrorKind,
}
