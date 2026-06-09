//! 引擎错误类型。`render_*` 返回 `Result`,调用方决定回退(如改发纯文字)。

use thiserror::Error;

/// 排版引擎错误。
#[derive(Debug, Error)]
pub enum Error {
    /// 字体栈为空 / 字体数据损坏。
    #[error("字体加载失败:{0}")]
    FontLoad(String),
    /// 标记语言语法错误。
    #[error("标记解析错误(第 {line} 行):{msg}")]
    Parse {
        /// 出错的行号(从 1 起)。
        line: usize,
        /// 错误说明。
        msg: String,
    },
    /// 内嵌图片解码失败 / `@名字` 未提供。
    #[error("图片错误:{0}")]
    Image(String),
    /// 图片编码失败。
    #[error("图片编码失败:{0}")]
    Encode(String),
    /// 版式异常(如宽度 ≤ 0、画布尺寸非法)。
    #[error("版式错误:{0}")]
    Layout(String),
}

/// 引擎内部用的 `Result` 别名。
pub type Result<T> = std::result::Result<T, Error>;
