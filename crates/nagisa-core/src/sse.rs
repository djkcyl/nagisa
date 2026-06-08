//! **纯** SSE(Server-Sent Events)逐 chunk 行缓冲解析器:喂入 `&[u8]` 字节块、吐出每条
//! 事件的 `data:` payload。不依赖 reqwest / 网络——各站点保留自己的网络读取与 idle 超时逻辑,
//! 只把「按 \n 切行 + 累积 data 行 + 空行即一条事件」这套字节级小状态机收敛到这里共用。
//!
//! 仅实现 nagisa 用到的最小 SSE 子集:`data:` 字段行(可有可无一个前导空格)累积,空行作事件
//! 边界,其余字段行(`event:` / `id:` / `:comment` 等)忽略——nagisa 的事件 payload 全在 data 行。

/// SSE 行缓冲解析器。`feed` 增量喂字节、返回本次新凑齐的事件 payload。
#[derive(Debug, Default)]
pub struct SseParser {
    /// 未凑满一行的尾部字节。
    buf: Vec<u8>,
    /// 当前事件已累积的 `data:` 行(空行边界时 join 为一条 payload)。
    data_lines: Vec<String>,
}

impl SseParser {
    /// 新建空解析器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一个字节块,返回本次凑齐的事件 payload 列表(可能为空)。多条 `data:` 行以 `\n`
    /// 连接成单条 payload(与 SSE 规范一致);非 `data:` 字段行被忽略。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut events = Vec::new();
        self.buf.extend_from_slice(chunk);

        // 按 \n 切行,逐行处理;保留未完整的尾行在 buf 里。
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = self.buf.drain(..=pos).collect();
            // 去掉行尾 \n 和可能的 \r。
            let line = String::from_utf8_lossy(&line_bytes);
            let line = line.trim_end_matches('\n').trim_end_matches('\r');

            if line.is_empty() {
                // 事件边界:拼接累积的 data 行为一条 payload。
                if !self.data_lines.is_empty() {
                    events.push(std::mem::take(&mut self.data_lines).join("\n"));
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                // SSE: `data:` 后可有一个可选空格。
                self.data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
            // 其余字段行(`event:` / `id:` / `:comment` 等)忽略。
        }
        events
    }
}
